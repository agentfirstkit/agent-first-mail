import { env } from "cloudflare:workers";
import { runInDurableObject, runDurableObjectAlarm } from "cloudflare:test";
import type { SpoolQuota } from "../src/quota";
import { beforeEach, describe, expect, it, vi } from "vitest";
import worker, { type Env } from "../src/index";

const bindings = env as unknown as Env;
const raw = new TextEncoder().encode("From: sender@example.net\r\nTo: visible@example.net\r\nSubject: Spool\r\n\r\nHello\r\n");
function email(forward = vi.fn().mockResolvedValue(undefined)) {
  return {
    from: "sender@example.net", to: "hidden@example.com",
    raw: new ReadableStream({ start(controller) { controller.enqueue(raw); controller.close(); } }),
    rawSize: raw.length, setReject: vi.fn(), forward,
  } as unknown as ForwardableEmailMessage;
}

beforeEach(async () => {
  const stub = bindings.MAIL_QUOTA.get(bindings.MAIL_QUOTA.idFromName("spool"));
  await runInDurableObject(stub, (_instance, state) => {
    state.storage.sql.exec("DELETE FROM objects");
    state.storage.sql.exec("DELETE FROM settings");
  });
  const listed = await bindings.MAIL_SPOOL.list();
  for (const object of listed.objects) await bindings.MAIL_SPOOL.delete(object.key);
});

