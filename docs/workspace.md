# Workspace Model

An afmail workspace is a local, file-first mailbox workspace. Files are the read
interface; the CLI is the effect interface.

## Demo workspaces

`afmail demo init [PATH]` creates the same workspace layout and ordinary
`.afmail/config.json` used for a real account, but points IMAP and SMTP at the
loopback-only server started by `afmail demo serve`. There is no demo marker,
alternate workspace schema, or disabled-remote mode. Pull, triage, drafting,
push preview, and confirmed push therefore exercise their production paths
without credentials for or connectivity to a real mailbox.

The built-in `founder-inbox` scenario is deliberately focused: it contains 12
messages, within the demo's 10–12-message target. Server state is in memory and
resets whenever `demo serve` restarts. Workspace files do not reset with the
server; create a fresh demo destination when you want both sides clean.

## Layout

```text
account-workspace/
  AGENTS.md  # optional user-authored mailbox policy; init does not create it
  triage/
    <message_id>.md
  spam/  # created when spam messages first exist
    data/
      spam.json
    index.md
    <message_id>.md
  trash/  # created when trashed messages first exist
    data/
      trash.json
    index.md
    <message_id>.md
  deleted/  # created when remote-deleted messages first exist
    data/
      deleted.json
    index.md
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
  contacts/  # created when contacts first exist
    <group>/
      <contact_uid>-<name>.md
  identities/  # optional persona overrides, one <identity>.md per config identity
  archived-cases/
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
  notifications/
    <notification_uid>-<name>/
      notification.md
      notes.md
      data/
        notification.json
      views/
        messages/
          <message_id>.md
  messages/
    <message_id>.json
  templates/
    <language>/
      i18n/
      review/
    shared/
      markdown/
      review/
  .afmail/
    DO_NOT_EDIT.txt
    config.json
    workspace.json
    pull/
      cursors.json
    cache/
      workspace.lock
      progress.json
      render-cache.json
    logs/events.jsonl
    transactions/
    requests/
    push/
    messages/
      <message_id>.eml
      <message_id>.json
      <message_id>.files/
```

`triage/` and `cases/` are active attention surfaces. `spam/`, `trash/`, and
`deleted/` are generated review views for local discard states, created only
after the first matching message appears.
`archived-cases/` contains both archiving case workspaces waiting for queued
sends/archive effects and finalized archived cases. Canonical `status` metadata,
not another directory or CLI flag, distinguishes them.
`notifications/<notification_uid>-<name>/` contains notification messages
in one notification collection.

Case roots contain only user-facing Markdown entry points (`case.md` and
`notes.md`) plus working directories. Notification roots contain `notification.md`
and `notes.md`. Canonical local object state lives under `data/`; generated,
rebuildable Markdown detail views live under `views/`. `drafts/` and `files/`
are user-visible working materials.

`case.md`, `notification.md`, `triage/*.md`, `spam/*.md`, `trash/*.md`,
`deleted/*.md`, and `views/**/*.md` are generated read views. They are safe to
rebuild with `afmail render refresh`; use `notes.md` for durable notes instead
of generated views. Case and notification message links point to
`views/messages/<message_id>.md`.

Suggestions do not change that rule. Message `agent_suggestion` and
`human_suggestion` state lives in `.afmail/messages/<message_id>.json`; case and
notification suggestion state lives in the object's existing canonical
`data/*.json`. Generated Markdown may render a suggestion but never owns it.
Deleting any generated view cannot discard a pending review, and
`afmail render refresh` never reconstructs state from Markdown or audit history.

Show/list/status/push-preview commands never rebuild a missing view as a side
effect. Message, draft, case, and notification `show` return typed documents
from canonical state. Case/notification show contain the exact collection
render document used by Markdown and Review, and omit `locator.view_path` when
that generated file is missing. Use `afmail render refresh` explicitly to
materialize or repair generated views.

