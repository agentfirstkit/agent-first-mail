# afmail HTTP API behavior

Read this reference only when the user explicitly asks for an API, an
application integration, LAN access, or access through a secure tunnel.

## Start and discovery

- Prefer `afmail api serve --mode local`; it binds only loopback.
- Require the user/operator to provision a random 32–512-character bearer-safe
  ASCII credential through `AFMAIL_API_ACCESS_TOKEN_SECRET`. Do not
  invent, print, paste into a URL, log, or commit the credential. The explicit
  CLI secret flag is only a fallback because argv can be inspected.
- Wait for the `api_ready` progress event, show `api_url` and `openapi_url` as
  clickable links, and continue waiting for the server process. The event does
  not contain the credential. `api serve` is an ordered event stream, so read
  every event from stdout and branch on `kind`.
- Structural discovery (`/health`, `/openapi.json`, `/schemas/index.json`, and
  `/schemas/{schema_file}`) is public. Every `/v1` call requires a Bearer header.
- Use the served OpenAPI document rather than reconstructing paths or request
  bodies from this skill.

## Network choice

- Use `--mode lan` only when the user asks to access afmail from another device
  on the same trusted IPv4 local network. Explain that LAN mode is plaintext
  HTTP and the bearer credential must remain private.
- For arbitrary-network access, keep `--mode local` and use an established
  encrypted tunnel such as WireGuard, Tailscale/NetBird, or SSH forwarding, or
  an authenticated TLS reverse proxy. Do not expose LAN mode to the public
  Internet and do not invent a new transport protocol.
- The API sends no CORS allowlist by default. Do not weaken it to wildcard CORS;
  use a trusted same-origin Host/proxy when a browser application needs access.

## Mutation and authorization rules

- A bearer credential authenticates a client; it does not authorize remote
  reads, remote effects, permanent deletion, or persistent notes. Preserve the
  main skill's user-intent boundaries.
- Supply a stable `Idempotency-Key` for every local mutation that the OpenAPI
  document marks with that required header. Reuse it only for an exact retry of
  the same operation and input.
- Read the current object and use its object-specific review base hash before
  setting an Agent suggestion. Treat conflicts as stale state; re-read and
  reconsider rather than forcing the write.
- Preview remote effects with `GET /v1/push/plan`, show the complete plan to the
  user, and call `POST /v1/push/confirm` only after the user authorizes that
  exact `plan_id`. Never treat bearer authentication as plan approval.
- Human review is not an API operation. When the user says “review,” use the
  afmail-owned `triage review`, `case review`, `notification review`, or
  `case draft review` UI even if an API server is already running.
