# Workspace Model

An afmail workspace is a local, file-first mailbox workspace. Files are the read
interface; the CLI is the effect interface.

## Layout

```text
account-workspace/
  AGENTS.md
  triage/
    <message_id>.md
  spam/  # created when spam messages first exist
    data/
      spam.json
    <message_id>.md
  trash/  # created when trashed messages first exist
    data/
      trash.json
    <message_id>.md
  deleted/  # created when remote-deleted messages first exist
    data/
      deleted.json
    <message_id>.md
  cases/
    <group>/
      <case_uid>-<name>/
        case.md
        notes.md
        data/
          case.json
          drafts.json
        views/
          messages/
            <message_id>.md
        drafts/
        files/
  archive/
    cases/
      <case_uid>-<name>/
        case.md
        notes.md
        data/
          case.json
          drafts.json
        views/
          messages/
            <message_id>.md
        drafts/
        files/
    notifications/
      <archive_uid>-<name>/
        archive.md
        notes.md
        data/
          notification.json
        views/
          messages/
            <message_id>.md
  messages/
    <message_id>.json
  templates/
    en-US/
    zh-CN/
  .afmail/
    DO_NOT_EDIT.txt
    config.json
    workspace.lock
    workspace.progress.json
    logs/events.jsonl
    transactions/
    push/
    messages/
      <message_id>.eml
      <message_id>.remote.json
      <message_id>.files/
```

`triage/` and `cases/` are active attention surfaces. `spam/`, `trash/`, and
`deleted/` are generated review views for local discard states, created only
after the first matching message appears.
`archive/cases/` contains archived case workspaces addressed by case UID.
`archive/notifications/<archive_uid>-<name>/` contains direct archived messages
in one archive category.

Case roots contain only user-facing Markdown entry points (`case.md` and
`notes.md`) plus working directories. Direct archive roots contain `archive.md`
and `notes.md`. Canonical local object state lives under `data/`; generated,
rebuildable Markdown detail views live under `views/`. `drafts/` and `files/`
are user-visible working materials.

`case.md`, `archive.md`, `triage/*.md`, `spam/*.md`, `trash/*.md`,
`deleted/*.md`, and `views/**/*.md` are generated read views. They are safe to
rebuild with `afmail render refresh`; use `notes.md` for durable notes instead
of generated views. Case and archive message links point to
`views/messages/<message_id>.md`.

`.afmail/DO_NOT_EDIT.txt` is a warning sentinel. The rest of `.afmail/` is
machine-managed evidence, remote state, push queue, and audit history; use the
CLI for effects instead of editing it by hand. Generated Markdown template
overrides live under user-editable `templates/`, outside `.afmail/`.

Persisted JSON state documents identify their on-disk format with
`schema_name` and `schema_version`. CLI stdout, diagnostics, errors, and
`.afmail/logs/events.jsonl` audit events remain Agent-First Data protocol
messages and use `code`.

The managed `.gitignore` intentionally does not ignore `.afmail/messages/`: raw
mail evidence and remote metadata are durable local state. Tracking those files
in git means the repository contains private mail bodies and attachment bytes.
The managed ignore block covers local runtime or generated files such as
`.afmail/push/`, `.afmail/logs/`,
`.afmail/transactions/`, `.afmail/workspace.lock`,
`.afmail/workspace.progress.json`, `.codex/skills/agent-first-mail/`,
`messages/*.json`, `triage/*.md`,
`spam/*.md`, `trash/*.md`, `deleted/*.md`, and generated object Markdown views.

## Message State

Message evidence lives in `.afmail/messages/<message_id>.eml`; remote mailbox
metadata lives in `.afmail/messages/<message_id>.remote.json`; parsed
`messages/<message_id>.json` files are rebuildable cache. Triage, case,
notification, spam, trash, and deleted views are generated from message evidence
plus the object collections under their respective directories. Inbound
attachments belong to the message. Attachment metadata is stored on the message
record; `afmail message attachment fetch MESSAGE_ID [PART_ID]` materializes
files under `.afmail/messages/<message_id>.files/` and refreshes generated read
views so fetched paths appear in message renderings.

A message can be referenced by multiple active or archived cases. A message can
belong to at most one direct-message archive category. If a message needs
multiple contexts, create or use cases instead of multi-archiving the direct
message.

