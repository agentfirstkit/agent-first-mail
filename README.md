# Agent-First Mail

Let your AI agent work your inbox — email pulled into plain files it reads, sorts, and drafts on your machine, with nothing sent until you confirm.

> **Ask your agent:** "Sort this week's inbox and draft replies to anything urgent."

Agent-First Mail — the `afmail` command — isn't an email client, a helpdesk app,
or an auto-replying bot. It's the **workspace your agent operates in**. It borrows
the `git` shape — pull from a remote, work in a local worktree, push deliberately
— so there is always a clear line between *reading your mail* and *changing it*.

Supported platforms: macOS, Linux, Windows.

## Try the complete workflow locally

Create an ordinary workspace connected only to afmail's loopback mock mail
server:

```bash
afmail demo init
cd afmail-demo
afmail demo serve
```

Leave `demo serve` running, then ask your agent to pull and triage the inbox,
draft replies, and show the push preview before confirmation. The focused
`founder-inbox` scenario contains 12 messages, including a reproducible product
bug, receipts, an outage, a VIP request, phishing, and a fake invoice.

This is the real workflow: pull and push use the normal IMAP/SMTP paths, and a
remote effect still requires the exact `afmail push --confirm PLAN_ID` returned
by the preview. The server listens on `127.0.0.1` only. Restarting it resets its
in-memory mailbox; remove the generated `afmail-demo/` directory and run
`afmail demo init` again for a completely clean workspace.

## Build features

Default builds include all three optional product surfaces:

- `demo`: the loopback mock IMAP/SMTP server and embedded demo mailbox.
- `api`: the Axum HTTP API server and OpenAPI export commands.
- `review`: interactive local and LAN Review sessions.

Build a core mail-workspace binary without them using
`cargo build --no-default-features`. Enable only selected surfaces with, for
example, `cargo build --no-default-features --features api,review`. Commands
whose implementation was excluded remain discoverable and return a structured
`feature_disabled` error naming the required feature.

## How it works: one email, start to finish

<video src="https://github.com/user-attachments/assets/a7f9ee4d-3729-41e7-811c-74590d963782" controls muted playsinline></video>

You don't run afmail — your agent does. You ask for what you want in plain
language, and afmail is the tool it reaches for. The commands below are the
agent's tool calls, not something you type.

Say a mailbox is already configured (one folder per account, like a git
worktree). You ask:

> **You:** Check my latest email, go through it, and handle anything urgent.

The agent decides how to do that on its own. It starts by investigating — all
read-only and within the explicit request to check current remote mail: it pulls the mailbox into local files, sees
what came down, opens a **case** for the refund request, and reads what it
actually says:

```bash
afmail status                                            # local counts and latest pull/push progress
afmail push list                                         # pending real-mailbox effects
afmail pull                                              # bounded remote read → local files
afmail triage list                                       # locators for what came down
afmail case create --name "Refund request" --group support \
  --message message_inbox_88213_4 --summary "refund request for late order" --reason "wants a refund"
afmail message show message_inbox_88213_4                # read the request: order #4471, arrived late
```

Fresh pulls default to the last 90 days, at most 500 new messages and 512 MiB
across all selected mailboxes. Use `pull --dry-run` to estimate candidates
without downloading bodies or changing workspace bytes; use `--all-history`
only for an explicitly reviewed full-history import. Explicit
`--max-new-messages` and `--max-total-bytes` remain hard command-wide budgets.

The mailbox workspace is designed to be kept in your own private Git repository.
Raw `.eml` evidence, canonical metadata, generated read views, attachments, queued effects,
and audit files are normal workspace state that private Git can version, sync,
and back up. That is a Git trust boundary, not an encryption boundary: anyone
with repository access, host backups, local machine access, or retained Git
history may still be able to read the mail evidence. Deleting files from the
worktree does not erase old commits. Service credentials are different from mail
evidence; private Git support for mailbox state does not mean passwords or API
secrets should be committed.

To restore or move a mailbox workspace, clone or fetch the private repository,
then run `afmail status` from the workspace to verify the `workspace_uid`, local
counts, and `storage.*_bytes` summary. Run `afmail doctor` before making new
changes or pushing remote effects; it checks canonical files, incomplete local
transactions, pending idempotency records, contact index conflicts, and retained
push/audit recovery state. This is a validation flow over the existing
workspace, not a separate restore command.

Reading the email tells it what's being claimed. To check whether the claim holds,
it steps outside afmail to **another skill** — your store's transaction system —
and looks the order up. That's still read-only, so it still needs no permission.
afmail neither knows nor cares what that skill is: it holds the mailbox while your
agent brings the rest of the toolbox.

```text
# the agent's store / transaction skill — not afmail:
get order #4471      → 9 days late, $79.90, refund-eligible
```

