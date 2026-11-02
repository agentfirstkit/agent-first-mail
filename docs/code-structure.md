# Code Structure

This document maps the `agent-first-mail` crate after the module split. It is a
maintenance guide for contributors; user-facing mailbox behavior is documented in
`workspace.md`, `file-formats.md`, and `cli.md`.

## Top-Level Flow

`afmail` has four main layers:

1. `src/main.rs` binds the registry's actions, resolves argv, installs the
   resolved output plan, and owns process exit codes.
2. `src/cli/` holds the closed `cli-spec-v1` registry (`spec.rs`), the resolved
   command types, and the projection from a resolved invocation onto a
   `Command` (`build.rs`).
3. `src/runner/` dispatches parsed commands to workspace operations and owns
   structured command logging/progress output.
4. `src/api/` defines HTTP DTOs, OpenAPI/JSON Schema generation, bearer/network
   serving, and thin handlers that enter the same runner dispatch as CLI calls.

Most behavior then lands in `src/store/Workspace`, which owns the file-first
mailbox workspace. Network-facing and channel I/O stays outside the store in
`sources/`, `channels/`, `imap_client.rs`, `outbound.rs`, `senders/`,
`remote.rs`, and `push_queue/`.

## Source Map

```text
src/
  main.rs            registry binding, argv resolution, and exit handling
  cli/               cli-spec-v1 registry, command model, and invocation projection
  runner/            command dispatch, output, locks, push/purge, and blocking review sessions
  api/               HTTP handlers, typed DTOs, OpenAPI/Schema generation and export
  config/            typed config schema, defaults, key access, and validation
  store/             workspace file model and local state transitions
  ingest/            shared content identity, deduplication, storage, and delivery merging
  sources/imap/      IMAP sessions, UID cursors, flags, and special-use folders
  sources/cloudflare/ R2 S3 client, object ingestion, Queue consumption, durable agent tasks, and Worker scaffold
  channels/          native JSON file adapter: bounded inbox reads and atomic outbox publication
  imap_client.rs     low-level IMAP session, mailbox info, and move outcomes
  outbound.rs        transport-independent MIME construction and local sent evidence
  senders/           feature-gated SMTP and Cloudflare REST submission
  remote.rs          MailRemote trait over IMAP/SMTP side effects, plus the test fake
  push_queue/        durable local push items plus preview/execute helpers
  types/             stable ids, message/case/archive/push DTOs
  types/command_result.rs item counts, terminal-event selection, and process-exit mapping
  mail.rs            MIME parsing into message DTOs
  markdown.rs        Markdown frontmatter split/parse/render and conversation extraction
  frontmatter.rs     draft/triage/case-message frontmatter types
  draft_content.rs   typed compose/quote boundaries, completeness, and outbound projection
  render_document.rs versioned output-neutral documents shared by Markdown and Review
  templates.rs       shared Markdown/HTML MiniJinja loading, filters, and Review safety checks
  review_frontend.rs discovery, version, and trust of a user's own Review UI
  skill_admin.rs     skill install/status backing `afmail skill`
  progress.rs        workspace progress snapshot sink and status projection
  error.rs           AppError type with structured error codes and hints
  util.rs            crate-wide atomic-write, hashing, and flag helpers
  layout.rs          workspace directory and file name constants
  demo/              demo workspace and server
  workspace_identity.rs  immutable workspace UID file
  workspace_lock.rs      external runtime lock-file guard
  plan_id.rs             canonical workspace-bound plan digest
  sources/consumer_shutdown.rs CLI-owned consumer signal cancellation and idle waits
```

## Store Modules

`store/mod.rs` keeps the `Workspace` type, workspace discovery/init/status, and
high-level wiring. Sibling modules add focused `impl Workspace` blocks:

- `notification.rs` handles notification collections and their generated views.
- `cases.rs` handles active case CRUD, membership, notes, tags, and moves.
- `contacts.rs` handles contact-card CRUD, emails/phones/tags/notes, deletion,
  sender extraction, the ephemeral email→contact map, and the view refresh that
  materializes the contact link onto messages.
- `messages.rs` handles explicit message cache materialization, pure in-memory reads, local status
  transitions, attachments, related-message discovery, relocation, and stamping
  each message's materialized `contact` link.
- `refs.rs` builds the case->message reference index (`CaseIndex`) used to decide
  whether a message is still referenced by any active or archived case.