`.afmail/DO_NOT_EDIT.txt` is a warning sentinel. The rest of `.afmail/` is
machine-managed evidence, remote state, push queue, and audit history; use the
CLI for effects instead of editing it by hand. Generated Markdown template
overrides live under user-editable `templates/`, outside `.afmail/`. Optional
identity persona files live under user-editable `identities/`, also outside
`.afmail/`.

`.afmail/workspace.json` is machine-managed durable identity, intentionally
tracked with the private workspace. It contains the immutable `workspace_uid`
used to bind plans and must be preserved when cloning, moving, or restoring the
workspace. A missing or damaged identity is an error; afmail never silently
rotates it. Concurrent-process lock files live outside the workspace in the
user runtime temp directory, so the first shared read after a clone does not
change the workspace tree or mtimes.

Persisted JSON state documents identify their on-disk format with
`schema_name` and `schema_version`. CLI stdout, diagnostics, errors, and
`.afmail/logs/events.jsonl` audit events remain Agent-First Data protocol
messages and use `code`.

The managed `.gitignore` intentionally does not ignore `.afmail/messages/`: raw
mail evidence and remote metadata are durable local state. Tracking those files
in git means the repository contains private mail bodies and attachment bytes.
The managed ignore block covers the machine-local `.afmail/push/`,
`.afmail/logs/`, `.afmail/transactions/`, `.afmail/requests/`, and
`.afmail/cache/` directories, plus installed workspace skill directories such as
`.codex/skills/agent-first-mail/` or `.claude/skills/agent-first-mail/`,
`messages/*.json`, `triage/*.md`,
`spam/*.md`, `trash/*.md`, `deleted/*.md`, and generated object Markdown views.
Only `.afmail/cache/` is safe to delete, and only while no afmail process is
running: it holds `workspace.lock`, and removing the lock file under a running
process lets a second writer take a fresh lock beside it. The other ignored state
contains pending intent, recovery sentinels, idempotency records, or audit
history and is machine-local but lossy to remove.

## Message State

Message evidence lives in `.afmail/messages/<message_id>.eml`. Its canonical
metadata—including remote mailbox locations plus the current Agent and human
suggestions—lives in `.afmail/messages/<message_id>.json`. Parsed root
`messages/<message_id>.json` files are rebuildable projections. Triage, case,
notification, spam, trash, and deleted views are generated from message evidence
plus canonical metadata and object collections. Inbound
attachments belong to the message. Attachment metadata is stored on the message
record; `afmail message attachment fetch MESSAGE_ID [PART_ID]` materializes
files under `.afmail/messages/<message_id>.files/` and refreshes generated read
views so fetched paths appear in message renderings.

A message can be referenced by multiple active, archiving, or archived cases. A
message can belong to at most one notification collection. If a message needs
multiple contexts, create or use cases instead of multi-archiving the direct
message.

Local discard dispositions are canonical in their own directories:
`spam/data/spam.json`, `trash/data/trash.json`, and
`deleted/data/deleted.json`. Fresh workspaces omit empty disposition
directories and JSON; once created, those state files may remain if a
collection later becomes empty. The Markdown files under those directories are
generated views.

## Cases

Active cases live at `cases/<group>/<case_uid>-<name>/`. Archiving and finalized
archived cases live at `archived-cases/<group>/<case_uid>-<name>/`. The
`status: archiving` value and missing `archived_rfc3339` identify the temporary
state while queued sends and the archive push finish. The `<group>` segment is the case
group carried forward from active work, not an archive category; for example,
`archived-cases/support/...` is an archived case from the `support` case group.
`case_uid` is globally unique and stable across active, archiving, and archived
cases.
`rename --name` updates `data/case.json` and the readable directory suffix.

