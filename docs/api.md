# HTTP API

afmail provides a typed local HTTP API for Agent runtimes, stock applications,
and other trusted clients. The API uses OpenAPI 3.2, JSON Schema 2020-12, and
the same workspace locks, transaction guards, idempotency records, audit paths,
and push plan confirmation as the CLI. It is not a second mailbox state model.

## Start the API

Set a random credential containing 32–512 bearer-safe ASCII characters (letters,
digits, `-._~+/=`), then start the server from an afmail workspace:

```bash
export AFMAIL_API_ACCESS_TOKEN_SECRET='replace-with-a-random-32-character-value'
afmail api serve --mode local
```

`local` is the default and binds only `127.0.0.1`. The server emits an AFDATA
`api_ready` progress event containing `api_url`, `openapi_url`, the standalone
Schema index URL, and the actual port. It never emits the bearer credential.
The process runs until interrupted.

`api serve` reports that address before it can report an outcome, so it is an
ordered event stream: every event goes to stdout, and `--output-to split` is
refused. Read one stream and branch on `kind`.

For another device on the same trusted IPv4 local network:

```bash
afmail api serve --mode lan
```

LAN mode binds all local interfaces and advertises the primary LAN address. It
uses the same LAN route probe as `review --mode link`. The probe targets the
IPv4 all-hosts multicast route rather than the default unicast route, so a VPN
or overlay such as Tailscale can own the default route without displacing the
physical LAN address afmail advertises. LAN mode uses plaintext HTTP and is
only for a trusted network. For access across an
arbitrary network, keep afmail in `local` mode and use an established encrypted
tunnel such as WireGuard, Tailscale/NetBird, or SSH port forwarding, or place it
behind a TLS reverse proxy with its own authentication. Do not expose LAN mode
directly to the public Internet.

## Discovery and authentication

These structural endpoints contain no mailbox data and are public:

- `GET /health`
- `GET /openapi.json`
- `GET /schemas/index.json`
- `GET /schemas/{schema_file}`

Every `/v1` endpoint requires:

```text
Authorization: Bearer <AFMAIL_API_ACCESS_TOKEN_SECRET>
```

Credentials in a query string are rejected. CORS is disabled by default; a
browser application should use an explicitly trusted same-origin proxy rather
than a wildcard origin.

For example:

```bash
curl -sS \
  -H "Authorization: Bearer $AFMAIL_API_ACCESS_TOKEN_SECRET" \
  http://127.0.0.1:9417/v1/status
```

Responses use AFDATA result/error envelopes. Mail body, header, and attachment
content returned by the API remains untrusted remote data.

HTTP status and the structured `error.code` are both significant. In
particular, the API preserves `405` for an unsupported method, `413` for a JSON
body over the 8 MiB limit, and `415` when a body endpoint does not receive
`application/json`; those failures use the same redacted AFDATA error envelope.
JSON, query and path extractor failures use fixed messages and a static
`error.details.kind`, without echoing rejected values or serde diagnostics.

## Operations

The served OpenAPI document is the complete operation reference. Its first
slice covers:

- workspace status and pull;
- triage and local message reads;
- typed message dispositions and Agent suggestions;
- case and notification collection reads, creation, actions, membership, and
  Agent suggestions;
- case draft creation, read, patch, validation, queueing, and removal;
- remote-effect push preview and exact plan confirmation.

The API deliberately has no arbitrary command, argv, filesystem-path write, or
shell endpoint. Request bodies are typed closed objects. Draft bodies are inline;
the HTTP API does not accept CLI `--body-file` paths.

## Idempotency and conflicts

Every non-naturally-idempotent local mutation requires a stable caller-generated
header:

```text
Idempotency-Key: <8-to-200-character-path-safe-id>
```

Reuse the key only for an exact retry of the same operation and input. afmail
persists the request and result in the workspace's machine state. Reusing a key
with changed input returns a conflict.

Case creation performs its read-only preparation before writing a new pending
record. If that preparation fails, no key has been consumed and corrected input
can use the same key. Existing completed, conflicting and incomplete records
are handled before preparation: a completed retry still returns its cached
result even if the current configuration or source message has changed. Once
the local operation starts, a failure keeps the pending record so an uncertain
effect cannot be repeated blindly.

Suggestion writes also require the current object-specific review base hash.
Draft patch distinguishes omission from clearing: omitted fields are unchanged,
an empty `cc` list clears Cc, and an empty `to` list is rejected.

Pull is a repeat-safe remote read/local reconciliation and does not use an
idempotency header. A Cloudflare pull also deletes or archives the R2 spool
objects it imported (see [Authorization](authorization.md)). Remote push
confirmation instead uses the existing, workspace-bound `plan_id`:

1. `GET /v1/push/plan` returns the complete resolved plan.
2. The caller presents that plan to the user.
3. `POST /v1/push/confirm` sends `{"plan_id":"..."}`.

Any relevant queue, draft, attachment, identity, configuration, or workspace
change invalidates the old plan before remote access.

## Review remains a UI action

OpenAPI exposes canonical suggestions and review hashes, but it does not turn
human review into a generic HTTP command. When a user asks to review mail, use
the afmail-owned interactive UI commands (`triage review`, `case review`,
`notification review`, or `case draft review`). Review persists typed human
feedback/local edits under afmail's existing rules and never authorizes push.

## Exported contract

The release package includes generated files under `openapi/`. Regenerate them
from the Rust DTO source with:

```bash
afmail api export --directory openapi --force
```

The test suite fails if committed OpenAPI or standalone Schemas drift from the
Rust source.
