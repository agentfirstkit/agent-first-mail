# Message channels and file input/output

The workspace processes messages, cases, drafts, review decisions, and confirmed
publication. An adapter owns receiving and delivery details. Email adapters keep
MIME, SMTP envelopes, authentication facts, and IMAP locations. A native channel
keeps its original JSON and a typed reply route instead.

`MessageFile.origin` contains an external message id and a reply route with
`channel`, `source_id`, and `conversation_id`. Common subject, participant, and
body fields remain in the message itself. Native participant ids are opaque
strings; they need not be email addresses. Markdown and Review consume the same
typed message document and show the source and conversation. Native records do
not acquire email authentication warnings or invented IMAP locations.

## Configure the file adapter

File channels are included in default builds. A headless file-only build can use
`cargo build --no-default-features --features files`. Add `review` if needed.
No SMTP, Cloudflare, or network credentials are required for this adapter.

Create separate input and output directories, then configure a named source:

```bash
afmail config set files.sources '{"inquiries":{"inbox_path":"input","outbox_path":"output","max_message_bytes":1048576,"max_input_files":10000}}'
```

Paths are relative to the workspace unless absolute. Configuration owns paths;
input records cannot supply an output path, URL, or source id. An inbox or
outbox may not be the workspace root or lie inside a directory afmail manages
(`.afmail`, `messages`, `triage`, `cases`, `archived-cases`, `notifications`,
`templates`, `spam`, `trash`, `deleted`, `contacts`, `identities`), and no
outbox may be any source's inbox, and no two sources may share an inbox. Relative spellings are normalised when the
config is loaded; absolute, aliased, and symlinked spellings are checked again at
pull and publish time, so neither directory needs to exist yet. A violation is
`config_invalid`. The source id must contain ASCII
letters, digits, underscores, or hyphens, at most 128 bytes.

The producer atomically publishes UTF-8 `.json` files into the input directory
(write a temporary file, then rename it). The first contract supports text:

```json
{
  "schema_name": "channel_message",
  "schema_version": 1,
  "external_message_id": "web/001",
  "conversation_id": "inquiry/42",
  "sender_id": "visitor/42",
  "recipient_ids": ["support"],
  "subject": "Opening hours",
  "body_text": "Are you open on Sunday?"
}
```

An optional `reply_to_external_message_id` relates a native reply to an earlier
external message in the same channel, configured source, and conversation.
It is retained at `message.origin.reply_to_external_message_id` in the typed
message document. `message show`, detailed triage lists, and the shared
Markdown/Review render documents expose explicit parent/reply links through
`related_messages` in both directions. A shared conversation id alone does not create a reply link
or file messages into a case. Message caches contain the complete reply
reference, so indexing does not reopen unrelated native message bodies. If an
unrelated cache is invalid, reads report the problem under
`context.related_message_issues` (or `related_message_issues` on detailed triage
lists) instead of hiding it or making healthy messages unreadable. Relationship
checks used to authorize direct filing/discard fail with
`message_relation_index_incomplete` until the evidence/cache problems are fixed.
The built-in Review page shows related messages as collapsible cards and warns
when the relationship check is incomplete; generated Markdown shows the same
documents and diagnostics.

IDs and subjects must contain 1–512 printable UTF-8 bytes;
unknown fields are rejected. The default input/output limit is 1 MiB per record,
configurable up to 100 MiB. Each inbox accepts at most 10,000 JSON files by
default. Input symbolic links and non-regular files are refused, and reads bound
actual bytes as well as declared size.

```bash
afmail pull --files --dry-run
afmail pull --files
afmail case create --name 'Opening hours' --group support --message MESSAGE_ID --reason 'Answer inquiry'
afmail case draft reply CASE_UID MESSAGE_ID --body 'Yes, from 10 to 4.'
afmail case draft send CASE_UID DRAFT_NAME
afmail push
# Inspect the returned plan before confirming it.
afmail push --confirm PLAN_ID
```

Plain `pull` includes all configured sources. `--files` explicitly selects every
file source; it can be combined with mailbox ids or `--cloudflare`. New-message
and byte budgets apply across selected adapters. Time-window options apply to
IMAP and cannot accompany an explicit `--files` selection. Invalid/conflicting
input records remain available and appear in `files.warnings`. A problem with one
source — a missing or unreadable inbox (`channel_inbox_unavailable`), more files
than `max_input_files` (`channel_inbox_limit`), or an overlapping path
(`config_invalid`) — is also a `files.warnings` entry carrying its `source_id`;
the other sources and adapters still import.

The source id plus external message id determines a stable local message id.
Re-reading identical input creates no duplicate. Reusing an id with different
bytes reports `channel_message_conflict` and preserves the earlier evidence.
Canonical `.afmail/messages/MESSAGE_ID.message` files contain a small evidence
envelope with the source, direction, receipt/message timestamp, and original
UTF-8 JSON in `content_json`. Generated `messages/*.json` can be rebuilt from it.
The input is retained: the producer owns retirement after checking pull results.
Inputs the pull budget left unimported are listed in `files.skipped_inputs` with
their `input_path`; retire neither those nor inputs named in `files.warnings`.
Retire old inputs before purging local messages to prevent reimport.

Replies inherit the source and conversation. The configured identity id becomes
the native sender id. Drafts use the same authored/quoted body contract as email;
published text can include the quoted original. `push` resolves a file reply to
one `file_send` operation, without IMAP Drafts/Sent operations. Its preview shows
the route, output path, and canonical content hash; changes to source settings,
the draft, or attachments invalidate an earlier confirmation plan.

Confirmed publication creates an owner-only (mode 0600 on Unix)
`output/MESSAGE_SENT_ID.json` using an atomic,
no-overwrite hard link from a fully synced temporary file on the same filesystem.
The output is a `channel_message` record, with stable external id, conversation,
recipients, and `reply_to_external_message_id`. Different existing bytes are a
conflict. The workspace retains canonical sent evidence and appends it to the
case before removing the draft. A consumer must retain published output until
local push completion has been confirmed. If publication was interrupted,
matching retained output proves completion. Missing or different output leaves
an unknown outcome and never authorizes automatic re-publication: the consumer
might already have processed and removed the file.

## Adapter capabilities and future channels

The file adapter reports capabilities in pull results: reply and idempotent
publication are available; attachment transfer, input acknowledgement, remote
mailboxes, remote draft upload, reply-all, cc, and bcc are unavailable. Unsupported
operations fail explicitly. Output is local delivery to a configured directory;
it does not prove that an external consumer processed the reply.

A web inquiry producer can already write this JSON contract and consume replies.
Its authenticated user/session id can be a participant id and its inquiry id a
conversation id. The web application remains responsible for identifying users,
authorizing access, rate limits, and durable consumption. A future direct HTTP
adapter can implement that transport using the same routing and case workflow.
Cloudflare DO/R2 details stay inside the email adapter; they do not become a
requirement for file or future web channels.

Run `scripts/test.sh files` for the native workflow, conflicting input, bounded
reads, symlink refusal, and uncertain publication recovery contracts.
