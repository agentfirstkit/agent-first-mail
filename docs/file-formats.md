# File Formats

This document describes the afmail v2 archive-oriented disk model.

Persisted JSON state documents use `schema_name` plus `schema_version` to
identify their on-disk format. Agent-First Data protocol outputs and audit
events use `code` instead.

## Workspace Identity

`.afmail/workspace.json` is durable, private-Git-trackable identity:

```json
{
  "schema_name": "workspace_identity",
  "schema_version": 2,
  "layout_version": 2,
  "workspace_uid": "workspace_0123456789abcdef0123456789abcdef",
  "created_rfc3339": "2026-07-11T08:00:00Z"
}
```

The UID is `workspace_` plus 32 lowercase hex digits generated from 128 bits of
secure randomness. Init is idempotent and never changes the UID or creation
time. Parsing is strict, including unknown fields. Missing and invalid identity files return
`workspace_identity_missing` and `workspace_identity_invalid`; afmail never
silently generates a replacement for a non-empty existing workspace. Preserve
this file when moving, cloning, or restoring the private mailbox repository.
A v1 identity or other legacy layout returns `workspace_upgrade_required` from
ordinary commands. Run `afmail init` once to migrate; runtime readers do not
support both layouts.

## Private Git Restore Checks

afmail does not have a separate backup or restore format. A private Git clone,
fetch, or worktree move is expected to preserve the workspace files directly.
After restoring that repository on the same or another machine:

1. Run `afmail status` in the workspace and verify the `workspace_uid`,
   readiness fields, object counts, and `storage.*_bytes` totals look like the
   intended mailbox.
2. Run `afmail doctor` before writing new local state or confirming remote
   pushes. Doctor reports canonical file problems, incomplete transactions,
   pending idempotency request records, contact index conflicts, and retained
   push/audit recovery state.
3. Resolve reported issues using the existing doctor and push recovery flows.
   Do not assume that deleting files from the restored worktree removes mail
   evidence from Git history or from other clones/backups.

## IMAP Body Fetch Safety

`imap.allow_unbounded_body_fetch` defaults to `false`. Before a mutating pull,
afmail probes a separate read-only connection with a one-byte RFC partial BODY
fetch. Supported servers use bounded chunks and reject a lying oversized
literal before parsing or persistence. A server that closes/rejects this probe
fails with `imap_partial_fetch_unsupported` and zero local changes.

Some compatibility servers, notably GreenMail, do not implement partial BODY
ranges. Setting `imap.allow_unbounded_body_fetch: true` explicitly permits the
fallback that fetches one declared-in-limit literal at a time and checks actual
bytes afterward. This preserves interoperability but trusts the server not to
lie so severely that one response exhausts memory; enable it only for a trusted
server. It never disables count/bytes budgets or the declared/actual skip.

## Message JSON

Raw message evidence lives at `.afmail/messages/<message_id>.eml`. Canonical
message metadata lives beside it at `.afmail/messages/<message_id>.json`; this
single document owns remote IMAP state plus the current Agent/human suggestion
round. Parsed root `messages/<message_id>.json` files are rebuildable projections
with `schema_name: "message"` and `schema_version: 2`; they expose the canonical
suggestions at the message top level alongside parsed headers, attachment
metadata, `body_text`, remote and workspace overlays, contact links, and identity
matches.

Shared reads parse canonical EML/metadata evidence and apply workspace, identity,
and contact projections in memory without rewriting this cache. Pull, explicit
mutation/repair, and `afmail render refresh` are the materialization boundaries.

The `contact` field is present only when the `From` address matches a contact
card. It contains the exact typed `ContactFrontmatter`, not a separately shaped
UID/name snapshot, and is refreshed at pull/render time and on contact changes:

