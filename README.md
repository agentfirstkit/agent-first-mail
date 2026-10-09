# Agent-First Mail

Let your AI agent work your inbox the way it works a git checkout — mail pulled into local files it reads, sorts, and drafts on your machine, and nothing reaches your real mailbox until you confirm the push.

> **Ask your agent:** "Sort this week's inbox and draft replies to anything urgent."

Agent-First Mail — the `afmail` command — isn't an email client, a helpdesk
app, or an auto-replying bot. It's the **workspace your agent operates in**,
borrowed from the `git` shape: `pull` the mailbox into a local worktree, work
in files, and `push` deliberately. Reading your mail and changing it are two
different commands, and only one of them can touch the server.

Supported platforms: macOS, Linux, Windows. Works with Gmail, Fastmail,
iCloud, and any other IMAP/SMTP mailbox that accepts a password login; see
[Connect a Mailbox](docs/setup.md).

## Try it in a minute

No credentials needed. `demo init` creates an ordinary workspace wired to a
loopback mock mail server with a 12-message founder inbox: a reproducible
product bug, receipts, an outage, a VIP request, phishing, and a fake invoice.

```bash
afmail demo init
cd afmail-demo
afmail demo serve
```

Leave `demo serve` running and ask your agent to pull and triage the inbox,
draft replies, and show you the push preview. Pull and push use the real
IMAP/SMTP paths, and a remote effect still requires the exact
`afmail push --confirm PLAN_ID` the preview returned. Restart the server to
reset it; delete `afmail-demo/` and run `demo init` again for a clean slate.

## One email, start to finish

<video src="https://github.com/user-attachments/assets/a7f9ee4d-3729-41e7-811c-74590d963782" controls muted playsinline></video>

You don't run afmail — your agent does. You ask in plain language, and afmail
is the tool it reaches for. You say:

> **You:** Check my latest email, go through it, and handle anything urgent.

The agent starts read-only: it pulls the mailbox into local files, sees what
came down, opens a **case** for a refund request, and reads what the customer
actually wrote.

```bash
afmail pull                                              # bounded remote read → local files
afmail triage list                                       # locators for what came down
afmail case create --name "Refund request" --group support \
  --message message_inbox_88213_4 --summary "refund request for late order" --reason "wants a refund"
afmail message show message_inbox_88213_4                # order #4471, arrived late
```

To check whether the claim holds, it steps outside afmail to **another
skill** — your store's order system — and looks the order up. afmail neither
knows nor cares what that skill is: it holds the mailbox while your agent
brings the rest of the toolbox. Then it comes back with a recommendation:

> **Agent:** One email needs a decision. A customer wants a refund on order
> #4471: it arrived 9 days late, it's $79.90, and it's refund-eligible. I'd
> refund it, reply to confirm, then archive the thread. Go ahead?
>
> **You:** Ok, go ahead.

The refund goes through the store's own API. Then the agent drafts the reply
under your configured send identity and queues it — still local, nothing sent:

```bash
afmail case draft reply c20260606001 message_inbox_88213_4 --body-file refund-reply.txt
afmail case draft send c20260606001 reply-message_inbox_88213_4.md   # queue it
afmail case archive c20260606001 --reason "refunded order #4471, replied to customer"
```

Pushing is the moment your real mailbox changes, so the agent stops and shows
you the draft and every other pending effect first:

> **Agent:** Refund's done. Here's the reply before I send it:
>
> > Hi — sorry your order arrived late. I've refunded the full $79.90 to your
> > original card; it should land in a few days. Thanks for your patience.
>
> Send it and archive the thread?
>
> **You:** Looks good — send it.

```bash
afmail push                                              # preview: the full plan and its plan_id
afmail push --confirm push_plan_<REVIEWED_PLAN_ID>       # apply exactly that plan
```

Then it reports back on everything else in the inbox — the look-alike
PayPal domain to junk, the `.zip` "invoice" from a stranger to junk, the
shipping notification and the Stripe payout to archive — and asks before
filing any of it.

That is the whole shape: **you ask, the agent works in local files, and
nothing reaches your real mailbox until you've seen the resolved plan and it
runs `push --confirm` with that exact ID.**

## Why it is shaped this way

- **A worktree, not tool calls.** Mail becomes plain files with stable ids.
  Your agent reads Markdown, greps it, and points at the same message across
  runs, instead of round-tripping every look through a live provider API.
- **One confirm boundary.** Replying writes a draft, archiving moves a local
  folder, marking spam removes it from triage. Each of those only *queues* a
  remote effect. Bare `afmail push` is a pure preview with a plan id bound to
  that exact snapshot; `--confirm PLAN_ID` is the single moment anything is
  sent, moved, or flagged, and any change in between makes the old id fail.
  Every confirmed effect lands in an append-only audit log.
- **Cases, not just messages.** Ongoing issues get a folder with the messages
  involved, notes, and draft replies, so work survives the session that
  started it.
- **Bring your own brain.** afmail does not classify mail, decide what
  matters, or write replies. Any skill or model does that on top of a stable,
  file-first mailbox. Put your priorities and reply style in `AGENTS.md`; the
  embedded [Agent Skill](skills/agent-first-mail/SKILL.md) teaches the safety
  contract, and yours stacks on it.

The exact permission line between local reads, remote reads, local changes,
remote effects, and permanent deletion is the
[authorization contract](docs/authorization.md).