Case refs must start with `cYYYYMMDDNNN`; notification refs must start with
`nYYYYMMDDNNN`. A ref may include a readable suffix after one dash, so
`c20260521001-anything` and `c20260521001` are equivalent, as are
`n20260521001-anything` and `n20260521001`. Names alone are not looked up. Group
and tag values are local path-segment identifiers. Human names may use Unicode
such as Chinese, for example `应用反馈-肥料登记` or `服务通知`. Do not use path
separators (`/` or `\`) or the dot-only segments `.`/`..`.

Case metadata is canonical in `data/case.json`. Active cases have an empty
`status`; archiving cases use `status: archiving`; finalized cases use
`status: archived`. Final archived cases carry top-level
`archived_rfc3339`; archiving cases do not set it until finalization. Message
membership is stored in the same `data/case.json` collection under `items[]`,
with per-message `summary` and `added_rfc3339`. Archived cases do not use
notification collections.

Case-local `data/drafts.json` files are afmail-managed machine state. They
record only the last validation hash/time for `drafts/*.md` files. Humans and
agents should edit draft Markdown, not `data/drafts.json`; queued save/send
items reference the draft by case and filename and use the latest content at
push time.

## Contacts

Identities are "us": configured send personas with an address, display name,
and default flag in `.afmail/config.json`. Contacts are everyone else. Multiple
identities may share one email address when the same mailbox needs separate
personas, such as `support` and `sales`.

Contact cards live at `contacts/<group>/<contact_uid>-<name>.md`. `contact_uid`
starts with `pYYYYMMDDNNN`; a ref may include a readable suffix after one dash.
The card is a Markdown file with a `kind: contact` frontmatter (display name,
`emails`, `phones`, organization, role, tags, timestamps) and a free-form notes
body. Each card is canonical state — edit it with `afmail contact` commands, not
by hand.

Email addresses are globally unique across active contacts; `contact create` and
`contact email add` reject an address already held by another contact. The
sender→contact association is materialized onto each message as a `contact`
object (`messages/<message_id>.json`) at pull/render time. That object contains
the exact typed `ContactFrontmatter`; it is not a separate UID/name projection.
It is refreshed automatically whenever the canonical contact changes, and
`afmail render refresh` rebuilds it for every message. There is no separate
contact index file.

## Drafts And Case Files

Draft Markdown lives under a case `drafts/` directory. Outbound attachments
belong to the draft/case, not to inbound message evidence. Use
`afmail case draft attach REF DRAFT_NAME PATH` to add one: external files are
copied into the case `files/` directory with a safe filename, and files already
inside the case are recorded as case-relative paths without another copy.

The draft frontmatter `attachments:` list contains case-relative paths such as
`files/screenshot.png`. Validation and push check that each path is relative,
safe, and points to an existing file under the case workspace. Adding or editing
attachments changes the draft; queued `draft upload` / `draft send` items still
use the latest Markdown content. Use `afmail case draft remove` to delete the
draft and any queued save/send item.

Draft frontmatter may include `identity: <slug>`. New/reply/change commands
accept `--identity <slug>`; when omitted, afmail records or resolves the config
default identity. An identity footer from `identities/<slug>.md` is inserted
outside the draft compose/quote sections immediately so reviewers see the exact
footer before queueing or sending. Send time uses the resolved identity for the
RFC 5322 `From` header and SMTP envelope sender, but it does not secretly append
or mutate footers.

Every current-format draft has exactly one
`<!-- afmail:compose:start -->` / `<!-- afmail:compose:end -->` pair. Edit the
authored text only inside that pair; `draft change --body` does the same without
removing reply history or the identity footer. Reply scaffolds keep remote quoted
history in a separate quote marker pair after compose. Marker lines and template
headings are editor schema, so afmail removes them when it builds the outbound
plain-text MIME body.

An empty compose section is intentionally valid for local editing and remote
Drafts upload, and appears as `completeness: incomplete`. It cannot enter a send
queue: `draft send` returns `draft_incomplete`, leaves the file/queue unchanged,
and push checks completeness again in case a queued draft was edited. Draft
list, show, and typed Review documents share the states `editable`,
`queued_upload`, `queued_send`, `pushing`, and `failed`; `queued_send` never
means the message was sent.

## Notification Collections

The canonical membership file for a notification collection is
`notifications/<notification_uid>-<name>/data/notification.json`:

```json
{
  "schema_name": "notification",
  "schema_version": 3,
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

`summary` is optional. Generated `notification.md` receives this exact
collection plus contained typed message documents. Its template owns
summary/subject fallback, selected fields, labels, ordering, links, and layout;
there is no notification presentation config in canonical or render data.
The collection does not persist derived message, attachment, or last-message
counts; templates derive those values from `items` and the contained messages.
Generated message views live under
`notifications/<notification_uid>-<name>/views/messages/<message_id>.md`.

## Markdown and Review Templates

Built-in MiniJinja templates render generated Markdown, human-facing scaffolds,
and Review HTML. A workspace can override content entry templates under
`templates/<language>/` and shared Markdown layouts under `templates/shared/`.
Legacy `.afmail/templates/` files are ignored.

Common generated-view template keys include:

- `case/case.md.j2` and `case/message.md.j2`
- `notification/notification.md.j2` and `notification/message.md.j2`
- `triage/view.md.j2` and `message/section.md.j2`
- `contact/default.md.j2`, `draft/*.md.j2`, and `notes/*.md.j2`
- `i18n/catalog.j2` for localized copy and punctuation

The Review UI is not part of this tree. It is replaced through an AFUI
frontend at `.afui/frontends/afmail/<ui_kind>/`, which afmail loads only after
`afui frontend enable` trusts it; see `docs/templates.md`.

Run `afmail render templates` to export built-ins, then `afmail render refresh`
to rebuild generated Markdown after template edits. Review loads its templates
when the page is rendered.

Markdown and Review receive the same schema-versioned typed render documents.
Each document contains canonical data once plus output-independent semantic
analysis; it never carries a Markdown-shaped or HTML-shaped display projection.
Body and draft text exist once and semantic byte ranges identify authored and
quoted portions. Templates own localization, fallback text, formatting,
escaping, ordering, visibility, and markup.

Workspace Review templates can use MiniJinja, HTML, and CSS but cannot provide
JavaScript. afmail rejects JavaScript-capable Review overrides and supplies its
own behavior-only autosave/settle runtime. See [Markdown and Review
Templates](templates.md) for all paths, data fields, filters, language packs,
and Review behavior hooks.

## Deleted Remote Messages

When a remote message disappears and has no case/notification/draft/push reference,
afmail keeps the local evidence under `.afmail/messages/`, marks its local
state as `deleted_remote`, and exposes it under generated `deleted/` views.
`afmail purge` previews old local `spam`, `trashed`, and `deleted_remote`
message records; add `spam`, `trash`, or `deleted` to limit it to one
disposition, and use `--older-than-days` to override the default 30-day
threshold. The preview does not write the workspace. Permanent deletion requires
the exact returned `--confirm PLAN_ID`; a changed candidate or artifact rejects
the stale plan before deletion. When a referenced remote message disappears, afmail keeps the
business state such as `case` or `notification` and only marks remote locations
missing so existing case/notification state stays resolvable.

## Notes

`notes.md` files are plain Markdown with no frontmatter. They exist for active
cases, archived cases, and notification collections. They are the only
durable local notes surface inside those objects; generated views are disposable.

Case review automatically saves edited `notes.md` locally after a short idle
period. Draft review does the same for the reviewed draft. Each save uses the
canonical Workspace API under an exclusive workspace lock after a base-hash
check, and the review result records the completed files in `saved_files`.
Triage review only returns typed decisions and does not mutate the workspace.
No review action queues, pushes, or sends mail; closing the window does not undo
an earlier completed local save.