```json
{
  "contact": {
    "kind": "contact",
    "contact_uid": "p20260521001",
    "display_name": "Zhang San",
    "emails": ["zhangsan@example.com"],
    "phones": [],
    "organization": "Example",
    "role": "Customer",
    "tags": [],
    "created_rfc3339": "2026-05-21T10:00:00Z",
    "updated_rfc3339": "2026-05-21T10:00:00Z"
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

Canonical message metadata:

```json
{
  "schema_name": "message_metadata",
  "schema_version": 2,
  "message_id": "message_20260521_3af9c1b2e8d04f6a",
  "remote": {
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
  },
  "case_relationship_suggestion": {
    "case_uids": ["c20260521001"],
    "evidence": "reply_headers_matched_local_case"
  },
  "agent_suggestion": {
    "agent_suggestion_uid": "agent_suggestion_01",
    "message_review_base_hash": "52aca0f5",
    "suggested_action": {"kind": "case_add", "case_ref": "c20260521001"},
    "reason": "This continues the existing case.",
    "confidence_percent": 94,
    "created_rfc3339": "2026-06-01T17:32:00Z"
  }
}
```

`human_suggestion`, when present, must bind the current
`agent_suggestion_uid` and the same review hash. `accepted` copies the complete
Agent action, `changed` stores a complete replacement action, `rejected` stores
no action and requires a reason, and `deferred` stores no action. A new Agent
suggestion or explicit clear removes the prior human response.

Triage message:

```json
{"workspace": {"status": "triage"}}
```

Local discard dispositions are canonical in object directories, not per-message
state sidecars:

```json
{
  "schema_name": "spam",
  "schema_version": 2,
  "collection_uid": "spam",
  "collection_name": "spam",
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

Other local statuses include `case`, `spam`, `trashed`, `push_queued`, and
`deleted_remote`. The parser also accepts `triage`, `notification`, and
`archived` as workspace statuses, even though the materialized
case/disposition views do not emit them.

Generated read views for negative dispositions live at `spam/index.md`,
`spam/<message_id>.md`, `trash/index.md`, `trash/<message_id>.md`,
`deleted/index.md`, and `deleted/<message_id>.md`. Fresh workspaces do not
create empty disposition views. Existing generated views are rebuildable with
`afmail render refresh` and ignored by the managed `.gitignore`; the durable
message state remains in `.afmail/messages/` and `messages/*.json`.
`afmail purge`, `afmail purge spam`, `afmail purge trash`, and
`afmail purge deleted` are pure previews. They return a `purge_plan` whose ID
binds the immutable workspace identity, selected dispositions, age threshold,
candidate/skipped decisions, and candidate artifact bytes. Supplying that exact
ID through `--confirm PLAN_ID` permanently deletes the reviewed records and
then refreshes the same views; changed state rejects the old plan before any
deletion.

Message attachment metadata lives in the materialized message JSON. Attachments are not
copied into case files by assignment or case creation:

Agent-facing `message show` and generated template message objects mark remote
mail fields with `content_trust: untrusted_remote` and a
`content_provenance` object containing `source: remote_mail`; the stable message
ID remains in its one canonical `message_id` field. Markdown templates consume
canonical fields through output-specific escaping filters, so raw headers stay
available as evidence without being treated as trusted Markdown structure.

```json
{
  "attachments": [
    {
      "part_id": "2",
      "filename": "pricing.txt",
      "content_type": "text/plain",
      "size_bytes": 128,
      "file_path": ".afmail/messages/message_20260521_3af9c1b2e8d04f6a.files/pricing.txt"
    }
  ]
}
```

`part_id` is the MIME part id used by
`afmail message attachment fetch MESSAGE_ID [PART_ID]`. `file_path` is present
only when a local message-cache copy exists under
`.afmail/messages/<message_id>.files/`; there is no duplicate `fetched` flag.

The managed `.gitignore` ignores rebuildable `messages/*.json`, generated
Markdown read views, `.afmail/push/`, `.afmail/logs/`, `.afmail/transactions/`,
`.afmail/requests/`, `.afmail/cache/`, and installed
workspace skill directories such as `.codex/skills/agent-first-mail/` or
`.claude/skills/agent-first-mail/`. It intentionally does not ignore
`.afmail/messages/`. If you track
`.afmail/messages/` in git, the repository will contain private mail bodies,
canonical message metadata, and raw attachment bytes.
`.afmail/cache/` is the only metadata directory that is always safe to remove;
the other ignored directories contain lossy machine-local state.

## Triage View

Generated triage views live at `triage/<message_id>.md` and include YAML
frontmatter with only `kind: triage_view` and `message_id`. Triage views are
rebuildable and are not a notes or suggestion surface. Relationship evidence
lives once in the typed `case_relationship_suggestion` of canonical message
metadata; the template may render it, but neither suggestions, relationships,
parallel IDs, nor derived counts are copied into the built-in frontmatter.
afmail never reads generated Markdown back as state, so workspace templates are
free to replace or omit that frontmatter.

The Agent reads `review_base_hash` from `afmail message show` and calls
`afmail triage suggestion set`; afmail writes the typed proposal to canonical
message metadata and records `agent_suggestion_set`. `suggestion clear` requires
the current `agent_suggestion_uid` and records `suggestion_cleared`, preventing
an old review from clearing a newer round. `afmail triage review` accepts no
caller-owned context file. A submitted review persists `human_suggestion` and
records `human_suggestion_set` before returning its finite result.

Message actions are a closed enum: `case_create`, `case_add`,
`notification_create`, `notification_add`, `spam`, and `trash`. These are
proposals only. Applying one still requires the corresponding canonical domain
command; remote effects retain their separate preview/confirm boundary.

## Case Workspace

Active cases live at `cases/<group>/<case_uid>-<name>/`; archiving and archived
cases live at `archived-cases/<group>/<case_uid>-<name>/`. The `<group>` segment is a case
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
  "schema_version": 3,
  "collection_uid": "c20260521001",
  "collection_name": "应用反馈-肥料登记",
  "tags": [],
  "created_rfc3339": "2026-05-22T09:00:00Z",
  "updated_rfc3339": "2026-05-22T09:00:00Z",
  "items": [
    {
      "message_id": "message_20260521_3af9c1b2e8d04f6a",
      "summary": "Renewal pricing and contract timing.",
      "added_rfc3339": "2026-05-22T09:00:00Z"
    }
  ]
}
```

The collection stores membership once in `items`. It does not persist
`message_count`, `thread_count`, `attachment_count`, or a derived last-message
timestamp. Markdown and Review templates derive counts and ordering from the
contained collection and message documents.

Case-level `agent_suggestion` and `human_suggestion` use
`case_review_base_hash` and a case-specific action enum (`case_archive`,
`case_unarchive`, `case_move`, `case_rename`, `case_tag`, `case_untag`,
`case_trash`, or `case_delete`). They live in this same `case.json`; no separate
suggestion file exists. Notes and drafts remain their own canonical user files
and are never copied into the suggestion envelope.

Active cases have an empty `status`; archiving cases use `status: archiving`
while waiting for queued sends and the independent remote archive push;
finalized cases use `status: archived`. Archived cases set
`archived_rfc3339`; archiving cases do not. Cases do not store notification
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

Draft Markdown files use frontmatter fields `kind`, `case_uid`, `send_intent`,
`reply_to_message_id`, `identity`, `subject`, `to`, `cc`, and `attachments`.
When queued, afmail also manages `sync_intent`, whose only values are
`save_draft` and `send`:

```markdown
---
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
---
# Reply

