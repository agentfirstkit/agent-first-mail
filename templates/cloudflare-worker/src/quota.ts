import { DurableObject } from "cloudflare:workers";
import { failure, LimitExceeded, limits, readBounded, result, type PolicyEnv } from "./policy";
export interface QuotaEnv extends PolicyEnv { MAIL_SPOOL: R2Bucket; }
type Entry = { key: string; size_bytes: number; state: string; source_key: string | null; }

/** One instance named spool owns admission and the complete bucket ledger. */
export class SpoolQuota extends DurableObject<QuotaEnv> {
  /** Keys whose R2 write this instance is still awaiting. */
  private readonly writing = new Set<string>();
  constructor(ctx: DurableObjectState, env: QuotaEnv) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS objects (key TEXT PRIMARY KEY, size_bytes INTEGER NOT NULL, state TEXT NOT NULL, source_key TEXT)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS settings (name TEXT PRIMARY KEY, value TEXT NOT NULL)");
  }
  private setting(name: string): string | undefined {
    return this.ctx.storage.sql.exec<{ value: string }>("SELECT value FROM settings WHERE name=?", name).toArray()[0]?.value;
  }
  private set(name: string, value: string) {
    this.ctx.storage.sql.exec("INSERT OR REPLACE INTO settings VALUES (?, ?)", name, value);
  }
  private entry(key: string): Entry | undefined {
    return this.ctx.storage.sql.exec<Entry>("SELECT * FROM objects WHERE key=?", key).toArray()[0];
  }
  private usage() {
    return this.ctx.storage.sql.exec<{ used_bytes: number; object_count: number }>("SELECT COALESCE(SUM(size_bytes),0) AS used_bytes, COUNT(*) AS object_count FROM objects").one();
  }
  private reserve(key: string, size: number, source: string | null, ingress: boolean): boolean {
    return this.ctx.storage.transactionSync(() => {
      const policy = limits(this.env);
      const usage = this.usage();
      if (this.entry(key) || usage.used_bytes + size > policy.max_spool_bytes || usage.object_count + 1 > policy.max_spool_messages) return false;
      if (ingress) {
        const window = Math.floor(Date.now() / (policy.rate_window_s * 1000)).toString();
        const count = this.setting("rate_window") === window ? Number(this.setting("rate_count") ?? "0") : 0;
        if (count >= policy.max_messages_per_window) return false;
        this.set("rate_window", window);
        this.set("rate_count", String(count + 1));
      }
      this.ctx.storage.sql.exec("INSERT INTO objects VALUES (?, ?, 'reserved', ?)", key, size, source);
      return true;
    });
  }
  private committed(key: string) { this.ctx.storage.sql.exec("UPDATE objects SET state='stored' WHERE key=?", key); }
  private release(key: string) { this.ctx.storage.sql.exec("DELETE FROM objects WHERE key=?", key); }
  private async schedule() {
    if (await this.ctx.storage.getAlarm() === null) await this.ctx.storage.setAlarm(Date.now() + 60000);
  }
  private async initialize(): Promise<boolean> {
    if (this.setting("initialized") === "true") return true;
    // Adopt existing objects before new writes. One page per event bounds work;
    // an alarm continues large bucket scans while admission remains closed.
    return this.ctx.blockConcurrencyWhile(async () => {
      if (this.setting("initialized") === "true") return true;
      const page = await this.env.MAIL_SPOOL.list({ cursor: this.setting("initial_cursor"), limit: 1000 });
      this.ctx.storage.transactionSync(() => {
        for (const object of page.objects) this.ctx.storage.sql.exec("INSERT OR IGNORE INTO objects VALUES (?, ?, 'stored', NULL)", object.key, object.size);
        if (page.truncated) this.set("initial_cursor", page.cursor);
        else this.set("initialized", "true");
      });
      if (page.truncated) await this.schedule();
      return !page.truncated;
    });
  }
  async alarm(): Promise<void> {
    if (!await this.initialize()) return;
    const entries = this.ctx.storage.sql.exec<Entry>("SELECT * FROM objects WHERE key>? ORDER BY key LIMIT 50", this.setting("reconcile_cursor") ?? "").toArray();
    for (const entry of entries) {
      const object = await this.env.MAIL_SPOOL.head(entry.key);
      // A fence left by an interrupted settlement proves the original never landed.
      if (object?.customMetadata?.spool_fence === "true") { await this.env.MAIL_SPOOL.delete(entry.key); this.release(entry.key); }
      else if (object) {
        // A visible object proves a reserved write committed.
        this.ctx.storage.sql.exec("UPDATE objects SET state='stored', size_bytes=? WHERE key=?", object.size, entry.key);
      } else if (entry.state === "stored") this.release(entry.key);
      else if (!this.writing.has(entry.key)) await this.settleUncertain(entry.key);
    }
    this.set("reconcile_cursor", entries.length === 50 ? entries[entries.length - 1].key : "");
    if (this.usage().object_count) await this.schedule();
  }
  /**
   * Settle a reservation whose write ended without a confirmed outcome. Every
   * spool write is a conditional create, so a fence created at the same key
   * decides it: if the fence is refused the original landed and stays charged;
   * if the fence is created the original can no longer commit, so the fence is
   * removed and the reservation released.
   */
  private async settleUncertain(key: string) {
    const fence = await this.env.MAIL_SPOOL.put(key, new Uint8Array(), { onlyIf: { etagDoesNotMatch: "*" }, customMetadata: { spool_fence: "true" } });
    if (!fence) {
      const object = await this.env.MAIL_SPOOL.head(key);
      if (object) this.ctx.storage.sql.exec("UPDATE objects SET state='stored', size_bytes=? WHERE key=?", object.size, key);
      return;
    }
    await this.env.MAIL_SPOOL.delete(key);
    this.release(key);
  }
  /** Run one conditional spool write; the caller marked its key in flight. */
  private async write(key: string, value: ReadableStream | ArrayBuffer, options: R2PutOptions) {
    try { return await this.env.MAIL_SPOOL.put(key, value, options); }
    finally { this.writing.delete(key); }
  }
  private async put(request: Request): Promise<Response> {
    if (!request.body) return failure("invalid_message", 400);
    let raw: ArrayBuffer;
    try { raw = await readBounded(request.body, limits(this.env).max_message_bytes); }
    catch (error) { if (error instanceof LimitExceeded) return failure("message_too_large", 413); throw error; }
    const key = `inbox/${new Date().toISOString().replace(/[-:]/g, "").replace(/\.\d{3}Z$/, "Z")}-${crypto.randomUUID().replaceAll("-", "")}.eml`;
    const metadata = JSON.parse(decodeURIComponent(request.headers.get("X-Spool-Metadata") ?? "{}")) as Record<string, string>;
    if (!this.reserve(key, raw.byteLength, null, true)) return failure("spool_limit_exceeded", 429);
    this.writing.add(key);
    await this.schedule();
    try {
      const written = await this.write(key, raw, { onlyIf: { etagDoesNotMatch: "*" }, httpMetadata: { contentType: "message/rfc822" }, customMetadata: { ...metadata, quota_managed: "true" } });
      if (!written) { this.release(key); return failure("spool_key_conflict", 409); }
      this.committed(key);
      return result({ key, size_bytes: raw.byteLength });
    } catch { return failure("spool_write_outcome_unknown", 503); }
  }
  private archivePrefix(): string {
    const prefix = this.env.ARCHIVE_PREFIX ?? "imported/";
    if (!prefix.endsWith("/") || prefix.startsWith("inbox/") || "inbox/".startsWith(prefix) || prefix.split("/").some(part => part === "." || part === "..")) throw new Error("Invalid ARCHIVE_PREFIX");
    return prefix;
  }
  private async acknowledge(request: Request): Promise<Response> {
    if (!request.body) return failure("invalid_request", 400);
    const body = JSON.parse(new TextDecoder().decode(await readBounded(request.body, 4096))) as { key?: unknown; archive_key?: unknown };
    const key = body.key;
    const validKey = typeof key === "string" && ["inbox/", this.archivePrefix()].some(prefix => key.startsWith(prefix) && /^[A-Za-z0-9._-]+\.eml$/.test(key.slice(prefix.length)));
    if (!validKey || typeof key !== "string") return failure("invalid_key", 400);
    if (this.entry(key)?.state === "reserved") return failure("spool_operation_pending", 409);
    if (body.archive_key !== undefined) {
      const target = body.archive_key;
      if (!key.startsWith("inbox/") || typeof target !== "string" || target !== this.archivePrefix() + key.slice("inbox/".length)) return failure("invalid_archive_key", 400);
      const prior = this.entry(target);
      if (prior && prior.source_key !== key) return failure("archive_conflict", 409);
      if (prior && await this.env.MAIL_SPOOL.head(target)) this.committed(target);
      else {
        // A confirmed copy that vanished is released; an uncertain one keeps its
        // reservation and is written again. The conditional put never overwrites.
        if (prior?.state === "stored") this.release(target);
        const object = await this.env.MAIL_SPOOL.get(key);
        if (!object) return failure("source_missing", 409);
        if (!this.entry(target) && !this.reserve(target, object.size, key, false)) return failure("spool_limit_exceeded", 429);
        this.writing.add(target);
        await this.schedule();
        try {
          const written = await this.write(target, object.body, { onlyIf: { etagDoesNotMatch: "*" }, httpMetadata: object.httpMetadata, customMetadata: object.customMetadata });
          if (!written) return failure("archive_conflict", 409);
          this.committed(target);
        } catch { return failure("spool_write_outcome_unknown", 503); }
      }
    }
    try { await this.env.MAIL_SPOOL.delete(key); this.release(key); }
    catch { return failure("spool_delete_outcome_unknown", 503); }
    return result({ acknowledged: true });
  }
  async fetch(request: Request): Promise<Response> {
    try {
      limits(this.env);
      if (!await this.initialize()) return failure("spool_initializing", 503);
      const path = new URL(request.url).pathname;
      if (request.method === "POST" && path === "/put") return await this.put(request);
      if (request.method === "POST" && path === "/v1/ack") return await this.acknowledge(request);
      if (request.method === "GET" && path === "/v1/quota") return result({ ...this.usage(), ...limits(this.env) });
      return failure("not_found", 404);
    } catch { return failure("spool_operation_failed", 503); }
  }
}
