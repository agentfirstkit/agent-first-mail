import { authenticate, failure, limits, LimitExceeded, readBounded } from "./policy";
import type { QuotaEnv } from "./quota";
export { SpoolQuota } from "./quota";

export interface Env extends QuotaEnv {
  MAIL_QUOTA: DurableObjectNamespace;
  SPOOL_CONTROL_TOKEN_SECRET?: string;
  FORWARD_TO?: string;
}
function quota(env: Env): DurableObjectStub {
  return env.MAIL_QUOTA.get(env.MAIL_QUOTA.idFromName("spool"));
}
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const token = env.SPOOL_CONTROL_TOKEN_SECRET;
    if (!token || token.length < 32) return failure("control_unavailable", 503);
    const digest = async (value: string) => new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value)));
    const [left, right] = await Promise.all([digest(request.headers.get("Authorization") ?? ""), digest(`Bearer ${token}`)]);
    let difference = 0;
    for (let i = 0; i < left.length; i++) difference |= left[i] ^ right[i];
    if (difference !== 0) return failure("unauthorized", 401);
    const path = new URL(request.url).pathname;
    if (!((request.method === "POST" && path === "/v1/ack") || (request.method === "GET" && path === "/v1/quota"))) return failure("not_found", 404);
    return quota(env).fetch(request);
  },
  async email(message: ForwardableEmailMessage, env: Env): Promise<void> {
    const max = limits(env).max_message_bytes;
    if (message.rawSize > max) { message.setReject("Message exceeds configured size limit"); return; }
    let raw: ArrayBuffer;
    try { raw = await readBounded(message.raw, max); }
    catch (error) {
      if (!(error instanceof LimitExceeded)) throw error;
      message.setReject("Message exceeds configured size limit"); return;
    }
    if (!await authenticate(message, raw, env)) { message.setReject("Message rejected by configured sender or recipient policy"); return; }
    const response = await quota(env).fetch("https://spool.invalid/put", {
      method: "POST", body: raw,
      headers: { "X-Spool-Metadata": encodeURIComponent(JSON.stringify({ envelope_from: message.from, envelope_to: message.to, received_rfc3339: new Date().toISOString() })) },
    });
    if (response.status === 429 || response.status === 413) { message.setReject("Mail spool limit reached"); return; }
    if (!response.ok) throw new Error("Mail spool storage did not confirm acceptance");
    if (env.FORWARD_TO) {
      try { await message.forward(env.FORWARD_TO); }
      catch { console.warn("Optional email forwarding failed after durable storage"); }
    }
  },
};
