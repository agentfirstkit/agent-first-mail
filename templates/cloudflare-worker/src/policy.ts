export interface PolicyEnv {
  MAX_MESSAGE_BYTES?: string;
  MAX_SPOOL_BYTES?: string;
  MAX_SPOOL_MESSAGES?: string;
  RATE_WINDOW_S?: string;
  MAX_MESSAGES_PER_WINDOW?: string;
  ALLOWED_RECIPIENTS?: string;
  ALLOWED_SENDER_DOMAINS?: string;
  REQUIRE_DMARC_PASS?: string;
  SENDER_VERIFIER?: Fetcher;
  ARCHIVE_PREFIX?: string;
}

function positive(value: string | undefined, fallback: number): number {
  const parsed = value === undefined ? fallback : Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error("Invalid positive integer limit");
  return parsed;
}
export function limits(env: PolicyEnv) {
  const result = {
    max_message_bytes: positive(env.MAX_MESSAGE_BYTES, 10 * 1024 * 1024),
    max_spool_bytes: positive(env.MAX_SPOOL_BYTES, 1024 * 1024 * 1024),
    max_spool_messages: positive(env.MAX_SPOOL_MESSAGES, 10000),
    rate_window_s: positive(env.RATE_WINDOW_S, 60),
    max_messages_per_window: positive(env.MAX_MESSAGES_PER_WINDOW, 100),
  };
  if (result.max_message_bytes > 25 * 1024 * 1024) throw new Error("MAX_MESSAGE_BYTES exceeds inbound platform limit");
  return result;
}
export class LimitExceeded extends Error {}
export async function readBounded(stream: ReadableStream<Uint8Array>, maxBytes: number): Promise<ArrayBuffer> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > maxBytes) { await reader.cancel(); throw new LimitExceeded("message_too_large"); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  const raw = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { raw.set(chunk, offset); offset += chunk.byteLength; }
  return raw.buffer;
}
function list(value: string | undefined): string[] {
  return value?.split(",").map(item => item.trim().toLowerCase()).filter(Boolean) ?? [];
}
export async function authenticate(message: ForwardableEmailMessage, raw: ArrayBuffer, env: PolicyEnv): Promise<boolean> {
  const recipients = list(env.ALLOWED_RECIPIENTS);
  if (recipients.length && !recipients.includes(message.to.toLowerCase())) return false;
  if (env.REQUIRE_DMARC_PASS !== undefined && !["true", "false"].includes(env.REQUIRE_DMARC_PASS)) throw new Error("REQUIRE_DMARC_PASS must be true or false");
  const domains = list(env.ALLOWED_SENDER_DOMAINS);
  if (!domains.length && env.REQUIRE_DMARC_PASS !== "true") return true;
  // Sender-controlled headers never establish a trusted authentication result.
  // This service must independently verify the message and its aligned author.
  if (!env.SENDER_VERIFIER) throw new Error("Sender policy requires the trusted SENDER_VERIFIER service binding");
  const response = await env.SENDER_VERIFIER.fetch("https://verifier.invalid/verify", {
    method: "POST", body: raw,
    headers: { "Content-Type": "message/rfc822", "X-Envelope-From": message.from, "X-Envelope-To": message.to },
  });
  if (!response.ok || !response.body) throw new Error("Sender verifier unavailable");
  const result = JSON.parse(new TextDecoder().decode(await readBounded(response.body, 4096))) as { authenticated_domain?: unknown; aligned?: unknown; dmarc?: unknown };
  if (result.aligned !== true || typeof result.authenticated_domain !== "string") return false;
  if (env.REQUIRE_DMARC_PASS === "true" && result.dmarc !== "pass") return false;
  return !domains.length || domains.includes(result.authenticated_domain.toLowerCase());
}
export function result(value: object): Response {
  return Response.json({ kind: "result", result: value, trace: {} });
}
export function failure(code: string, status: number): Response {
  return Response.json({ kind: "error", error: { code, message: code, retryable: status >= 500 }, trace: {} }, { status });
}
