# File Formats

This document describes the afmail v1 archive-oriented disk model.

Persisted JSON state documents use `schema_name` plus `schema_version` to
identify their on-disk format. Agent-First Data protocol outputs and audit
events use `code` instead.

## Message JSON

Raw message evidence lives at `.afmail/messages/<message_id>.eml`. Local
disposition lives at `.afmail/messages/<message_id>.state.json`, and remote IMAP
locations live at `.afmail/messages/<message_id>.remote.json` when remote
metadata exists. Parsed `messages/<message_id>.json` files are rebuildable cache
with `schema_name: "message"` and `schema_version: 1`; they include headers,
attachment metadata, `body_text`, remote overlay, and workspace overlay
materialized from the durable sidecars plus case/archive files.

Local state sidecar:

```json
{
  "schema_name": "message_state",
  "schema_version": 1,
  "message_id": "message_20260521_001",
  "status": "triage",
  "archive_uid": null,
  "archived_rfc3339": null,
  "origin": null,
  "updated_rfc3339": "2026-06-01T17:30:00Z"
}
```

Remote sidecar:

```json
{
  "schema_name": "message_remote",
  "schema_version": 1,
  "message_id": "message_20260521_001",
  "locations": [
    {
      "mailbox_name": "INBOX",
      "mailbox_id": "inbox",
      "uid_validity": 44,
      "uid": 900,
      "flags": ["\\Seen"],
      "observed_rfc3339": "2026-06-01T17:30:00Z",
      "missing_rfc3339": null
    }
  ]
}
```

Triage message:

```json
{"workspace": {"status": "triage"}}
```

Direct archived message:

```json
{
  "workspace": {
    "status": "archived",
    "archive_uid": "a20260521001",
    "archived_rfc3339": "2026-06-01T17:30:00Z"
  }
}
```

Message-side remote effects are tracked separately from local disposition. A
message that is already locally filed, spammed, or trashed can still show queued
server work under `workspace.push.pending[]`:

```json
{
  "workspace": {
    "status": "spam",
    "push": {
      "pending": [
        {
          "push_id": "push_20260606_120000_001",
          "kind": "message.spam",
          "queued_rfc3339": "2026-06-06T12:00:00Z"
        }
      ]
    }
  }
}
```

When a queued remote effect succeeds, `pending[]` is cleared and
`last_completed_rfc3339` records the last successful server-side write.

Other local statuses include `case`, `spam`, `trashed`, `sent`,
`draft`, `flagged`, `push_queued`, and `deleted_remote`.

Generated read views for negative dispositions live at `spam/index.md`,
`spam/<message_id>.md`, `trash/index.md`, `trash/<message_id>.md`,
`deleted/index.md`, and `deleted/<message_id>.md`. These files are rebuildable
with `afmail render refresh` and ignored by the managed `.gitignore`; the
durable message state remains in `.afmail/messages/` and `messages/*.json`.
`afmail purge`, `afmail purge spam`, `afmail purge trash`, and
`afmail purge deleted` permanently delete old local discard message records and
then refresh the same views.

Message attachment metadata lives in the materialized message JSON. Attachments are not
copied into case files by assignment or case creation:

```json
{
  "attachments": [
    {
      "part_id": "2",
      "filename": "pricing.txt",
      "content_type": "text/plain",
      "size_bytes": 128,
      "fetched": true,
      "file_path": ".afmail/messages/message_20260521_3af9c1b2e8d04f6a.files/pricing.txt"
    }
  ]
}
```

`part_id` is the MIME part id used by
`afmail message attachment fetch MESSAGE_ID [PART_ID]`. When `fetched` is true,
`file_path` points to the message-cache copy under
`.afmail/messages/<message_id>.files/`.

The managed `.gitignore` ignores rebuildable `messages/*.json` and generated
Markdown read views, but it does not ignore `.afmail/messages/` or
`.afmail/push/`. If you track those durable files in git, the repository will
contain private mail bodies, sidecar metadata, pending push operations, and raw
attachment bytes. Transient `.afmail/transactions/`,
`.afmail/workspace.lock`, and `.afmail/workspace.progress.json` files are
ignored.

## Triage View

Generated triage views live at `triage/<message_id>.md` and include YAML
frontmatter with `kind: triage_view`, `message_id`, `message_ids`,
`generated_rfc3339`, counts, and optional suggestion fields. Triage views are
rebuildable and are not a notes surface.

## Case Workspace

Active cases live at `cases/<group>/<case_uid>-<name>/`; archived cases live at
`archive/cases/<case_uid>-<name>/`.

`case_uid` is a stable `cYYYYMMDDNNN` identity and `archive_uid` is a stable
`aYYYYMMDDNNN` identity. Human-readable names live in `case_name` or
`archive_name` and in the directory suffix. Refs may be either the bare UID or
`UID-any-readable-suffix`; names alone are not valid refs. Human names may use
Unicode path segments such as Chinese, but they must not contain path separators
or be dot-only segments.

Case metadata is canonical in `data/case.json`:

```json
{
  "kind": "case",
  "case_uid": "c20260521001",
  "case_name": "应用反馈-肥料登记",
  "status": "active",
  "tags": [],
  "created_rfc3339": "2026-05-22T09:00:00Z",
  "updated_rfc3339": "2026-05-22T09:00:00Z",
  "message_count": 1,
  "thread_count": 0,
  "attachment_count": 0
}
```