<!-- afmail:compose:start -->
Thanks, I will send the revised terms today.
<!-- afmail:compose:end -->

On a previous message, Alice wrote:

<!-- afmail:quote:start -->
> Could you send the revised terms?
<!-- afmail:quote:end -->
```

The compose marker pair is part of the current draft schema. Edit only the
content between it; do not delete, duplicate, reorder, or paste another marker
into the authored body. Reply templates keep quoted history in a separate quote
pair after compose. There is deliberately no fallback for pre-schema drafts:
validation returns `draft_invalid` with an upgrade hint until the file is
rewritten once into this format.

`draft validate`, list/show output, and draft review expose
`completeness: complete | incomplete`. Template headings, quoted history, and
identity footers are outside compose and do not make a draft complete. An
incomplete draft may be queued with `draft upload`, but `draft send` returns
`draft_incomplete` without creating or changing a push item. Push validates the
latest draft again before any remote step. Editor-only headings and marker lines
are removed from the generated `text/plain` MIME body.

Draft status is projected from both frontmatter and the matching durable push
item: `editable | queued_upload | queued_send | pushing | failed`. A queued send
is never called `sent`; successful push consumes the draft, and the outbound
message plus audit event record the completed send. Compact `draft list` rows
also expose operational `queued_action` (`save_draft`, `send`, or null) and
`remote_changed: false`. `draft show` and Review instead contain the exact
typed `DraftRenderDocument`: locator data, one canonical
`draft.frontmatter`/`draft.body_markdown`, and output-neutral analysis. They do
not repeat the list projection.

Draft review Save writes the canonical draft locally through the Workspace API,
under an exclusive lock and only when its base hash still matches. Its result
includes the completed local write in `saved_files`; cancelling, dismissing, or
finishing the review does not undo it. Case and notification review follow the
same rule for `notes.md`. Submitting triage, case, or notification review writes
the hash-bound `human_suggestion` to the canonical JSON and records an audit
event, but does not execute the suggested domain action or modify notes or
drafts as part of submission. Review Save never queues, pushes, or sends mail.

`attachments:` is a list of case-relative paths for outbound files. Use
`afmail case draft attach REF DRAFT_NAME PATH` to populate it. External sources
are copied into the case `files/` directory; files already under the case are
recorded as safe relative paths. Inbound message-cache paths under
`.afmail/messages/<message_id>.files/` are message evidence and should not be
used as draft attachment paths.

`identity:` is a config identity slug. If absent, afmail resolves the configured
default identity. Optional footers are inserted outside the compose/quote
sections when the draft is created or its identity changes; send time does not
append hidden text.

`data/drafts.json` is afmail-managed case-local state. It records each draft's
`last_validated_hash` and `last_validated_rfc3339`. Agents and humans edit
draft Markdown directly (or use `draft change`); queued `draft upload` and
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
Free-form notes.
```

