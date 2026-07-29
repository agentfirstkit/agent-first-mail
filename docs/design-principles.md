# Core Design Principles

## 1. Files Are The Default Local Read Interface

Agents and humans should read Markdown and JSON files directly. The CLI exists
for effects: moving local attention state, queueing remote work, composing mail,
fetching attachments, and writing audit events.

The typed HTTP API is the integration interface for Agent runtimes and
applications that need an explicit machine contract. It publishes OpenAPI and
standalone JSON Schemas but enters the same dispatch, locks, transaction guards,
idempotency records, store methods, and push plan confirmation as the CLI. It is
not another state model, and neither API clients nor UIs may edit machine state
behind those boundaries.

Commands assigned a shared lock are strict `Read` or `Plan` operations: they may
project EML, canonical metadata, object state, identities, contacts, and push effects in
memory, but they do not change workspace files, directories, bytes, or mtimes.
Generated caches and views are materialized only by explicit write paths such as
pull, mutation, repair, or `afmail render refresh`.

An API server holds no workspace lock for its lifetime. Each request takes only
the shared or exclusive lock required by its corresponding command, so a
long-running local integration surface does not block normal afmail work.

## 2. Active Attention Is Separate From Archiving And Archived Cases

`triage/` is for unprocessed message views. `cases/` is for active case work.
`archived-cases/` holds both cases waiting on queued sends/archive effects and
completed cases:

- `archived-cases/<group>/<case_uid>-<name>/` with `status: archiving` and no
  `archived_rfc3339` is an archiving case workspace.
- The same path with `status: archived` is a finalized case workspace; its
  `<group>` segment is the case group carried forward from active work, not an
  archive category.
- `notifications/<notification_uid>-<name>/` is a notification collection.

Case identity is stable across these directories. `afmail case show`,
`afmail case notes`, `afmail case remove`, `afmail case move`, `afmail case trash`,
`afmail case delete`, and `afmail case rename` operate on active, archiving, or
archived cases by ref. Active-work commands such as `case add`, `case tag`, and
`case draft ...` require `afmail case unarchive REF --group GROUP --reason TEXT`
first.

## 3. Identity Is Stable

`message_id` is the stable local identity for mail. `case_uid` is a stable
`cYYYYMMDDNNN` identity across active, archiving, and archived cases, and
`notification_uid` is a stable `nYYYYMMDDNNN` identity for notification
collections. Remote IMAP moves update recorded locations but do not rename local
message ids.

Each workspace also has an immutable `.afmail/workspace.json` identity. Its
`workspace_uid` survives directory moves and private-Git clone/restore and is
included in destructive plan digests, so approval from one workspace cannot be
reused in another.

Human names are separate labels. Directories use `<uid>-<name>`, and
`rename --name` changes the label and suffix without changing the UID. Commands
resolve refs only from the UID prefix: `c20260521001` and
`c20260521001-anything` are equivalent, while names alone are invalid. Human
names may use Unicode such as `应用反馈-肥料登记` and `服务通知`; path separators and
dot-only segments are not valid names.

## 4. Cases Are The Multi-Context Tool

A notification message may belong to exactly one notification collection. If a
message needs multiple classifications, use cases instead of placing one message
into multiple notification collections. A message may be referenced by multiple
active, archiving, or archived cases.

## 5. Archive Is Local First, Remote Explicit

Notification and archive commands change local attention/archive state and may
queue configured remote moves. They do not create IMAP notification/category folders and they do not
mutate remote mail until `afmail push --confirm PLAN_ID` runs. Bare `afmail push`
previews the whole resolved queue and returns a workspace-bound `plan_id`; confirm
applies only that exact snapshot. Changes to the queue, drafts, attachments,
identities, or action configuration invalidate the old ID before remote access.

Remote archive moves are rule-driven by recorded source mailbox id via
`actions.message.notification.by_source_mailbox_id.<id>.steps`. Default `inbox` moves to
`archive`; default non-inbox sources have no archive remote steps.

## 6. Notes Are Human Memory

`notes.md` files are plain Markdown with no frontmatter and are user-authored
notes. Command reasons and machine history belong in
`.afmail/logs/events.jsonl`, not in notes.

## 7. Generated Views Are Rebuildable

Generated triage views, case `case.md`, case `views/messages/*.md`,
notification `notification.md`, and notification `views/messages/*.md` should be reproducible from
message evidence and canonical `data/*.json` state. Persistent human edits
belong in notes, drafts, files, or `templates/` when the user is
intentionally customizing generated Markdown or Review templates. Both outputs
contain the same typed render documents; presentation remains in templates.

Suggestions preserve this boundary. `agent_suggestion` and `human_suggestion`
are typed annotations on canonical message, case, or notification JSON. They
never live in generated Markdown and never get a separate recommendation or
review-record file. A new Agent suggestion replaces the current round and clears
its human response; audit records the transition but is not replayed into current
state. Rendering only projects canonical state and cannot create, recover, or
modify a suggestion.

Drafts remain ordinary Markdown, and files under `drafts/` are the only
editable source. afmail records validation metadata in case-local
`data/drafts.json`, but queued `draft upload` / `draft send` items only reference
the draft by case and filename. Do not edit machine state directly; change the
Markdown (or use `draft change`) and the next preview will use the latest valid
content. Once previewed, any further relevant edit requires a new plan and
approval.

## 8. Safety Comes From Reference Checks

Before remote Archive/Junk/Trash moves, afmail scans case message refs, drafts,
and push queue items. A message with an active case or draft reference cannot be
archived remotely until the blocking local work is resolved.

## 9. One User, One Workspace

A workspace belongs to a single user and their agent. afmail is not a shared
inbox or helpdesk: it does not synchronize local workspace state across machines,
and it has no claim/assign or multi-editor coordination. `cases/`, `drafts/`, and
`triage/` are personal working memory, not shared state.

The IMAP account is the only shared source of truth. Several personal workspaces
may point at one account; coordination between them happens through IMAP itself
(flags such as `\Seen`/`\Answered` and folder moves, reconciled on `pull`), not
through afmail. Nothing prevents two such workspaces from independently drafting
and sending, so concurrent operators on one account is out of scope by design.

The runtime workspace lock is stored outside the workspace under the current
user's temporary runtime directory and keyed by the canonical workspace path.
It serializes local afmail processes without making shared reads dirty the
workspace; it is not cross-host coordination. `.afmail/cache/progress.json` is only the latest local
push/pull progress snapshot for observers; it is not durable coordination or an
audit log.

## Lifecycle Summary

```text
message imported as triage -> triage/ -> active case -> archived-cases/<group>/<case_uid>-<name>/
message imported as triage -> triage/ -> notification collection
unreferenced remote-missing message -> deleted/
notification message -> notification remove -> triage/
archived case -> case unarchive -> cases/<group>/<case_uid>-<name>/
```

`spam`, `trash`, and `deleted_remote` are negative/discard dispositions.
Notification collections are for completed local filing, not remote folder
design.

## Skill Design: Behavior, Not Flag Reference

`skills/agent-first-mail/SKILL.md` is loaded by Codex and Claude Code as the agent's
behavior contract when operating afmail. Keep behavior rules, decision rules,
non-obvious defaults, and recovery guidance in the skill. Keep flag
enumerations, option matrices, and full command references in `afmail --help`
and `docs/cli.md` so the skill stays small and does not rot across releases.