Local discard dispositions are canonical in their own directories:
`spam/data/spam.json`, `trash/data/trash.json`, and
`deleted/data/deleted.json`. Fresh workspaces omit empty disposition
directories and JSON; once created, those state files may remain if a
collection later becomes empty. The Markdown files under those directories are
generated views.

## Cases

Active cases live at `cases/<group>/<case_uid>-<name>/`. Archived cases live at
`archive/cases/<case_uid>-<name>/`. `case_uid` is globally unique and stable
across active and archived cases. `rename --name` updates `data/case.json` and
the readable directory suffix.

Case refs must start with `cYYYYMMDDNNN`; archive refs must start with
`aYYYYMMDDNNN`. A ref may include a readable suffix after one dash, so
`c20260521001-anything` and `c20260521001` are equivalent. Names alone are not
looked up. Group and tag values are local path-segment identifiers. Human names
may use Unicode such as Chinese, for example `应用反馈-肥料登记` or `服务通知`.
Do not use path separators (`/` or `\`) or the dot-only segments `.`/`..`.

Case metadata is canonical in `data/case.json`. Active cases use
`status: "active"`; archived cases use `status: "archived"` plus
top-level `archived_rfc3339`. Message membership is stored in the same
`data/case.json` collection under `items[]`, with per-message `summary` and
`added_rfc3339`. Archived cases do not use direct-message archive categories.

Case-local `data/drafts.json` files are afmail-managed machine state. They
record only the last validation hash/time for `drafts/*.md` files. Humans and
agents should edit draft Markdown, not `data/drafts.json`; queued save/send
items reference the draft by case and filename and use the latest content at
push time.

## Drafts And Case Files

Draft Markdown lives under a case `drafts/` directory. Outbound attachments
belong to the draft/case, not to inbound message evidence. Use
`afmail case draft attach REF DRAFT_NAME PATH` to add one: external files are
copied into the case `files/` directory with a safe filename, and files already
inside the case are recorded as case-relative paths without another copy.

The draft frontmatter `attachments:` list contains case-relative paths such as
`files/screenshot.png`. Validation and push check that each path is relative,
safe, and points to an existing file under the case workspace. Adding or editing
attachments changes the draft; queued `draft save` / `draft send` items still
use the latest Markdown content. Use `afmail case draft remove` to delete the
draft and any queued save/send item.

## Direct Message Archive Categories

The canonical membership file for a direct-message archive category is
`archive/notifications/<archive_uid>-<name>/data/notification.json`:

```json
{
  "schema_name": "notification",
  "schema_version": 1,
  "collection_uid": "a20260521001",
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

`summary` is optional. Generated `archive.md` renders a Markdown list using
`archive.message_index` config. The built-in archive templates display the
message subject when a summary field is empty. Generated message views live
under `archive/notifications/<archive_uid>-<name>/views/messages/<message_id>.md`.

## Generated Read-View Templates

Built-in MiniJinja Markdown templates render generated read views and
human-facing scaffolds. A workspace can override language-specific templates
under `templates/<language>/`. Legacy `.afmail/templates/` files are ignored.

Common generated-view template keys include:

- `case/case.md.j2` and `case/message.md.j2`
- `archive-message/archive.md.j2` and `archive-message/message.md.j2`
- `triage/view.md.j2` and `message/section.md.j2`

Run `afmail render templates` to export built-ins, then `afmail render refresh`
to rebuild generated Markdown after template edits.

Generated read-view templates use namespaced context data: `message` is the
exact materialized `messages/<message_id>.json` record, and `view` contains
Markdown-only display facts such as rendered conversation text, security notes,
and attachment preview paths. Index rows use the same split as `item.message`
and `item.view`.

## Deleted Remote Messages

When a remote message disappears and has no case/archive/draft/push reference,
afmail keeps the local evidence under `.afmail/messages/`, marks its local
state as `deleted_remote`, and exposes it under generated `deleted/` views.
`afmail purge` permanently deletes old local `spam`, `trashed`, and
`deleted_remote` message records; add `spam`, `trash`, or `deleted` to limit it
to one disposition, and use `--older-than-days` to override the default 30-day
threshold. When a referenced remote message disappears, afmail keeps the
business state such as `case` or `archived` and only marks remote locations
missing so existing case/archive state stays resolvable.

## Notes

`notes.md` files are plain Markdown with no frontmatter. They exist for active
cases, archived cases, and direct-message archive categories. They are the only
durable local notes surface inside those objects; generated views are disposable.