- `disposition_views.rs` handles canonical local spam/trash/deleted collections
  plus generated review views.
- `remote_sync.rs` owns the `pull` entrypoint (`Workspace::pull` /
  `pull_with_progress`, dispatched for `afmail pull`), reconciles local references
  with remote IMAP locations, and exposes remote-location helpers used by push
  execution. It selects IMAP, Cloudflare, and file sources; their I/O lives in
  `sources/imap/`, `sources/cloudflare/`, and `channels/files.rs`.
- `triage.rs` and `render.rs` build generated Markdown read views and shared
  message rendering context.
- Relationship reads use complete current caches, consulting native evidence
  only for legacy caches. Unreadable unrelated records become shared typed
  diagnostics; disposition guards require a complete index.
- `drafts.rs` owns draft creation, queue validation, and attachment bookkeeping
  for cases. `draft_content.rs` owns the body schema and authored/outbound
  projections; `types/draft.rs` owns intent/status/completeness enums shared by
  CLI output and typed Review documents.
- `readiness.rs` derives `status.readiness` from configuration alone.
- `push_state.rs`, `purge.rs`, `doctor.rs`, `transactions.rs`, `migration.rs`,
  `idempotency.rs`, `render_cache.rs`, `storage_usage.rs`, and `suggestions.rs`
  own narrower maintenance concerns.
- `util.rs` contains store-private filesystem, id, path, timestamp, and audit
  helpers shared by the modules above.

Keep new store behavior near the durable state it changes. Prefer another small
module only when a concern has its own file format, lifecycle, or command group.

`runner/review.rs` owns the review domain for triage, cases, notifications, and
drafts. It contains the same typed render documents used by Markdown, renders
the page through AFUI's `UiPage` — one engine for both the doctor's check and
the live session, so a template cannot pass one and fail the other — while
holding only a shared lock, then returns one AFDATA result read back from the
workspace once the session ends.
There is no batch and no page-wide submit: each item — a triage message, or
the one case/notification a case/notification review is about — settles the
moment a person decides it, writing typed, hash-bound human suggestion state
immediately, but never applying the suggested domain action itself. A stale
`agent_suggestion` is marked in place rather than refusing to open the whole
review; only accepting or changing it is refused, at settle time. Case-note,
notification-note, and draft edits debounce into automatic saves that acquire
an exclusive lock, verify the base hash, and call the canonical Workspace API;
the result lists each completed write in `saved_files`. Review never queues,
pushes, or sends remote effects, and ending a session leaves every settlement
and saved local change exactly as it was written.