describe("email spool", () => {
  const stub = () => bindings.MAIL_QUOTA.get(bindings.MAIL_QUOTA.idFromName("spool"));
  async function usage() {
    return (await (await stub().fetch("https://spool.invalid/v1/quota")).json() as { result: { used_bytes: number; object_count: number } }).result;
  }
  it("rejects declared and actual oversize messages before writing", async () => {
    for (const declared of [0, 100000]) {
      const message = { ...email(), rawSize: declared, setReject: vi.fn() };
      await worker.email(message as unknown as ForwardableEmailMessage, { ...bindings, MAX_MESSAGE_BYTES: "10" });
      expect(message.setReject).toHaveBeenCalledOnce();
      expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(0);
    }
  });
  it("counts preexisting bucket objects and rate windows before admission", async () => {
    await bindings.MAIL_SPOOL.put("imported/existing.eml", raw);
    expect(await usage()).toMatchObject({ used_bytes: raw.length, object_count: 1 });
    await runInDurableObject(stub(), (_instance, state) => {
      state.storage.sql.exec("INSERT OR REPLACE INTO settings VALUES ('rate_window', ?)", String(Math.floor(Date.now() / 60000)));
      state.storage.sql.exec("INSERT OR REPLACE INTO settings VALUES ('rate_count', '100')");
    });
    const message = { ...email(), setReject: vi.fn() };
    await worker.email(message as unknown as ForwardableEmailMessage, bindings);
    expect(message.setReject).toHaveBeenCalledOnce();
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(1);
  });
  it("settles a failed write by fencing its key and releasing the reservation", async () => {
    await usage();
    const response = await runInDurableObject(stub(), async (instance: SpoolQuota) => {
      const object = instance as unknown as { env: Env };
      const original = object.env;
      object.env = { ...original, MAIL_SPOOL: { put: async () => { throw new Error("lost write response"); } } as unknown as R2Bucket };
      try { return await instance.fetch(new Request("https://spool.invalid/put", { method: "POST", body: raw })); }
      finally { object.env = original; }
    });
    expect(response.status).toBe(503);
    expect(await usage()).toMatchObject({ used_bytes: raw.length, object_count: 1 });
    expect(await runDurableObjectAlarm(stub())).toBe(true);
    expect(await usage()).toMatchObject({ used_bytes: 0, object_count: 0 });
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(0);
  });
  it("atomically rejects concurrent writes when the byte quota is reserved", async () => {
    // Seed existing bucket usage to leave room for exactly one message.
    await stub().fetch("https://spool.invalid/v1/quota");
    await runInDurableObject(stub(), (_instance, state) => {
      state.storage.sql.exec("INSERT INTO objects VALUES ('existing', ?, 'reserved', NULL)", 1073741824 - raw.length);
    });
    const messages = Array.from({ length: 10 }, () => ({ ...email(), setReject: vi.fn() }));
    await Promise.all(messages.map(message => worker.email(message as unknown as ForwardableEmailMessage, bindings)));
    expect(messages.filter(message => message.setReject.mock.calls.length === 0)).toHaveLength(1);
    expect((await usage()).used_bytes).toBe(1073741824);
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(1);
  });
  it("releases confirmed deletions and uncertain writes that never landed", async () => {
    await worker.email(email(), bindings);
    const object = (await bindings.MAIL_SPOOL.list()).objects[0];
    await bindings.MAIL_SPOOL.delete(object.key);
    await runInDurableObject(stub(), (_instance, state) => {
      state.storage.sql.exec("INSERT INTO objects VALUES ('inbox/uncertain.eml', 123, 'reserved', NULL)");
    });
    expect(await runDurableObjectAlarm(stub())).toBe(true);
    expect(await usage()).toMatchObject({ used_bytes: 0, object_count: 0 });
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(0);
  });
  it("keeps the charge for an uncertain write that landed, at its actual size", async () => {
    await usage();
    await bindings.MAIL_SPOOL.put("inbox/landed.eml", raw);
    await runInDurableObject(stub(), (_instance, state) => {
      state.storage.sql.exec("INSERT INTO objects VALUES ('inbox/landed.eml', 1, 'reserved', NULL)");
      return state.storage.setAlarm(Date.now() + 60000);
    });
    expect(await runDurableObjectAlarm(stub())).toBe(true);
    expect(await usage()).toMatchObject({ used_bytes: raw.length, object_count: 1 });
    expect(new Uint8Array(await (await bindings.MAIL_SPOOL.get("inbox/landed.eml"))!.arrayBuffer())).toEqual(raw);
  });
  it("never fences a write still in flight, and clears a fence left by an interrupted settlement", async () => {
    await usage();
    await bindings.MAIL_SPOOL.put("inbox/fenced.eml", new Uint8Array(), { customMetadata: { spool_fence: "true" } });
    await runInDurableObject(stub(), (instance: SpoolQuota, state) => {
      state.storage.sql.exec("INSERT INTO objects VALUES ('inbox/writing.eml', 7, 'reserved', NULL)");
      state.storage.sql.exec("INSERT INTO objects VALUES ('inbox/fenced.eml', 7, 'reserved', NULL)");
      (instance as unknown as { writing: Set<string> }).writing.add("inbox/writing.eml");
      return state.storage.setAlarm(Date.now() + 60000);
    });
    expect(await runDurableObjectAlarm(stub())).toBe(true);
    expect(await usage()).toMatchObject({ used_bytes: 7, object_count: 1 });
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(0);
  });
  it("archives through the ledger and acknowledges idempotently", async () => {
    await worker.email(email(), bindings);
    const key = (await bindings.MAIL_SPOOL.list()).objects[0].key;
    const archive_key = `imported/${key.slice("inbox/".length)}`;
    for (let i = 0; i < 2; i++) {
      const response = await stub().fetch("https://spool.invalid/v1/ack", { method: "POST", body: JSON.stringify({ key, archive_key }) });
      expect(response.ok).toBe(true);
    }
    expect(await bindings.MAIL_SPOOL.head(key)).toBeNull();
    expect(await usage()).toMatchObject({ used_bytes: raw.length, object_count: 1 });
    expect(new Uint8Array(await (await bindings.MAIL_SPOOL.get(archive_key))!.arrayBuffer())).toEqual(raw);
  });
  it("completes an archive copy whose earlier write outcome was unknown", async () => {
    await worker.email(email(), bindings);
    const key = (await bindings.MAIL_SPOOL.list()).objects[0].key;
    const archive_key = `imported/${key.slice("inbox/".length)}`;
    await runInDurableObject(stub(), (_instance, state) => {
      state.storage.sql.exec("INSERT INTO objects VALUES (?, ?, 'reserved', ?)", archive_key, raw.length, key);
    });
    const response = await stub().fetch("https://spool.invalid/v1/ack", { method: "POST", body: JSON.stringify({ key, archive_key }) });
    expect(response.ok).toBe(true);
    expect(await bindings.MAIL_SPOOL.head(key)).toBeNull();
    expect(new Uint8Array(await (await bindings.MAIL_SPOOL.get(archive_key))!.arrayBuffer())).toEqual(raw);
    expect(await usage()).toMatchObject({ used_bytes: raw.length, object_count: 1 });
  });
  it("requires control authentication and an independent verifier for strict source policy", async () => {
    const config = { ...bindings, SPOOL_CONTROL_TOKEN_SECRET: "x".repeat(32) };
    expect((await worker.fetch(new Request("https://worker.invalid/v1/quota"), config)).status).toBe(401);
    expect((await worker.fetch(new Request("https://worker.invalid/v1/quota", { headers: { Authorization: `Bearer ${"x".repeat(32)}` } }), config)).ok).toBe(true);
    await expect(worker.email(email(), { ...bindings, REQUIRE_DMARC_PASS: "true" })).rejects.toThrow("trusted SENDER_VERIFIER");
    const verified = { fetch: vi.fn().mockResolvedValue(Response.json({ authenticated_domain: "sender.example", aligned: true, dmarc: "pass" })) } as unknown as Fetcher;
    await worker.email(email(), { ...bindings, REQUIRE_DMARC_PASS: "true", ALLOWED_SENDER_DOMAINS: "sender.example", SENDER_VERIFIER: verified });
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(1);
  });
  it("preserves exact MIME bytes and records the delivery envelope", async () => {
    await worker.email(email(), bindings);
    const listed = await bindings.MAIL_SPOOL.list();
    expect(listed.objects).toHaveLength(1);
    expect(listed.objects[0].key).toMatch(/^inbox\/\d{8}T\d{6}Z-[0-9a-f]{32}\.eml$/);
    const object = await bindings.MAIL_SPOOL.get(listed.objects[0].key);
    expect(new Uint8Array(await object!.arrayBuffer())).toEqual(raw);
    expect(object!.httpMetadata?.contentType).toBe("message/rfc822");
    expect(object!.customMetadata).toEqual({ envelope_from: "sender@example.net", envelope_to: "hidden@example.com", received_rfc3339: expect.any(String), quota_managed: "true" });
    expect(Number.isNaN(Date.parse(object!.customMetadata!.received_rfc3339))).toBe(false);
  });
  it("forwards only after storing, and retains storage if forwarding fails", async () => {
    const forward = vi.fn().mockRejectedValue(new Error("test rejection"));
    await worker.email(email(forward), { ...bindings, FORWARD_TO: "copy@example.com" });
    expect(forward).toHaveBeenCalledWith("copy@example.com");
    expect((await bindings.MAIL_SPOOL.list()).objects).toHaveLength(1);
  });
  it("propagates storage failures without forwarding", async () => {
    const forward = vi.fn();
    const failing = { idFromName: vi.fn(), get: () => ({ fetch: vi.fn().mockResolvedValue(new Response("unavailable", { status: 503 })) }) } as unknown as DurableObjectNamespace;
    await expect(worker.email(email(forward), { ...bindings, MAIL_QUOTA: failing, FORWARD_TO: "copy@example.com" })).rejects.toThrow("did not confirm acceptance");
    expect(forward).not.toHaveBeenCalled();
  });
});