`contact_uid` starts with `pYYYYMMDDNNN` and is stable for the life of the contact.
Email addresses are globally unique across all contacts. There is no separate
email→contact index file; the sender→contact link is materialized onto each
message's `contact` field (see Message JSON) at pull/render time and on contact
changes as the exact typed `ContactFrontmatter`, never as a flattened contact
summary. The built-in contact scaffold starts with an empty notes body; any body
text, including a heading added by a workspace scaffold, is canonical
notes content.

`afmail contact show` and `contact notes show` return one exact typed
`ContactRenderDocument`: parsed frontmatter at `contact`, raw body at
`notes_markdown`, and path/group only under `locator`. They do not return the
whole file again as an opaque `content` string or repeat the contact fields.

## Notification Collection

`notifications/<notification_uid>-<name>/data/notification.json` is canonical:

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

Notification-level suggestions live in this same JSON and use
`notification_review_base_hash` with the closed `notification_rename` and
`notification_delete` action enum. As with messages and cases, a new Agent
suggestion clears the prior human response and generated Markdown never owns
this state.

`notification.md` is generated and rebuildable. Its template receives the exact
canonical notification collection plus contained typed message documents. The
template alone chooses summary/subject fallback, fields, labels, ordering,
links, and layout; there is no parallel notification presentation config or
render-data preference projection.
Message views live at
`notifications/<notification_uid>-<name>/views/messages/<message_id>.md` and
preserve the readable message rendering used elsewhere.

## Generated Markdown and Review Templates

One MiniJinja system renders generated Markdown, human-facing scaffolds, and
Review HTML. A workspace can override language entries under
`templates/<language>/` and shared layouts under `templates/shared/`. Legacy
`.afmail/templates/` files are ignored.

Template keys include `case/case.md.j2`, `case/message.md.j2`,
`notification/notification.md.j2`, `notification/message.md.j2`,
`triage/view.md.j2`, `message/section.md.j2`, `status/index.md.j2`,
`status/message.md.j2`, `contact/default.md.j2`, `draft/*.md.j2`,
`notes/*.md.j2`, `workspace/DO_NOT_EDIT.txt.j2`, `review/*.html.j2`, and
`i18n/catalog.j2`. Shared Markdown and Review layouts plus Review CSS live
under `templates/shared/`.

Generated Markdown and Review contain the same typed version-3 render
documents. `document.message` is one exact canonical message record;
output-independent `document.analysis` adds semantic direction, action, quote
ranges, and security conclusions without copying body text. Collection and
Review contexts contain these documents rather than flattening, aliasing, or
rebuilding them. Templates own all display copy, formatting, escaping,
fallback, ordering, visibility, and markup. Template failures return
`template_render_failed`; afmail does not fall back to built-ins when a
selected-language workspace override exists but is invalid.

Review overrides may contain MiniJinja, HTML, and CSS but no JavaScript or
inline event handlers. afmail validates their sources and the composed HTML,
then injects only its trusted behavior runtime. See `docs/templates.md` for the
complete path, data, multilingual, filter, and Review-hook contract.