Now, before changing anything, it comes back with what it found and what it
recommends:

> **Agent:** One email needs a decision. A customer wants a refund on order #4471.
> I pulled it up: it arrived 9 days late, it's $79.90, and it's refund-eligible.
> I'd refund the full amount, reply to confirm, then archive the thread. Go ahead?
>
> **You:** Ok, go ahead.

With that approved, the agent issues the refund — which never touches afmail, just
the store's own API:

```text
refund order #4471   → refunded $79.90 to original card (transaction rf_8c21)
```

With the money back, the agent turns to afmail. It drafts the reply confirming
the refund and queues it — still local, nothing sent yet:

```bash
afmail case draft reply c20260606001 message_inbox_88213_4 --body-file refund-reply.txt
afmail case draft send c20260606001 reply-message_inbox_88213_4.md # queue the reply
```

Replies go out under an explicit send identity you've configured, and any persona
footer is written into the draft for review rather than appended secretly at send
time.

If the queued reply needs correction before sending, edit the draft Markdown or
use `draft change`; the queued item still points at the latest draft content:

```bash
afmail case draft change c20260606001 reply-message_inbox_88213_4.md --body-file revised-reply.txt
```

Edits are allowed until preview. Once `afmail push` returns a `plan_id`, changing
the draft, attachment bytes, identity, queue, or action configuration invalidates
that reviewed plan; afmail requires a new preview instead of silently sending
newer content under an older approval.

Draft Markdown has a marked compose section so afmail can distinguish the text
you wrote from template headings, quoted history, and the identity footer. An
unfinished draft may be uploaded to the remote Drafts mailbox, but it cannot be
queued for sending until that compose section contains authored text. The local
state is then `queued_send`, not `sent`; afmail revalidates the latest file at
push time and strips editor-only markers from the outgoing plain-text message.
Drafts accept `To`, `Cc`, and `Bcc` recipients. Bcc addresses remain visible in
the local draft, Review UI, push preview, and the sender's private Drafts/Sent
evidence, but afmail removes the `Bcc` header from SMTP DATA while keeping those
addresses in the SMTP envelope.

Pushing is the moment your real mailbox changes. Bare `afmail push` is a pure
preview containing the complete resolved queue and a workspace-bound `plan_id`;
only `--confirm PLAN_ID` can apply that exact snapshot. So the agent stops and
shows you the draft and all other effects before anything goes out:

> **Agent:** Refund's done. Here's the reply before I send it:
>
> > Hi — sorry your order arrived late. I've refunded the full $79.90 to your
> > original card; it should land in a few days. Thanks for your patience.
>
> Send it and archive the thread?
>
> **You:** Looks good — send it.

Only now, with the draft approved, does it push:

```bash
afmail case archive c20260606001 --reason "refunded order #4471, replied to customer"
afmail push                                              # review the full plan and copy plan_id
afmail push --confirm push_plan_<REVIEWED_PLAN_ID>       # apply only that exact plan
```

If the case still contains queued send drafts or a remote archive effect,
`case archive` returns canonical `status: archiving` and queues the remote archive as
its own push item. Its workspace is held under `archived-cases/`, but it does
not become `status: archived` or receive `archived_rfc3339` until the send and
archive pushes both succeed.

If a confirmed push exits with code `3` and `outcome: partial`, do not blindly
repeat the whole push. Inspect `failures`, `partially_applied_count`,
`remaining_count`, and `audit_warnings`, then compare `afmail push list` with
`afmail status` before deciding whether another confirmed push is safe. A
`remote_outcome_unknown` item must never be retried automatically. If a remote
effect succeeded but its audit append did not complete, the queue retains an
`audit_pending` item. The next preview marks it `effect_scope: local_recovery`;
after review, confirming that new plan only completes the idempotent audit append
and queue cleanup, and never repeats SMTP or IMAP effects.

Then it reports back:

> **Agent:** Sent and archived. That was the only message that needed a reply.
> Here's everything else in the inbox and what I'd do with each:
>
> - **"Your account is suspended — verify now"** — phishing. The link points to `secure-paypa1.com` (a look-alike, "1" in place of the "l"), not the real site, and it pressures you to log in. I'd mark it junk.
> - **"RE: unpaid invoice"** with a `.zip` attachment from a Gmail address, no prior thread — almost certainly malware. Junk.
> - **"Order #4490 is out for delivery"** — a routine shipping notification from the store, nothing to do. Archive.
> - **"Payout of $2,140 sent to your bank"** — your weekly Stripe payout receipt; worth keeping for records. Archive.
>
> Want me to junk the first two and archive the rest?

That is the whole shape: **you ask in plain language and the agent does the work
in local files — investigating, proposing a plan, drafting — and nothing reaches
your real mailbox until you've reviewed the resolved plan and it runs
`afmail push --confirm PLAN_ID` with that exact ID.**