## Connect your mailbox

One workspace directory per account. Your agent can run every step; the only
thing it needs from you is an app password from your provider.

```bash
mkdir gmail && cd gmail
afmail init
afmail config set imap.host imap.gmail.com
afmail config set imap.username ada@gmail.com
afmail config set imap.password_secret "file:$HOME/.config/afmail/secrets.json#gmail_app_password"
afmail config set smtp.host smtp.gmail.com
afmail config set smtp.username ada@gmail.com
afmail config set smtp.password_secret "file:$HOME/.config/afmail/secrets.json#gmail_app_password"
afmail config add identities ada name="Ada Lovelace" email=ada@gmail.com default=true
afmail config remove identities me
afmail status          # what is still missing
afmail remote test     # logs in to IMAP and SMTP; sends nothing
```

For Gmail that app password comes from
<https://myaccount.google.com/apppasswords> once 2-Step Verification is on,
and Gmail needs a label named `Archive` because it has no archive folder of
its own. [Connect a Mailbox](docs/setup.md) walks through Gmail, Fastmail,
iCloud, and generic servers, where to keep the password so it stays out of
your private Git history, and how mailbox folders map. Outlook.com and
Microsoft 365 are not supported: Microsoft only allows OAuth logins, which
afmail does not implement yet.

## What your agent gets to work with

- **`triage/`** — readable Markdown views of new mail, to decide what needs
  attention.
- **`cases/`** and **`archived-cases/`** — a folder per ongoing issue: notes,
  the messages involved, draft replies; archived once its effects are
  confirmed.
- **`notifications/`** — completed notification mail filed out of the way.
- **Stable refs and locator lists** — every message, case, and notification
  has a durable id, and `triage list`, `case list`, `notification list`
  return those ids with path templates the agent expands or hands to `show`.

The [Workspace Model](docs/workspace.md) and [File Formats](docs/file-formats.md)
cover the exact layout.

## Review with your agent

Some decisions want a screen, not a chat transcript. `afmail triage review`,
`afmail case review CASE_REF`, and `afmail case draft review CASE_REF DRAFT`
open a blocking, local human-in-the-loop page: the person reads the actual
mail, settles each item on the spot, edits notes or a draft in place, and can
type a question that reaches the agent on the command's event stream.
Closing the window is the ordinary end of a review; whatever was settled
stays settled, whatever was not stays open. Review never files a message,
queues a push, or touches the remote mailbox — settling records a decision,
and the normal preview/confirm flow still applies.

`--mode link` hands the same page to a phone on your trusted local network;
`--mode session` registers it for an [Agent-First UI](https://agentfirstkit.com/agent-first-ui)
session to pick up later. Review pages render from the same editable
templates as generated Markdown; see the [template contract](docs/templates.md).

## Typed local HTTP API

Trusted agent runtimes and applications can drive afmail over HTTP through the
same domain core. `afmail api serve` publishes OpenAPI 3.2 plus standalone JSON
Schemas and reuses the CLI's locks, idempotency records, audit behavior, and
push plan confirmation. It exposes no shell or generic command endpoint. See
the [HTTP API contract](docs/api.md).

## Adopt it: hand afmail to your agent

The quickest way to find out whether afmail earns a place in your toolkit is to
let your agent read it and tell you. Paste this to your agent:

> Read what Agent-First Mail is at https://agentfirstkit.com/agent-first-mail and
> tell me in plain terms what it would do for me. If I want it, install it — the
> prebuilt package for the quick path, or build from source after a quick security
> review of the repo if you'd rather read what you run — then run `afmail skill
> install` so you follow its behavior rules. Then set up my mailbox: afmail uses
> one workspace directory per account, so make a folder, run `afmail init` in it,
> and help me connect my mailbox following https://agentfirstkit.com/agent-first-mail/docs/setup/.

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

Prebuilt archives are also available from
[GitHub Releases](https://github.com/agentfirstkit/agent-first-mail/releases).
Default builds include the `demo`, `api`, and `review` features;
`cargo build --no-default-features` gives a core mail-workspace binary whose
excluded commands return a structured `feature_disabled` error.

Then install the embedded [Agent Skill](skills/agent-first-mail/SKILL.md) so the
agent follows afmail's behavior rules, and restart the agent so it reloads
them. `skill install` targets Codex, Claude Code, opencode, and Hermes;
`skill status` reports whether each install is present, valid, and current:

```bash
afmail skill install
afmail skill status
```

To remove it, run `afmail skill uninstall`.

## Docs

- [Connect a Mailbox](docs/setup.md) — workspace setup, credentials, Gmail / Fastmail / iCloud / generic IMAP
- [Core Design Principles](docs/design-principles.md) — what afmail guarantees, and why
- [Authorization Contract](docs/authorization.md) — the permission line for reads, local changes, remote effects, deletion
- [Workspace Model](docs/workspace.md) — the folder layout and what each part is for
- [File Formats](docs/file-formats.md) — the exact files a pull and your commands produce
- [HTTP API](docs/api.md) — OpenAPI discovery, authentication, idempotency, and network modes
- [Templates](docs/templates.md) — the Markdown and Review template contract
- [CLI Contract](docs/cli.md) — every command and flag for the `afmail` binary
- [Code Structure](docs/code-structure.md) — where crate responsibilities live
- [Agent Skill](skills/agent-first-mail/SKILL.md) — the behavior contract agents follow

## License

MIT