`runner/review/host.rs` is the only place that domain meets `agent-first-ui`.
AFUI owns delivery — the listener, the credential in the URL path, attention
and warning-grace enforcement, the security headers and CSP nonce, the exact-`Host` and
`Origin` checks, the isolated browser window on a disposable profile, and
shutdown — so afmail hosts nothing itself. `--mode link` and `afui session
serve` use the same AFUI attention policy (15 unattended minutes plus 5
minutes of warning grace by default), renewed by visible-page attention or
framed UI traffic. afmail neither receives a lifetime setting nor constructs
an AFUI expiry type. The host module contributes only the server-rendered
`GET /` page and attaches AFUI's typed runtime with managed blobs. Notes,
draft saves, settlement, refresh, image loading, and the Agent turn are typed
Review actions; afmail defines their domain payloads while AFUI owns the
envelope, correlation, reconnect, progress/result/error completion, retained
state, and temporary byte URLs. A user message becomes a `review_input` AFDATA
progress event; one strict `review_response` JSON line on the still-running
command's stdin completes the one waiting AFUI call. With `refresh:true`, AFUI
publishes retained changed state; the page requests a freshly rendered HTML
blob, whose render first rereads the exact surface from canonical workspace
documents, so controller text cannot inject a draft or suggestion. The host
then builds the terminal result by rereading the workspace
regardless of how the session ended: `closed` means the window closed (or,
under `--mode link`/`--mode session`, the session's own end route was hit) —
the ordinary, unremarkable end of every review, not a decision — and
`expired` means the AFUI-owned Link page reached the end of its unattended
warning grace. The session's type parameter to
AFUI is `()`: nothing is ever handed back through completion, so
`Outcome::Completed` is unreachable here. Saved files and the
settled/unsettled summary are reported the same way for both outcomes,
because the session's state outlives the session.

The old Review-specific HTML/CSS/DOM-rendering bundle does not exist. Review
templates own server-rendered structure, style, and localized copy;
`runner/review_runtime.js` is an afmail-owned behavior-only runtime. It binds
autosave/settlement controls to `afui.connect`, keeps the built-in UI's visible
turn history, loads attachment/page bytes through AFUI-managed blobs, and
replaces only the declared Review content subtree after a canonical refresh.
Review templates cannot supply JavaScript; the runtime clones
template-declared turn markup rather than constructing page structure or
carrying user-facing copy.

`review_frontend.rs` is where a person's own Review UI comes from.
`agent-first-ui` owns the location, the lookup order, safe mode, and the trust
gate; this module names afmail's provider id, the `ui_kind` per surface, and
the Review UI API version afmail implements, and turns a frontend that cannot
be loaded into an explicit error rather than a quiet built-in page. Only Review
UI templates come from there — content templates stay in the workspace
`templates/` tree, and `templates.rs` refuses to serve either one from the
other's tree.

`api/server.rs` owns the Axum boundary but not domain behavior. Handlers reject
malformed typed JSON, authenticate, obtain any required idempotency key, convert
the DTO to an existing `Command`, and call `runner::dispatch::execute_command_at`.
That path applies the normal workspace lock and incomplete-transaction guard.
`api/schema.rs` generates both OpenAPI components and standalone JSON Schemas
from the DTOs; committed files under `openapi/` have a drift test.

## File-State Boundaries

The workspace keeps durable state in a few canonical places:

- Raw message evidence and canonical message metadata live under
  `.afmail/messages/`; metadata includes remote state and current suggestion
  annotations.
- Rebuildable parsed message caches live under `messages/`.
- Case state lives in `cases/.../data/case.json` or
  `archived-cases/.../data/case.json`.
- Direct archive state lives in
  `notifications/.../data/notification.json`.
- Local discard state lives in `spam/data/spam.json`,
  `trash/data/trash.json`, and `deleted/data/deleted.json` after the first
  message enters that disposition.
- Push items live in `.afmail/push/`; audit events live in
  `.afmail/logs/events.jsonl`.
- Immutable workspace identity lives in `.afmail/workspace.json`; process locks
  deliberately live outside the workspace.

`push_queue/plan.rs` is the pure `Plan` boundary: it resolves queue items,
draft/MIME/attachment bytes, action steps, and content hashes without writing.
Only a matching `--confirm PLAN_ID` enters exclusive reconcile/apply; audit-only
recovery remains explicitly `effect_scope: local_recovery`.

Generated Markdown views (`triage/`, `case.md`, `notification.md`, `views/`,
`spam/*.md`, `trash/*.md`, and `deleted/*.md`) should be rebuildable from those
canonical files with `afmail render refresh`.

## Refactor Rules

- Preserve public command names, JSON schemas, stdout event codes, and on-disk
  paths unless a migration is explicitly planned.
- Add new CLI actions to the registry in `src/cli/spec.rs` first, give them an
  arm in `src/cli/build.rs`, then dispatch them in `src/runner/`, and keep
  workspace mutations on `Workspace` methods. Express a constraint as disjoint
  combinations rather than as a check in a handler.
- Use typed structs at module boundaries and for template documents; reserve
  `serde_json::Value` for genuinely dynamic CLI output.
- Keep generated view code separate from durable state mutation code where
  possible, then call refresh helpers after state changes.
- Keep helpers `pub(super)` or narrower unless another crate module already
  relies on the path.

## Validation Loop

Run every validation through the repository-owned project entry point from the
repository root:

```bash
./scripts/test.sh static
./scripts/test.sh unit
```

`static` covers formatting, Clippy for the default set, the empty feature set,
and each standalone `api`, `demo`, `review`, `cloudflare`, `files`, and `smtp` feature, plus generated CLI
documentation drift, manifest JSON validity, and a dependency-tree assertion
that `axum`, `tokio`, `ureq`, `rusty-s3`, and SMTP transport are absent when
default features are disabled. It also checks the live-test script's argument
parser without touching cloud resources. `lib [FILTER]` runs focused library
tests. `unit`
runs all-feature tests; `minimal` runs the same crate with no default
features. `all` runs those gates plus the Worker gate. Do not replace these entry points with
hand-written Cargo command sequences.

The default feature set is `api,demo,review,cloudflare,smtp,files`. IMAP remains part of
the core because remote reconciliation and the push queue depend on it. SMTP
transport is optional; the core retains only lettre's MIME builder. Cloudflare
receiving and sending share the `cloudflare` feature; file channels use `files`.
Disabled implementations are not compiled, but their command shapes remain
discoverable and return a structured `feature_disabled` error. A push plan with a
send step whose feature is not compiled in is rejected with `feature_disabled` at
preview time, before any remote effect. Keep the small fallback modules free of the optional
dependencies so `--no-default-features` remains a real capability boundary.

Use focused modes for large integration targets instead of ad hoc Cargo command
lines:

```bash
./scripts/test.sh cli [FILTER]
./scripts/test.sh request-id [FILTER]
./scripts/test.sh workspace [FILTER]
./scripts/test.sh config [FILTER]
./scripts/test.sh surface [FILTER]
./scripts/test.sh draft [FILTER]
./scripts/test.sh push [FILTER]
```

`request-id` runs the split-out request-id retry/audit integration target
(`tests/request_id_field_edits.rs`). `workspace` runs the split-out workspace
identity integration target (`tests/workspace_identity.rs`). `config` runs the
split-out config and dependency contract target (`tests/config_contract.rs`).
`surface` runs the split-out CLI surface contract target
(`tests/cli_surface_contract.rs`). `draft` runs the split-out draft contract
target (`tests/draft_contract.rs`). `push` runs the split-out push contract
target (`tests/push_contract.rs`).

## Fixture Batch, Container E2E, and GreenMail Workflow

The reusable mailbox story lives under `tests/fixtures/mail-batch/`: a
`manifest.json` plus 30 realistic `.eml` files. Tests should load that manifest
instead of inventing new one-off EML strings when they need a realistic inbox.

Run all ignored GreenMail/Dovecot container tests through the fixed E2E mode, or
pass one Rust test-name filter as the second argument:

```bash
./scripts/test.sh e2e
./scripts/test.sh e2e container_greenmail_pull_reply_send_e2e
```

The same fixture batch powers a real agent-operation workspace. This prepares a
live GreenMail mailbox and configured afmail workspace, but does not pull or
process mail; the agent must actually operate the inbox:

```bash
./scripts/test.sh greenmail-prepare
```

After it prints the workspace path, open your agent there and say:

```text
Check mail.
```

The prepared workspace uses `./bin/afmail`, so it works without a global
install. To install the current source globally for demo commands outside the
workspace:

```bash
cargo install --path . --bin afmail --locked --force
```

The prepared workspace also includes a small mock CRM (`./bin/crm`). The
`prepare` output suggests short demo prompts:

```text
Clean spam.
Check the refund request.
refund this order, reply.
```

The CRM helper is intentionally optional context for customer/order cases, not a
scripted requirement; the agent can use it when the email thread mentions an
order and more context would help. The demo-owned `AGENTS.md` only receives this
CRM lookup hint; normal mailbox behavior comes from the installed afmail skill,
while `afmail init` deliberately does not create or edit `AGENTS.md`.

For an automated scripted demo pass, use:

```bash
./scripts/test.sh greenmail
```

## Cloudflare Worker validation

`scripts/test.sh worker` generates the embedded project in a temporary directory,
installs its lockfile, checks TypeScript, runs the Workers runtime tests, and
runs both Wrangler and cf deployment dry-runs. Build outputs and dependencies
never enter the embedded template. This gate is included in all, release, and
full checks and requires Node.js 24+.

The MinIO container e2e uses a local image built from the fixed official
`RELEASE.2025-10-15T17-29-55Z` source because the original public image is no
longer available. The fixed e2e script builds that image on first use, using
Go 1.24.8 and a scratch Dockerfile; subsequent runs reuse it. It verifies real
S3 signatures, delivery metadata, exact bytes, and CopyObject with encoded keys.

## Event-driven receipt

`cloudflare consume` runs one Queue tick; `cloudflare watch` supervises ticks and
periodic R2 scans without a model. `sources/cloudflare/queue.rs` owns the HTTP
pull/ack wire boundary, `journal.rs` owns import receipts and durable notification
intent, and `consumer.rs` owns replay, cleanup, and handler dispatch outside the
workspace lock. The process lock still excludes overlapping consumer ticks.
See [the design and deployment guide](cloudflare-consumer.md).
