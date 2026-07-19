# Code Structure

This document maps the `agent-first-mail` crate after the module split. It is a
maintenance guide for contributors; user-facing mailbox behavior is documented in
`workspace.md`, `file-formats.md`, and `cli.md`.

## Top-Level Flow

`afmail` has three main layers:

1. `src/main.rs` handles early CLI bootstrapping: version/help output, parse
   errors, output-format recovery, and process exit codes.
2. `src/cli/` defines the Clap contract and parse-time conveniences such as
   command-specific error hints.
3. `src/runner/` dispatches parsed commands to workspace operations and owns
   structured command logging/progress output.

Most behavior then lands in `src/store/Workspace`, which owns the file-first
mailbox workspace. Network-facing code stays outside the store in `imap_pull/`,
`imap_client.rs`, `smtp_send.rs`, `remote.rs`, and `push_queue/`.

## Source Map

```text
src/
  main.rs            early CLI bootstrap and exit handling
  cli/               Clap argument model, parsing, and invalid-command hints
  runner/            command dispatch, output, locks, push/purge, and trusted AFUI owner bridge
  config/            typed config schema, defaults, key access, and validation
  store/             workspace file model and local state transitions
  imap_pull/         read-only IMAP pull sessions, remote identity, special-use folders
  imap_client.rs     low-level IMAP session, mailbox info, and move outcomes
  smtp_send.rs       outbound message construction and SMTP sending
  remote.rs          MailRemote trait over IMAP/SMTP side effects, plus the test fake
  push_queue/        durable local push items plus preview/execute helpers
  types/             stable ids, message/case/archive/push DTOs
  types/command_result.rs item counts, terminal-event selection, and process-exit mapping
  mail.rs            MIME parsing into message DTOs
  markdown.rs        Markdown frontmatter split/parse/render and conversation extraction
  frontmatter.rs     draft/triage/case-message frontmatter types
  draft_content.rs   typed compose/quote boundaries, completeness, and outbound projection
  templates.rs       built-in and workspace MiniJinja template loading
  skill_admin.rs     skill install/status backing `afmail skill`
  progress.rs        workspace progress snapshot sink and status projection
  error.rs           AppError type with structured error codes and hints
  util.rs            crate-wide atomic-write, hashing, and flag helpers
  workspace_identity.rs  immutable workspace UID file
  workspace_lock.rs      external runtime lock-file guard
  plan_id.rs             canonical workspace-bound plan digest
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
  execution. The IMAP I/O it drives lives in `imap_pull/`.
- `triage.rs`, `render.rs`, and `view_model.rs` build generated Markdown read
  views and shared message rendering context.
- `drafts.rs` owns draft creation, queue validation, and attachment bookkeeping
  for cases. `draft_content.rs` owns the body schema and authored/outbound
  projections; `types/draft.rs` owns intent/status/completeness enums shared by
  CLI and AFUI.
- `push_state.rs`, `purge.rs`, `doctor.rs`, and `transactions.rs` own narrower
  maintenance concerns.
- `util.rs` contains store-private filesystem, id, path, timestamp, and audit
  helpers shared by the modules above.

Keep new store behavior near the durable state it changes. Prefer another small
module only when a concern has its own file format, lifecycle, or command group.

`runner/ui.rs` launches the generic AFUI Host with afmail's pack, default termail
companion, and trusted sidecar configuration. `runner/ui_bridge.rs` is the JSONL
owner process: it accepts only afmail's declared local-mutation draft contract,
takes the workspace lock, calls `Workspace::change_draft`, and returns a fresh
producer snapshot. It is transport glue, not a second draft implementation.

## File-State Boundaries

The workspace keeps durable state in a few canonical places:

- Raw message evidence and remote sidecars live under `.afmail/messages/`.
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
- Add new CLI actions in `src/cli/` first, then dispatch them in `src/runner/`,
  and keep workspace mutations on `Workspace` methods.
- Use typed structs at module boundaries; reserve `serde_json::Value` for dynamic
  CLI output and template contexts.
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

`static` covers formatting, default and no-default-feature Clippy, generated CLI
documentation drift, and manifest JSON validity. `unit` runs default-feature
tests; `minimal` runs the same crate without the optional UI feature. `all` runs
all three. Do not replace these entry points with hand-written Cargo command
sequences.

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

## Fixture Batch, Docker E2E, and Demo

The reusable mailbox story lives under `tests/fixtures/mail-batch/`: a
`manifest.json` plus 30 realistic `.eml` files. Tests should load that manifest
instead of inventing new one-off EML strings when they need a realistic inbox.

Run all ignored GreenMail/Dovecot container tests through the fixed E2E mode, or
pass one Rust test-name filter as the second argument:

```bash
./scripts/test.sh e2e
./scripts/test.sh e2e docker_greenmail_pull_reply_send_e2e
```

The same fixture batch powers a real agent-operation workspace. This prepares a
live GreenMail mailbox and configured afmail workspace, but does not pull or
process mail; the agent must actually operate the inbox:

```bash
./scripts/test.sh demo-prepare
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
order and more context would help. `AGENTS.md` only receives this CRM lookup
hint; the normal mailbox workflow instructions come from `afmail init`.

For an automated scripted demo pass, use:

```bash
./scripts/test.sh demo
```
