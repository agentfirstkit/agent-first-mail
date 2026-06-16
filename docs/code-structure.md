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
  runner/            command dispatch, output formatting, locks, push, and purge orchestration
  config/            typed config schema, defaults, key access, and validation
  store/             workspace file model and local state transitions
  imap_pull/         read-only IMAP pull sessions, remote identity, special-use folders
  imap_client.rs     low-level IMAP session, mailbox info, and move outcomes
  smtp_send.rs       outbound message construction and SMTP sending
  remote.rs          MailRemote trait over IMAP/SMTP side effects, plus the test fake
  push_queue/        durable local push items plus preview/execute helpers
  types/             stable ids, message/case/archive/push DTOs
  mail.rs            MIME parsing into message DTOs
  markdown.rs        Markdown frontmatter split/parse/render and conversation extraction
  frontmatter.rs     draft/triage/case-message frontmatter types
  templates.rs       built-in and workspace MiniJinja template loading
  skill_admin.rs     skill install/status backing `afmail skill`
  progress.rs        workspace progress snapshot sink and status projection
  error.rs           AppError type with structured error codes and hints
  util.rs            crate-wide atomic-write, hashing, and flag helpers
  workspace_lock.rs  workspace lock-file guard
```

## Store Modules

`store/mod.rs` keeps the `Workspace` type, workspace discovery/init/status, and
high-level wiring. Sibling modules add focused `impl Workspace` blocks:

- `archive.rs` handles direct-message archive categories and archived case IO.
- `cases.rs` handles active case CRUD, membership, notes, tags, and moves.
- `messages.rs` handles message cache materialization, reads, local status
  transitions, attachments, related-message discovery, and relocation.
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
- `drafts.rs` owns draft creation, validation, composition, and attachment
  bookkeeping for cases.
- `push_state.rs`, `purge.rs`, `doctor.rs`, and `transactions.rs` own narrower
  maintenance concerns.
- `util.rs` contains store-private filesystem, id, path, timestamp, and audit
  helpers shared by the modules above.

Keep new store behavior near the durable state it changes. Prefer another small
module only when a concern has its own file format, lifecycle, or command group.

## File-State Boundaries

The workspace keeps durable state in a few canonical places:

- Raw message evidence and remote sidecars live under `.afmail/messages/`.
- Rebuildable parsed message caches live under `messages/`.
- Case state lives in `cases/.../data/case.json` or
  `archive/cases/.../data/case.json`.
- Direct archive state lives in
  `archive/notifications/.../data/notification.json`.
- Local discard state lives in `spam/data/spam.json`,
  `trash/data/trash.json`, and `deleted/data/deleted.json` after the first
  message enters that disposition.
- Push items live in `.afmail/push/`; audit events live in
  `.afmail/logs/events.jsonl`.

Generated Markdown views (`triage/`, `case.md`, `archive.md`, `views/`,
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

For behavior-preserving cleanup, run from `spores/agent-first-mail/`:

```bash
cargo fmt
cargo check
cargo test
```

Use `cargo clippy --all-targets --all-features -- -D warnings` before release or
when changing shared helpers. Docker/container tests remain gated by the existing
integration-test environment flags.

## Fixture Batch, Docker E2E, and Demo

The reusable mailbox story lives under `tests/fixtures/mail-batch/`: a
`manifest.json` plus 30 realistic `.eml` files. Tests should load that manifest
instead of inventing new one-off EML strings when they need a realistic inbox.

Run the GreenMail-backed E2E locally with `--ignored`; no extra environment
gate is required:

```bash
cargo test --test container_e2e -- --ignored --nocapture
```

The same fixture batch powers a real agent-operation workspace. This prepares a
live GreenMail mailbox and configured afmail workspace, but does not pull or
process mail; the agent must actually operate the inbox:

```bash
bash scripts/demo-greenmail-fixtures.sh prepare
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
bash scripts/demo-greenmail-fixtures.sh run
```