The exact permission boundary for local reads, remote reads, local changes,
remote effects, and permanent deletion is the
[authorization contract](docs/authorization.md). Loading the afmail skill is
automatic when an installed agent recognizes a mail task; loading it grants no
additional permission.

## Nothing touches your real mailbox until you confirm

Everything your agent does by default is local: replying writes a draft,
archiving moves a local folder, marking spam/trash removes it from triage and
adds it to generated `spam/` or `trash/` review views, and remotely deleted mail
is retained under generated `deleted/` views. `afmail purge` only previews the
old local copies it would permanently remove; deletion requires rerunning it
with the exact returned `--confirm PLAN_ID`. Local filing only *queues* a remote effect. `afmail push` is like `git push` — it
previews everything pending, and **`--confirm PLAN_ID` is the one explicit
plan-bound moment** that reviewed snapshot is sent, moved, or flagged. Any
relevant change makes the old ID fail before remote access.
Every confirmed effect lands in an append-only audit log, so a suggestion is never
mistaken for a change after the fact.

## Bring your own skill: afmail is the workspace, your agent is the brain

afmail deliberately does not classify your mail, decide what matters, or write
your replies. It gives **any agent skill a stable, file-first mailbox to operate
on**, safely behind the push boundary — so you compose the behavior you want on
top of it:

- Drop in a custom **skill** that reads the workspace, triages by your rules,
  summarizes threads, suggests cases, and drafts replies in your voice. Its
  output stays local until you push.
- Put your mailbox **policy in `AGENTS.md`**: priorities, reply style, escalation
  rules, labels, who gets a fast response.
- afmail ships an embedded [Agent Skill](skills/agent-first-mail/SKILL.md) that teaches
  an agent its safe behavior contract. Your own skill stacks on top of it, not
  against it.

afmail stays deliberately small — it's the mailbox substrate, and the
intelligence is whatever skill you point at it.

## What your agent gets to work with

A pull leaves behind a workspace that keeps active attention, finished work, and
machine evidence in separate places:

- **`triage/`** — readable Markdown views of new mail, to decide what needs
  attention.
- **`cases/`** — a folder per ongoing issue, holding notes, the messages
  involved, and draft replies.
- **`archived-cases/`** — both archiving cases waiting for confirmed effects and
  fully archived cases, distinguished by canonical `collection.status`.
- **`notifications/`** — completed notification mail filed out of the way.
- **Stable refs** — every message, case, and notification collection has a durable id,
  so an agent can point at the same thing across runs.
- **Locator lists** — `triage list`, `case list`, `notification list`,
  `case list`, and `case list --archived` return
  compact stable ids plus path templates; the agent expands those templates or
  uses the matching `show` command to read detail.