Archived cases set `status: "archived"` and `archived_rfc3339`. Cases do not
store direct-message archive categories.

Case membership is canonical in `data/messages.json`:

```json
{
  "schema_name": "case_messages",
  "schema_version": 1,
  "case_uid": "c20260521001",
  "message_ids": ["message_20260521_3af9c1b2e8d04f6a"]
}
```

Generated case read views live at `case.md` and
`views/messages/<message_id>.md` inside the case workspace. `case.md` is rendered
from `case/case.md.j2`, starts directly with a Markdown heading (no YAML
frontmatter), and links to messages from the case root as
`views/messages/<message_id>.md`. Generated views are rebuilt from
`data/case.json`, `data/messages.json`, and message evidence/cache.

Draft markdown files use frontmatter fields `kind`, `case_uid`, `send_intent`,
`reply_to_message_id`, `subject`, `to`, `cc`, and `attachments`:

```yaml
kind: draft
case_uid: c20260521001
send_intent: reply
reply_to_message_id: message_20260521_3af9c1b2e8d04f6a
subject: "Re: Contract renewal"
to:
  - alice@example.com
cc: []
attachments:
  - files/pricing.txt
```

`attachments:` is a list of case-relative paths for outbound files. Use
`afmail case draft attach REF DRAFT_NAME PATH` to populate it. External sources
are copied into the case `files/` directory; files already under the case are
recorded as safe relative paths. Inbound message-cache paths under
`.afmail/messages/<message_id>.files/` are message evidence and should not be
used as draft attachment paths.

`data/drafts.json` is afmail-managed case-local state. It records each draft's
`last_validated_hash`, `last_validated_rfc3339`, `last_composed_hash`,
`last_composed_rfc3339`, and queued `push_id`. Agents and humans edit draft
Markdown, then run `draft validate` and `compose`; they should not edit
`data/drafts.json` directly.

## Direct Message Archive Category

`archive/notifications/<archive_uid>-<name>/data/archive.json` is canonical:

```json
{
  "schema_name": "archive_messages",
  "schema_version": 1,
  "archive_uid": "a20260521001",
  "archive_name": "服务通知",
  "items": [
    {
      "message_id": "message_20260415_4e218374a33cbdc5",
      "summary": "Contacts Permissions policy update; review if app uses contacts.",
      "archived_rfc3339": "2026-06-01T17:30:00Z"
    }
  ]
}
```

`archive.md` is generated and rebuildable. By default it renders Markdown list
items as time, sender, summary-or-subject, then a message id and relative link.
Message views live at
`archive/notifications/<archive_uid>-<name>/views/messages/<message_id>.md` and
preserve the readable message rendering used elsewhere.

## Generated Templates

Built-in MiniJinja Markdown templates render generated read views and
human-facing scaffolds. A workspace can override templates under
`.afmail/templates/<language>/`; generic `.afmail/templates/<key>` files are
ignored.

Template keys include `case/case.md.j2`, `case/message.md.j2`,
`archive-message/archive.md.j2`, `archive-message/message.md.j2`,
`triage/view.md.j2`, `message/section.md.j2`, `draft/*.md.j2`,
`notes/*.md.j2`, and `workspace/*.j2`.

The context includes `language`, ids, metadata, message/item arrays for index
entry points, `security` facts for message views, compatibility hints,
attachments, and `conversation` where applicable. Template
failures return `template_render_failed`; afmail does not fall back to built-ins
when a selected-language workspace override exists but is invalid.

`afmail render templates` exports all built-in `en-US` and `zh-CN` templates.
Existing language-specific workspace templates are kept unless `--force` is
used.

## Push Items

Push items live in `.afmail/push/<push_id>.json`. Each item uses
`schema_name: "push_item"` and `schema_version: 1`, plus a typed payload:

- `kind: "outbound"` stores composed draft metadata, envelope data, the staged
  `.eml` path, `draft_hash`, and configured draft save/send steps.
- `kind: "message_action"` stores `action: "archive" | "spam" | "trash" |
  "case_add"`, message ids, remote locations, and configured action steps.

Remote writes occur only through explicit `afmail push ... --confirm`; bare push
commands are previews. Outbound reply send defaults mark the replied-to message
with `\Seen` and `\Answered`; adding a message to a case does not mark remote
mail as seen by default.

## Local Transactions

Incomplete local writes are recorded under
`.afmail/transactions/<transaction_id>.json` while afmail updates related local
files. Successful operations remove the transaction file. If one remains,
writers stop and `afmail doctor` reports `transaction_incomplete`.

## Workspace Progress

The latest long-running `pull` or confirmed `push` writes a runtime snapshot to
`.afmail/workspace.progress.json`. It uses `schema_name:
workspace_progress`, `schema_version: 1`, command/status/phase timestamps,
phase-specific `fields`, and final `result` or `error` summaries. Read it with
`afmail status`; it is a volatile progress surface, not an audit log.

## Audit Events

Audit logs are JSONL records in `.afmail/logs/events.jsonl`. Archive-related
events include:

- `message_archived`
- `message_archive_moved`
- `message_archive_category_renamed`
- `message_archive_summary_set`
- `message_unarchived`
- `message_unspammed`
- `message_untrashed`
- `case_archived`
- `case_restored`
- `archive_case_renamed`
