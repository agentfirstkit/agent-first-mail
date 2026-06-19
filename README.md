# Agent-First Mail

Let your AI agent work your inbox — email pulled into plain files it reads, sorts, and drafts on your machine, with nothing sent until you confirm.

> **Ask your agent:** "Sort this week's inbox and draft replies to anything urgent."

Agent-First Mail — the `afmail` command — isn't an email client, a helpdesk app,
or an auto-replying bot. It's the **workspace your agent operates in**. It borrows
the `git` shape — pull from a remote, work in a local worktree, push deliberately
— so there is always a clear line between *reading your mail* and *changing it*.

Supported platforms: macOS, Linux, Windows.

## How it works: one email, start to finish

<video src="https://github.com/user-attachments/assets/a7f9ee4d-3729-41e7-811c-74590d963782" controls muted playsinline></video>

You don't run afmail — your agent does. You ask for what you want in plain
language, and afmail is the tool it reaches for. The commands below are the
agent's tool calls, not something you type.

Say a mailbox is already configured (one folder per account, like a git
worktree). You ask:

> **You:** Can you go through my email and handle it?

The agent decides how to do that on its own. It starts by investigating — all
read-only, so it needs no permission: it pulls the mailbox into local files, sees
what came down, opens a **case** for the refund request, and reads what it
actually says:

```bash
afmail status                                            # local counts and latest pull/push progress
afmail push list                                         # pending real-mailbox effects
afmail pull                                              # read-only: mail → local files
afmail triage list                                       # locators for what came down
afmail case create --name "Refund request" \
  --message message_inbox_88213_4 --summary "refund request for late order" --reason "wants a refund"
afmail message show message_inbox_88213_4                # read the request: order #4471, arrived late
```

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

Workspaces define explicit send identities in `.afmail/config.json`; optional
`identities/<slug>.md` files add persona notes and footers. Draft commands accept
`--identity <slug>`, and footers are written into the draft for review rather
than appended secretly at send time.

If the queued reply needs correction before sending, edit the draft Markdown or
use `draft change`; the queued item still points at the latest draft content:

```bash
afmail case draft change c20260606001 reply-message_inbox_88213_4.md --body-file revised-reply.txt
```

Pushing is the moment your real mailbox changes, and afmail will not push without
an explicit `--confirm`. So the agent stops and shows you the draft to review
before anything goes out:

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
afmail push --confirm                                    # send the reply and apply the archive in one push
```

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
your real mailbox until you've reviewed what's queued and it runs an explicit
`afmail push --confirm`.**

## Nothing touches your real mailbox until you confirm

Everything your agent does by default is local: replying writes a draft,
archiving moves a local folder, marking spam/trash removes it from triage and
adds it to generated `spam/` or `trash/` review views, and remotely deleted mail
is retained under generated `deleted/` views until `afmail purge` clears old
local copies. Local filing only *queues* a remote effect. `afmail push` is like `git push` — it
previews everything pending, and a single **`--confirm` is the one explicit
moment** all of it is sent, moved, or flagged.
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
- afmail ships an embedded [Agent Skill](skills/agent-first-mail.md) that teaches
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
- **`archive/`** — finished work, filed and out of the way.
- **Stable refs** — every message, case, and archive category has a durable id,
  so an agent can point at the same thing across runs.
- **Locator lists** — `triage list`, `case list`, and `archive list ...` return
  compact stable ids plus path templates; the agent expands those templates or
  uses the matching `show` command to read detail.

That is the shape. The [docs](#docs) cover the exact files, fields, and commands.

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

Then install the embedded [Agent Skill](skills/agent-first-mail.md) so the agent
follows afmail's behavior rules, and restart the agent afterward so it reloads
them:

```bash
afmail skill install --agent codex --scope workspace
afmail skill status --agent codex --scope workspace
```

Then it's **one workspace directory per mailbox**: run `afmail init` in a fresh
folder or `afmail init email-a` from your agent root, connect the account, and
your agent works that mailbox from its files.
See the [Workspace Model](docs/workspace.md).

## Docs

- [Core Design Principles](docs/design-principles.md) — what afmail guarantees, and why
- [Workspace Model](docs/workspace.md) — the folder layout and what each part is for
- [File Formats](docs/file-formats.md) — the exact files a pull and your commands produce
- [CLI Contract](docs/cli.md) — every command and flag for the `afmail` binary
- [Code Structure](docs/code-structure.md) — where crate responsibilities live
- [Agent Skill](skills/agent-first-mail.md) — the behavior contract agents follow

## License

MIT