`afmail render templates` exports the built-in templates for every supported language.
Existing language-specific workspace templates are kept unless `--force` is
used.

## Push Items

Push items live in `.afmail/push/<push_id>.json`. Each item uses
`schema_name: "push_item"` and `schema_version: 2`. Its persisted `kind` is
`draft` (case/draft locator plus optional remote draft UID) or `message` (one
stable message id plus its canonical `action`). The action is queue intent and
is never reconstructed from the message's later workspace status. Resolved
action steps, remote locations, outbound MIME, message bytes, and hashes are
plan-time projections, not editable queue fields. Common durable fields include
timestamps, `attempt_count`, `step_states`, optional `local_stage`, optional
`audit_pending`, and `last_error`. All version-1 push items are rejected; afmail
does not guess a message action that was never persisted.

After all remote steps and local finalize stages succeed, the item first enters
`local_stage: "audit_pending"` and persists:

```json
{
  "audit_pending": {
    "event_id": "event_...",
    "created_rfc3339": "2026-07-11T08:00:00Z",
    "push_kind": "send",
    "case_uid": "c20260711001",
    "draft_name": "reply.md",
    "action": "send",
    "succeeded_step_count": 2,
    "attempt_count": 1
  }
}
```

The fixed event identity makes `push_succeeded` append idempotent. Queue deletion
happens only after the append succeeds. If the caller observes an append/fsync
error even though the line reached disk, retry detects the existing event and
does not duplicate it. A damaged audit tail blocks append with
`audit_log_recovery_required`. Doctor reports retained records as
`push_audit_pending`; recovery never resolves current action config or replays a
remote step.

## Push Plan Output

Bare `afmail push`, `afmail push --dry-run`, and `afmail push list` are pure plan
operations. They return `schema_name: "push_plan"`, `schema_version: 1`, an
immutable `workspace_uid`, canonical `workspace_path`, `config_effect_id`, and
`plan_id: "push_plan_<64 lowercase hex>"`. `push` uses code `push_plan`; `push
list` uses code `push_list`.

Each item exposes its queue-content hash, resolved action/steps and current step
states, effect scope, locations, and errors. Outbound review data includes
identity, to/cc, subject, review path, draft size/hash, exact MIME hash, and each
attachment's filename, size, path, and content hash. Append-style message actions
include the exact source EML size/hash. `effect_scope` is `remote` or
`local_recovery`; the latter is used for `audit_pending` only.

The canonical digest binds workspace UID, effect-relevant configuration, every
raw queue record, resolved/skipped items, drafts, deterministic MIME, attachments,
and appended message bytes. Password values and password-environment variable
names are excluded because they authorize transport but do not change the
reviewed mailbox effect. Identity and action configuration are included.

Only `afmail push --confirm PLAN_ID` applies the queue. A missing argument is a
CLI parse error (exit `2`). A stale or foreign ID returns `push_plan_changed`
before reconcile, remote-client creation, IMAP, or SMTP; details include both IDs,
workspace identity/path, and `remote_calls_started: false`. A confirmed result
uses `code: "push_result"`, echoes the plan/workspace identity, and reports
succeeded, failed, partially applied, audit-pending, reconciled, and remaining
items. Outbound reply send defaults mark the replied-to message with `\Seen` and
`\Answered`; adding a message to a case does not mark remote mail as seen by
default.

When a case with queued send drafts is archived locally, the archive move is a
separate message push item persisted with `kind: "message"`. The case stays `status: archiving` under
`archived-cases/` until outbound pushes have removed their local drafts and the
archive push has succeeded, then afmail writes `status: archived` and its
final timestamp.

## Local Transactions

Incomplete local writes are recorded under
`.afmail/transactions/<transaction_id>.json` while afmail updates related local
files. Successful operations remove the transaction file. If one remains,
writers stop and `afmail doctor` reports `transaction_incomplete`. A recoverable
`case_archive` transaction is resumed by `afmail doctor repair --confirm` after
the underlying validation error has been corrected. Transactions that cannot be
recovered unambiguously remain blocked for manual inspection.

## Status Storage Summary

`afmail status` includes a `storage` object with local byte counts so agents can
surface mailbox workspace growth without adding another command. The fields are
best-effort local filesystem totals, not a Git object-size report:

