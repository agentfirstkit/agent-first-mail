# Core Design Principles

## 1. Files Are The Read Interface

Agents and humans should read Markdown and JSON files directly. The CLI exists
for effects: moving local attention state, queueing remote work, composing mail,
fetching attachments, and writing audit events.

## 2. Active Attention Is Separate From Archive

`triage/` is for unprocessed message views. `cases/` is for active case work.
`archive/` is for completed classified items:

- `archive/cases/<group>/<case_uid>-<name>/` is an archived case workspace; its
  `<group>` segment is the case group carried forward from active work, not an
  archive category.
- `notifications/<notification_uid>-<name>/` is a notification collection.

Active commands operate on active surfaces. Archived items are addressed through
`afmail archive ...` commands. Reads are exempt: `afmail case show` and
`afmail case notes show` resolve a case whether it is active or archived.

## 3. Identity Is Stable

`message_id` is the stable local identity for mail. `case_uid` is a stable
`cYYYYMMDDNNN` identity across active and archived cases, and
`notification_uid` is a stable `nYYYYMMDDNNN` identity for notification
collections. Remote IMAP moves update recorded locations but do not rename local
message ids.

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
active or archived cases.

## 5. Archive Is Local First, Remote Explicit

Notification and archive commands change local attention/archive state and may
queue configured remote moves. They do not create IMAP notification/category folders and they do not
mutate remote mail until `afmail push --confirm` runs. Bare `afmail push`
previews the whole queue; `--confirm` applies all of it.

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
intentionally customizing generated read-view templates.

Drafts remain ordinary Markdown, and files under `drafts/` are the only
editable source. afmail records validation metadata in case-local
`data/drafts.json`, but queued `draft save` / `draft send` items only reference
the draft by case and filename. Do not edit machine state directly; change the
Markdown (or use `draft change`) and push will use the latest valid content.

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

`.afmail/workspace.lock` only serializes concurrent afmail processes against one
local workspace directory to prevent corruption. It is not cross-host
coordination. `.afmail/workspace.progress.json` is only the latest local
push/pull progress snapshot for observers; it is not durable coordination or an
audit log.

## Lifecycle Summary

```text
message imported as triage -> triage/ -> active case -> archive/cases/<group>/<case_uid>-<name>/
message imported as triage -> triage/ -> notification collection
unreferenced remote-missing message -> deleted/
notification message -> notification restore -> triage/
archived case -> archive case restore -> cases/<group>/<case_uid>-<name>/
```

`spam`, `trash`, and `deleted_remote` are negative/discard dispositions.
Notification collections are for completed local filing, not remote folder
design.

## Skill Design: Behavior, Not Flag Reference

`skills/agent-first-mail.md` is loaded by Codex and Claude Code as the agent's
behavior contract when operating afmail. Keep behavior rules, decision rules,
non-obvious defaults, and recovery guidance in the skill. Keep flag
enumerations, option matrices, and full command references in `afmail --help`
and `docs/cli.md` so the skill stays small and does not rot across releases.
