# Workspace Model

An afmail workspace is a local, file-first mailbox workspace. Files are the read
interface; the CLI is the effect interface.

## Layout

```text
account-workspace/
  AGENTS.md
  triage/
    message_<id>.md
  spam/
    <message_id>.md
  trash/
    <message_id>.md
  deleted/
    <message_id>.md
  cases/
    <group>/
      <case_uid>-<name>/
        case.md
        notes.md
        data/
          case.json
          messages.json
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
          messages.json
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
          archive.json
        views/
          messages/
            <message_id>.md
  messages/
    <message_id>.json
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
      <message_id>.state.json
      <message_id>.remote.json
      <message_id>.files/
    templates/
      en-US/
      zh-CN/
```

`triage/` and `cases/` are active attention surfaces. `spam/`, `trash/`, and
`deleted/` are generated review views for local discard states.
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
CLI for effects instead of editing it by hand. `.afmail/templates/` is the only
user-editable exception under `.afmail/`.

Persisted JSON state documents identify their on-disk format with
`schema_name` and `schema_version`. CLI stdout, diagnostics, errors, and
`.afmail/logs/events.jsonl` audit events remain Agent-First Data protocol
messages and use `code`.

The managed `.gitignore` intentionally does not ignore `.afmail/messages/` or
`.afmail/push/`: raw mail evidence, local disposition sidecars, remote metadata,
and pending push operations are durable local state. Tracking those files in git
means the repository contains private mail bodies and attachment bytes. The
managed ignore block covers rebuildable/runtime files such as `messages/*.json`,
`triage/*.md`, `spam/*.md`, `trash/*.md`, `deleted/*.md`, generated object
Markdown views, `.afmail/logs/`, `.afmail/transactions/`,
`.afmail/workspace.lock`, and `.afmail/workspace.progress.json`.

## Message State

Message evidence lives in `.afmail/messages/<message_id>.eml`. Local disposition
lives in `.afmail/messages/<message_id>.state.json`; remote mailbox metadata
lives in `.afmail/messages/<message_id>.remote.json`; parsed
`messages/<message_id>.json` files are rebuildable cache. Triage, case, and
archive views are generated from that evidence and workspace state. Inbound
attachments belong to the message. Attachment metadata is stored on the message
record; `afmail message attachment fetch MESSAGE_ID [PART_ID]` materializes
files under `.afmail/messages/<message_id>.files/` and refreshes generated read
views so fetched paths appear in message renderings.

A message can be referenced by multiple active or archived cases. A message can
belong to at most one direct-message archive category. If a message needs
multiple contexts, create or use cases instead of multi-archiving the direct
message.

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
`archived_rfc3339`. Case membership is canonical in `data/messages.json`.
Archived cases do not use direct-message archive categories.

Case-local `data/drafts.json` files are afmail-managed machine state. They
record the last validated and composed hashes for `drafts/*.md` files so afmail
can detect edits after validation or compose. Humans and agents should edit
draft Markdown, not `data/drafts.json`.

## Drafts And Case Files

Draft Markdown lives under a case `drafts/` directory. Outbound attachments
belong to the draft/case, not to inbound message evidence. Use
`afmail case draft attach REF DRAFT_NAME PATH` to add one: external files are
copied into the case `files/` directory with a safe filename, and files already
inside the case are recorded as case-relative paths without another copy.

The draft frontmatter `attachments:` list contains case-relative paths such as
`files/screenshot.png`. Validation and compose check that each path is relative,
safe, and points to an existing file under the case workspace. Adding or editing
attachments changes the draft, so run `draft validate` and `compose` again
before pushing outbound effects.

## Direct Message Archive Categories

The canonical membership file for a direct-message archive category is
`archive/notifications/<archive_uid>-<name>/data/archive.json`:

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

`summary` is optional. Generated `archive.md` renders a Markdown list using
`archive.message_index` config. The built-in archive templates display the
message subject when a summary field is empty. Generated message views live
under `archive/notifications/<archive_uid>-<name>/views/messages/<message_id>.md`.

## Generated Read-View Templates

Built-in MiniJinja Markdown templates render generated read views and
human-facing scaffolds. A workspace can override language-specific templates
under `.afmail/templates/<language>/`; generic `.afmail/templates/<key>` files
are ignored.

Common generated-view template keys include:

- `case/case.md.j2` and `case/message.md.j2`
- `archive-message/archive.md.j2` and `archive-message/message.md.j2`
- `triage/view.md.j2` and `message/section.md.j2`

Run `afmail render templates` to export built-ins, then `afmail render refresh`
to rebuild generated Markdown after template edits.

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