- `raw_bytes` — direct files under `.afmail/messages/`, including raw `.eml`
  evidence and canonical message metadata/state files.
- `cache_bytes` — rebuildable parsed message cache under `messages/` plus
  `.afmail/cache/render-cache.json`.
- `attachment_bytes` — fetched inbound `*.files/` attachment content and case
  `files/` directories used by drafts.
- `view_bytes` — generated Markdown read views in triage, status folders,
  case views, archived-case views, and notification views.
- `audit_bytes` — `.afmail/logs/`.
- `queue_bytes` — `.afmail/push/`, `.afmail/requests/`,
  `.afmail/transactions/`, and `.afmail/cache/progress.json`.
- `total_bytes` — the sum of the categories above.

## Idempotent Request Records

Supported non-idempotent local mutations may be run with global
`--request-id ID`. afmail stores `.afmail/requests/<request_id>.json` with
`schema_name: "idempotency_request"` and `schema_version: 1`.

Supported commands include triage, case, and notification suggestion `set` and
`clear`. A caller that may retry one of these commands after interrupted output
should reuse one request ID only for that exact normalized input.

The record stores the command key, a BLAKE3 hash of normalized inputs, status
`pending | completed`, timestamps, and the completed command result. It does not
copy raw note text or draft body text into the record; body-like inputs only
contribute to the input hash. Reusing the same request id with the same input
returns the stored result without repeating the local effect. Reusing it with
different input returns `idempotency_conflict`. A pending record returns
`idempotency_incomplete` so agents do not duplicate a possibly completed local
effect after a crash or interrupted output path.
`afmail doctor` reports pending records as `idempotency_incomplete`; review the
workspace effect before retrying with a new request id.

## Workspace Progress

The latest long-running `pull` or confirmed `push` writes a runtime snapshot to
`.afmail/cache/progress.json`. It uses `schema_name:
workspace_progress`, `schema_version: 1`, command/status/phase timestamps,
phase-specific `fields`, and final `result` or `error` summaries. While a
command is active, `status` is `running`; its final value is `succeeded`,
`partial`, or `failed` and corresponds to the terminal event and process exit
code.

At `phase: "finish"`, `fields.success` is `true` only for `succeeded` (it is `false` for both
`partial` and `failed`). `result` is a scalar summary of a returned command
result. `error` is `null` after success; partial/failed aggregate results record
an error summary, while an `AppError` failure records `code`, `message`, and
`retryable`. For a batch or push failure,
`error` may project one representative item envelope; the complete per-item
arrays and push `audit_warnings` remain in the command result. Read the snapshot
with `afmail status`; it is a progress surface, not an audit log. A final
`partial` status corresponds to exit code `3`, not success.

## Pull Cursors

`.afmail/pull/cursors.json` is an afmail-managed optimization file. It uses
`schema_name: "pull_cursors"` and `schema_version: 1`, keyed by configured
mailbox id. Each entry records the resolved `mailbox_name`, `uid_validity`,
`highest_uid`, count of pulls since the last full reconcile, and update time.

Cursor hits let a normal subsequent pull request only UIDs after `highest_uid`.
Cursor absence, corruption, schema mismatch, mailbox rename, UIDVALIDITY reset,
or the periodic full-reconcile interval safely falls back to the full scan path.
Dry-run pulls do not update this file. A pull that skips a UID because of budget
or body-fetch limits does not advance the cursor beyond the skipped UID.

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
- `case_archiving`
- `case_archived`
- `case_unarchived`
- `case_renamed`

Audit events are private mailbox evidence. Each record includes stable audit
metadata such as `event_id`, `created_rfc3339`, `actor`, `kind`, `targets`, and
the caller-supplied `reason` when the command requires or accepts one. Event
payloads also persist operation metadata needed to explain and recover the
change, such as message/case/contact/notification/draft ids, case or
notification names, summaries, contact display names, email addresses, groups,
tags, push ids, remote step counts, and error envelopes.

Audit events do not intentionally copy full message bodies, draft bodies,
attachment bytes, notes bodies, or credential values. Those contents live in
their primary evidence files (`.afmail/messages/`, draft Markdown, `notes.md`,
attachments, or config secrets) and may still be present in the private
workspace repository according to the mailbox evidence trust boundary. Treat
`--reason` text and short summaries as durable audit metadata; use `notes.md`
only when the user wants long-term working memory.