That is the shape. The [docs](#docs) cover the exact files, fields, and commands.

## Review with your agent

Review commands are blocking, local human-in-the-loop sessions for work that
benefits from dense reading or an explicit decision. An agent starts one in the
background. Before triage review, it saves one complete typed proposal with
`afmail triage suggestion set`; the proposal lives in the message's canonical
JSON metadata, never in Markdown or a separate review file. Review reads those
persisted proposals and, on submit, writes a hash-bound `human_suggestion`
before returning its finite result. It never files a message, changes a case,
queues a push, or touches the remote mailbox.

Case and notification suggestions use the same lifecycle in their canonical
JSON through `afmail case suggestion set|clear` and
`afmail notification suggestion set|clear`. Notes and draft fields remain
canonical user content and autosave through the Workspace API under an
exclusive lock and base-hash check; each result records those writes in
`saved_files`. Review never pushes or sends mail. A new Agent suggestion
replaces the previous round and clears its human response. Cancel or window
dismissal leaves suggestion state unchanged. Deletion proposals that set
`allow_notes: true` require a separate destructive confirmation in Review.

All review surfaces expose one top-right Done button. Case, notification, and
draft Done is enabled only after every edit is saved; dismissing the window
never rolls back an autosave. Triage uses every persisted Agent suggestion by default and only
shows a “Message to agent” text box when the user opens “Disagree or write to
the agent”. Notes and drafts show no save badge on open; save progress begins
only after an edit, and `Saved locally` appears only after the canonical file
write succeeds. A save error keeps Done disabled.

`afmail case review CASE_REF` opens the case and its notes;
`afmail notification review NOTIFICATION_REF` opens a notification and its notes;
`afmail case draft review CASE_REF DRAFT_NAME` opens one draft. Submit, cancel,
or dismiss each session returns one structured result with its outcome, base
hashes, and any `saved_files`. The agent must re-check triage hashes before
applying a decision. The persisted human suggestion is feedback, not proof that
the formal action ran and not authorization for push or send; those still use
the normal commands and preview/confirm flow.

If a suggestion `set` or `clear` may be retried after interrupted output, use a
stable `--request-id` for that exact command and inputs. Case and
notification review hashes include every canonical document visible to Review.
Message hashes include the exact message context and related messages; case
hashes include the exact collection, contained messages, current notes, and
active drafts; notification hashes include the exact collection, contained
messages, and current notes. Changing any of them makes an older proposal stale
and requires a fresh suggestion.

Use `--mode lan` with any review command to use a phone or another browser on
the same trusted IPv4 local network. afmail prints a clickable, one-time
`review_url`, waits for that review to finish, and expires the URL after 15
minutes. Review mode defaults to `local`; both modes emit `review_ready` when
the review surface is ready. LAN mode uses local HTTP and must not be used on
public or untrusted networks.

Review HTML uses the same MiniJinja system and the same typed render documents
as generated Markdown. Export editable multilingual templates with
`afmail render templates`; language entries live under `templates/<language>/`
and shared layouts under `templates/shared/`. Workspace Review templates may
provide HTML and CSS but never JavaScript. See the [template
contract](docs/templates.md).

## Typed local HTTP API

Trusted Agent runtimes and applications can use afmail through the same domain
core over HTTP. `afmail api serve` publishes OpenAPI 3.2 plus standalone JSON
Schemas and reuses the CLI's workspace locks, transaction guard, persistent
idempotency records, suggestion hashes, audit behavior, and push plan
confirmation. It does not expose a shell or generic command endpoint.

Set a random 32–512-character bearer-safe ASCII credential, then start the API
from a mailbox workspace:

```bash
export AFMAIL_API_ACCESS_TOKEN_SECRET='replace-with-a-random-32-character-value'
afmail api serve --mode local
```

The ready event prints the API and OpenAPI URLs without printing the credential.
Use `--mode lan` only on a trusted IPv4 local network. For arbitrary-network
access, keep loopback mode and use an established encrypted tunnel or TLS
reverse proxy instead of exposing afmail directly. Human `review` remains an
afmail-owned interactive UI action; OpenAPI does not replace it with a generic
HTTP command.

See the [HTTP API contract](docs/api.md).

## Adopt it: hand afmail to your agent

The fastest way to get going is to let your agent read what afmail is and set it
up for you. Paste this to your agent:

> Read what Agent-First Mail is at https://agentfirstkit.com/agent-first-mail and
> tell me in plain terms what it would do for me. If I want it, install it — the
> prebuilt package for the quick path, or build from source after a quick security
> review of the repo if you'd rather read what you run — then run `afmail skill
> install` so you follow its behavior rules. Then set up my mailbox: afmail uses
> one workspace directory per account, so make a folder, run `afmail init` in it,
> and help me connect my mailbox.

If it's a fit, install it — a prebuilt package, or from source if you want to
read it first:

```bash
# prebuilt binary
brew install agentfirstkit/tap/afmail   # macOS / Linux
scoop bucket add agentfirstkit https://github.com/agentfirstkit/scoop-bucket && scoop install afmail   # Windows

# or build from source after reviewing the repo
git clone https://github.com/agentfirstkit/agent-first-mail
cd agent-first-mail
cargo install --path . --bin afmail --locked --force
```

Then install the embedded [Agent Skill](skills/agent-first-mail/SKILL.md) so the agent
follows afmail's behavior rules, and restart the agent afterward so it reloads
them:

```bash
afmail skill install --agent codex --scope workspace
afmail skill status --agent codex --scope workspace
```

Then it's **one workspace directory per mailbox**: run `afmail init` in a fresh
folder or `afmail init email-a` from your agent root, connect the account, and
your agent works that mailbox from its files.
`init` creates only mailbox-workspace files; it never installs a Skill or edits
the Agent root's `AGENTS.md`. Its structured `created_files`, `changed_files`,
and `next_steps` fields drive setup without guessing config keys. After applying
those steps, use `afmail status` for local readiness and `afmail remote test`
for IMAP plus configured SMTP handshake/authentication. The remote test sends no
mail.
See the [Workspace Model](docs/workspace.md).

## Docs

- [Core Design Principles](docs/design-principles.md) — what afmail guarantees, and why
- [Workspace Model](docs/workspace.md) — the folder layout and what each part is for
- [File Formats](docs/file-formats.md) — the exact files a pull and your commands produce
- [HTTP API](docs/api.md) — OpenAPI discovery, authentication, idempotency, and network modes
- [CLI Contract](docs/cli.md) — every command and flag for the `afmail` binary
- [Code Structure](docs/code-structure.md) — where crate responsibilities live
- [Agent Skill](skills/agent-first-mail/SKILL.md) — the behavior contract agents follow

## License

MIT
