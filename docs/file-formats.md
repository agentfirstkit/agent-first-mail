# File Formats

This document describes the afmail v1 archive-oriented disk model.

Persisted JSON state documents use `schema_name` plus `schema_version` to
identify their on-disk format. Agent-First Data protocol outputs and audit
events use `code` instead.

## Message JSON

Raw message evidence lives at `.afmail/messages/<message_id>.eml`. Remote IMAP
locations live at `.afmail/messages/<message_id>.remote.json` when remote
metadata exists. Parsed `messages/<message_id>.json` files are rebuildable cache
with `schema_name: "message"` and `schema_version: 1`; they include headers,
attachment metadata, `body_text`, remote overlay, workspace overlay materialized
from durable object collections plus remote metadata, an optional `contact` link
materialized from the sender address, and inbound identity-match fields
materialized from configured workspace identities.

The `contact` field is present only when the `From` address matches a contact
card. It is a snapshot refreshed at pull/render time and on contact changes:

```json
{
  "contact": {
    "contact_uid": "p20260521001",
    "display_name": "Zhang San"
  }
}
```

Inbound mail records which configured identity appears to have received the
message. Unique matches set `identity`, `identity_email`, and `identity_match`
(`email` or `name`). Ambiguous same-address personas set `identity_match:
"multiple"` plus `identity_candidates`. Unknown recipient addresses set
`identity_match: "unmatched"` plus `observed_recipient_emails`.

```json
{
  "identity": "support",
  "identity_email": "hello@example.com",
  "identity_match": "name"
}
```

Remote sidecar:

```json
{
  "schema_name": "message_remote",
  "schema_version": 1,
  "message_id": "message_20260521_3af9c1b2e8d04f6a",
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

Local discard dispositions are canonical in object directories, not per-message
state sidecars:

```json
{
  "schema_name": "spam",
  "schema_version": 1,
  "collection_uid": "spam",
  "collection_name": "Spam",
  "status": "spam",
  "items": [
    {
      "message_id": "message_20260521_3af9c1b2e8d04f6a",
      "added_rfc3339": "2026-06-01T17:30:00Z"
    }
  ]
}
```

The matching files are `spam/data/spam.json`, `trash/data/trash.json`, and
`deleted/data/deleted.json` (`schema_name: "deleted_remote"`). Fresh
workspaces do not create empty disposition directories or JSON; a disposition
data file is created when the first message enters that disposition and may
remain if the collection later becomes empty.

Notification message:

```json
{
  "workspace": {
    "status": "notification",
    "notification_uid": "n20260521001",
    "notification_rfc3339": "2026-06-01T17:30:00Z"
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
          "push_id": "push_20260606T120000Z",
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
`deleted/index.md`, and `deleted/<message_id>.md`. Fresh workspaces do not
create empty disposition views. Existing generated views are rebuildable with
`afmail render refresh` and ignored by the managed `.gitignore`; the durable
message state remains in `.afmail/messages/` and `messages/*.json`.
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

The managed `.gitignore` ignores rebuildable `messages/*.json`, generated
Markdown read views, `.afmail/push/`, `.afmail/logs/`, `.afmail/transactions/`,
`.afmail/workspace.lock`, `.afmail/workspace.progress.json`,
`.afmail/render-cache.json`, `.afmail/config.example.json`, and installed
workspace skill directories such as `.codex/skills/agent-first-mail/` or
`.claude/skills/agent-first-mail/`. It intentionally does not ignore
`.afmail/messages/`. If you track
`.afmail/messages/` in git, the repository will contain private mail bodies,
sidecar metadata, and raw attachment bytes.

## Triage View

Generated triage views live at `triage/<message_id>.md` and include YAML
frontmatter with `kind: triage_view`, `message_id`, `message_ids`,
`message_count`, `attachment_count`, and optional suggestion fields
(`suggested_case_uids`, `suggested_reason`). Triage views are
rebuildable and are not a notes surface.

## Case Workspace

Active cases live at `cases/<group>/<case_uid>-<name>/`; archived cases live at
`archived-cases/<group>/<case_uid>-<name>/`. The `<group>` segment is a case
group retained for history/lookup, not an archive category.

`case_uid` is a stable `cYYYYMMDDNNN` identity and `notification_uid` is a stable
`nYYYYMMDDNNN` identity. Human-readable names live in the shared
`collection_name` field and in the directory suffix. Refs may be either the bare
UID or `UID-any-readable-suffix`; names alone are not valid refs. Human names
may use Unicode path segments such as Chinese, but they must not contain path
separators or be dot-only segments.

Case metadata is canonical in `data/case.json`:

```json
{
  "schema_name": "case",
  "schema_version": 1,
  "collection_uid": "c20260521001",
  "collection_name": "应用反馈-肥料登记",
  "tags": [],
  "created_rfc3339": "2026-05-22T09:00:00Z",
  "updated_rfc3339": "2026-05-22T09:00:00Z",
  "message_count": 1,
  "thread_count": 0,
  "attachment_count": 0,
  "items": [
    {
      "message_id": "message_20260521_3af9c1b2e8d04f6a",
      "summary": "Renewal pricing and contract timing.",
      "added_rfc3339": "2026-05-22T09:00:00Z"
    }
  ]
}
```

Active/archived lifecycle is represented by path, not by a case `status` field.
Archived cases set `archived_rfc3339`. Cases do not store notification
collections.

Case membership is canonical in `data/case.json` under `items[]`; each item uses
the same message collection item shape as notification collections:
`message_id`, optional `summary`, and `added_rfc3339`.

Generated case read views live at `case.md` and
`views/messages/<message_id>.md` inside the case workspace. `case.md` is rendered
from `case/case.md.j2`, starts directly with a Markdown heading (no YAML
frontmatter), and links to messages from the case root as
`views/messages/<message_id>.md`. Generated views are rebuilt from
`data/case.json` and message evidence/cache.

Draft markdown files use frontmatter fields `kind`, `case_uid`, `send_intent`,
`reply_to_message_id`, `identity`, `subject`, `to`, `cc`, and `attachments`:

```yaml
kind: draft
case_uid: c20260521001
send_intent: reply
reply_to_message_id: message_20260521_3af9c1b2e8d04f6a
identity: support
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

`identity:` is a config identity slug. If absent, afmail resolves the configured
default identity. Optional footers are inserted into the draft body when the
draft is created or its identity changes; send time does not append hidden text.

`data/drafts.json` is afmail-managed case-local state. It records each draft's
`last_validated_hash` and `last_validated_rfc3339`. Agents and humans edit
draft Markdown directly (or use `draft change`); queued `draft save` and
`draft send` items reference the draft by case and filename and use the latest
valid Markdown at push time. They should not edit `data/drafts.json` directly.

## Identity Persona Override

`.afmail/config.json` is the canonical identity registry. Each configured
identity has a slug, display name, email address, and exactly one default.
Optional `identities/<identity>.md` files may override the display name and add a
footer plus agent-facing notes. They do not override `email` or `default`.

```markdown
---
kind: identity
identity: support
name: "Support Team"
footer: |
  --
  Support Team
  Agent-First Kit
---
Use this identity for customer support replies.
```

The frontmatter `identity` must exist in config and must match the file stem.
Contacts are other people; identities are our send personas.

## Contact Card

Contact cards live at `contacts/<group>/<contact_uid>-<name>.md`. Each card is
canonical Markdown with a `kind: contact` frontmatter and a free-form notes body:

```markdown
---
kind: contact
contact_uid: p20260521001
display_name: Zhang San
emails:
- zhang@example.com
phones:
- "13800138000"
organization: Acme
role: Buyer
tags:
- vip
created_rfc3339: 2026-06-01T17:30:00Z
updated_rfc3339: 2026-06-01T17:30:00Z
---

## Zhang San

Free-form notes.
```

`contact_uid` starts with `pYYYYMMDDNNN` and is stable across active/archived.
Email addresses are globally unique across all contacts. There is no separate
email→contact index file; the sender→contact link is materialized onto each
message's `contact` field (see Message JSON) at pull/render time and on contact
changes.

## Notification Collection

`notifications/<notification_uid>-<name>/data/notification.json` is canonical:

```json
{
  "schema_name": "notification",
  "schema_version": 1,
  "collection_uid": "n20260521001",
  "collection_name": "服务通知",
  "items": [
    {
      "message_id": "message_20260415_4e218374a33cbdc5",
      "summary": "Contacts Permissions policy update; review if app uses contacts.",
      "added_rfc3339": "2026-06-01T17:30:00Z"
    }
  ]
}
```

`notification.md` is generated and rebuildable. By default it renders each message as
a numbered heading (the queued action label plus the message summary, or the
subject when the summary is empty), followed by bullets for the sender, a
message-id link to the detail view, and the time. The configurable
`notification.message_index.item_fields` (default time, from, summary) is exposed to
templates as `view.fields` for custom layouts.
Message views live at
`notifications/<notification_uid>-<name>/views/messages/<message_id>.md` and
preserve the readable message rendering used elsewhere.

## Generated Templates

Built-in MiniJinja Markdown templates render generated read views and
human-facing scaffolds. A workspace can override templates under
`templates/<language>/`. Legacy `.afmail/templates/` files are ignored.

Template keys include `case/case.md.j2`, `case/message.md.j2`,
`notification/notification.md.j2`, `notification/message.md.j2`,
`triage/view.md.j2`, `message/section.md.j2`, `status/index.md.j2`,
`status/message.md.j2`, `draft/*.md.j2`, `notes/*.md.j2`, and `workspace/*.j2`.

Generated read-view contexts use explicit namespaces. `message` is the exact
materialized `messages/<message_id>.json` record, while `view` holds Markdown
rendering facts such as display titles, security notes, attachment previews, and
conversation text. Index rows follow the same split with `item.message` and
`item.view`; container data lives under namespaces such as `frontmatter`,
`case`, `notification`, `item`, `items`, `messages`, and `config`. Template failures
return `template_render_failed`; afmail does not fall back to built-ins when a
selected-language workspace override exists but is invalid.

`afmail render templates` exports the built-in templates for every supported language.
Existing language-specific workspace templates are kept unless `--force` is
used.

## Push Items

Push items live in `.afmail/push/<push_id>.json`. Each item uses
`schema_name: "push_item"` and `schema_version: 1`, plus a typed payload:

- `kind: "outbound"` stores `action: "save_draft" | "send"`, `case_uid`,
  `draft_name`, timestamps, and step retry state. It does not store a staged
  `.eml`, a draft hash, or a precomputed outbound message id.
- `kind: "message_action"` stores `action: "notification" | "spam" | "trash" |
  "case_add"`, message ids, remote locations, and configured action steps.

Remote writes occur only through explicit `afmail push --confirm`, which applies
the whole queue; bare `afmail push` previews. Outbound reply send defaults mark the replied-to message
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

Audit logs are JSONL records in `.afmail/logs/events.jsonl`. Notification and
archive-related events include:

- `notification_created`
- `notification_message_added`
- `notification_message_moved`
- `notification_renamed`
- `notification_summary_set`
- `notification_message_removed`
- `message_unspammed`
- `message_untrashed`
- `case_archived`
- `case_unarchived`
- `case_renamed`
