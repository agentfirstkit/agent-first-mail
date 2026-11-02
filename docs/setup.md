# Connect a Mailbox

afmail works one mailbox per workspace directory. Setting one up is four
steps: create the workspace, configure receiving (IMAP, Cloudflare, or a file
channel) and sending (SMTP, Cloudflare REST, or the file outbox), name the
identity you send as, then check readiness before the first pull. Every step is
an ordinary `afmail` command, so your agent can run all of it for you; the only
part that needs a person is creating the app password at your mail provider.

Nothing here sends mail. `afmail remote test` logs in and logs out;
`afmail pull --dry-run` counts what a pull would fetch without downloading
bodies; an IMAP pull leaves the remote mailbox unchanged. A Cloudflare pull consumes
R2 spool objects only after local import succeeds.

If you just want to see afmail work before connecting a real account, use the
[demo workspace](workspace.md#demo-workspaces): `afmail demo init` needs no
credentials at all.

## 1. Create the workspace

```bash
mkdir gmail && cd gmail
afmail init
```

`init` creates only mailbox-workspace files: `.afmail/`, the `triage/`,
`cases/`, `archived-cases/`, and `notifications/` directories, `templates/`,
and a managed block in `.gitignore`. It never installs a skill or edits your
agent root's `AGENTS.md`. Its result lists `created_files`, `changed_files`,
and `next_steps`, so an agent can drive setup without guessing config keys.

The workspace is designed to be committed to a **private** Git repository:
pulled mail, attachments, queued effects, and audit files are all normal
workspace state. Credentials are the exception, which is why the next section
matters.

## 2. Point it at your servers

Every value below is set with `afmail config set KEY VALUE`; it validates the
key against the schema and writes `.afmail/config.json` in place.
`afmail config show` prints the effective config with defaults filled in.

| Key | Default | Meaning |
|---|---:|---|
| `imap.host` | | IMAP server hostname |
| `imap.port` | `993` | IMAP port |
| `imap.tls` | `true` | Implicit TLS on connect. Plaintext is refused for any host but loopback. |
| `imap.tls_root_cert_path` | | Extra PEM root for a self-signed server |
| `imap.username` | | Login name; usually your full address |
| `imap.password_secret` | | The password, or where to read it (below) |
| `cloudflare.account_id` | | Account hosting the R2 bucket and/or Email Sending |
| `cloudflare.sending.api_token_secret` | | Email Sending API token or secret source |
| `cloudflare.r2.bucket` | | R2 spool bucket; setting it enables this source |
| `cloudflare.r2.prefix` | `inbox/` | Pending delivery prefix |
| `cloudflare.r2.imported_prefix` | | Optional archive prefix; otherwise delete after import |
| `cloudflare.r2.endpoint_url` | | Optional S3 endpoint override; otherwise derived from account id |
| `cloudflare.r2.access_key_id` | | R2 S3 Access Key ID |
| `cloudflare.r2.secret_access_key_secret` | | Secret Access Key or password source |
| `cloudflare.import_as` | `triage` | Import disposition: triage, spam, or trashed |
| `cloudflare.direction` | `inbound` | Mail direction |
| `smtp.host` | | SMTP submission hostname |
| `smtp.port` | `587` | SMTP port |
| `smtp.starttls` | `true` | Upgrade the connection with STARTTLS |
| `smtp.tls_wrapper` | `false` | Implicit TLS instead, for port 465 servers |
| `smtp.username` | | Login name |
| `smtp.password_secret` | | The password, or where to read it |

### Where the password lives

`password_secret` accepts the password itself or a **source** that says where
to read it. The source is parsed when you set it and read only when a
connection needs it, so the machine that edits the config does not have to be
the machine that holds the secret.

| Value | Reads |
|---|---|
| `abcd efgh ijkl mnop` | The literal password, stored in `config.json` |
| `env:AFMAIL_GMAIL_PASSWORD_SECRET` | An environment variable; its name must end in `_SECRET` |
| `file:/abs/path/secrets.json#gmail_app_password` | One key of a JSON, TOML, YAML, dotenv, or INI file, format taken from the extension |
| `file+dotenv:/abs/path/afmail.env#GMAIL_APP_PASSWORD` | Same, with the format named explicitly |
| `literal:env:not-a-source` | Escape for a password that happens to start with a scheme |

Paths are used as written: no `~` expansion, so give an absolute path. Because
the workspace is meant to live in Git, keep the credential **outside** it:

```bash
mkdir -p ~/.config/afmail
printf '{"gmail_app_password": "abcdefghijklmnop"}\n' > ~/.config/afmail/secrets.json
chmod 600 ~/.config/afmail/secrets.json
afmail config set imap.password_secret "file:$HOME/.config/afmail/secrets.json#gmail_app_password"
```

Command output and logs always print a configured secret as `***`.

## 3. Say who you are

Identities are the addresses afmail sends as. `init` leaves a placeholder that
`afmail status` reports until it is replaced:

```bash
afmail config add identities ada name="Ada Lovelace" email=ada@gmail.com default=true
afmail config remove identities me
```

A `From` header and SMTP envelope sender always come from a configured
identity; an optional `identities/<slug>.md` file adds a persona footer that is
written into drafts for review, never appended at send time.

## 4. Check readiness, then pull

`status.readiness.sources` reports configuration readiness for IMAP, Cloudflare
R2, and file input; `senders` reports SMTP, Cloudflare sending, and file replies.
Each route has `configured`, `feature_enabled`, `ready`, and `missing_config`.
`ready_for_pull` requires all configured default pull sources to be ready.
IMAP pull readiness also requires non-empty `actions.pull.default_mailbox_ids`;
this does not affect IMAP connection or send-step readiness. Explicit
`pull --files` and `pull --cloudflare` select only those sources and do not
require default IMAP mailboxes.
`ready_for_send` means at least one configured reply route is ready with a
non-placeholder identity. Email readiness also checks every configured send
step: a Cloudflare send followed by an IMAP append still needs IMAP configuration.
An SMTP relay can deliberately omit authentication by leaving both username
and password absent. If either is supplied, both must be non-empty: readiness
reports the missing half and transport setup refuses to open a connection.
Configuration commands still allow setting the two keys one at a time.
A file-only workspace does
not need IMAP or SMTP. Disabled Cargo features keep the corresponding routes
unready even when their configuration is present.

These fields inspect configuration presence only. They do not resolve secret
sources, test credentials or permissions, open file-channel directories, or
contact providers. Use the relevant remote tests before pulling real mail.

```bash
afmail status          # readiness.missing_config lists what is still unset
afmail remote test     # IMAP login + SMTP handshake and auth; sends nothing
afmail remote folders  # what the server calls its mailboxes, with special-use attributes
afmail pull --dry-run  # how much a first pull would fetch
afmail pull
```

A fresh pull defaults to the last 90 days, at most 500 new messages and
512 MiB across the selected mailboxes. `--all-history` imports everything and
is meant for an explicitly reviewed one-off; `--max-new-messages` and
`--max-total-bytes` stay hard budgets either way.

IMAP connection attempts have a 30-second timeout for each resolved address.
Addresses are tried in turn; DNS resolution is outside that timeout, and the
timeout is not a total budget shared by every address. Established sockets
retain their existing read/write timeouts.

## Keeping the workspace in Git

Raw `.eml` evidence, canonical metadata, generated read views, attachments,
queued effects, and audit files are all ordinary workspace state that a
private repository can version, sync, and back up; the managed block in
`.gitignore` excludes only machine-local state and rebuildable caches. That
is a Git **trust** boundary, not an encryption boundary: anyone with
repository access, host backups, local machine access, or retained history
can still read the mail, and deleting files from the worktree does not erase
old commits. Credentials are different from evidence: keep them in a source
outside the repository as described above, never inline.

To restore or move a workspace, clone or fetch the repository, then run
`afmail status` from it to verify the `workspace_uid`, local counts, and
`storage.*_bytes` summary. Run `afmail doctor` before making new changes or
pushing remote effects; it checks canonical files, incomplete local
transactions, pending idempotency records, contact index conflicts, and
retained push/audit recovery state. There is no separate restore command;
this is validation over the existing workspace.

## Provider notes

For mail servers, afmail speaks plain IMAP and SMTP with password login; any
server that offers those works. Cloudflare and native file channels are the
alternatives to IMAP/SMTP. The providers below are the ones people ask about.

### Native file channels

For non-email conversations, configure a bounded JSON inbox and outbox under
`files.sources`. `afmail pull --files` imports records, and replies use the normal
case/draft/confirmed-push workflow. See [channels.md](channels.md) for the record
contract, capabilities, and recovery behavior.

### Cloudflare (Email Routing + R2 + Email Service)

Enable Email Routing for a domain hosted in Cloudflare DNS through the dashboard.
It adds the receiving DNS records and routes inbound mail to a Worker. For
outbound sending, separately onboard your sending domain to Email Sending and meet
its account/plan requirements.

Create an R2 Object Read & Write token scoped to the spool bucket. Save its
Secret Access Key outside the workspace, for example as `r2_secret_access_key`
in `~/.config/afmail/secrets.json`. This S3 credential differs from a general
Cloudflare API token. If sending, create an Email Sending token with Email
Sending: Edit permission and save it as `cloudflare_email_token` in the same
private file.

Generate the Worker project with `afmail cloudflare scaffold ~/afmail-worker`.
The destination must be absent or empty. It includes both CLI configurations,
source, locked dependencies, tests, and a deployment README. Use Node.js 24+.
Before cf deployment, edit the example receiving address in
`cloudflare.config.ts`. Choose one deployment column:

| Step | Wrangler | cf |
|---|---|---|
| Enter project and install tools | `cd ~/afmail-worker` then `npm ci` | `cd ~/afmail-worker` then `npm ci` |
| Authenticate | `npx wrangler login` | `npx cf auth login` |
| Create spool bucket | `npx wrangler r2 bucket create afmail-spool` | `npx cf r2 buckets create --name afmail-spool` |
| Check build | `npm run dry-run:wrangler` | `npm run dry-run:cf` |
| Deploy | `npx wrangler deploy` | `npx cf deploy` |
| Route receiving addresses | Dashboard → Email Routing → Routing Rules → Send to a Worker | `triggers.email({ addresses: [...] })` manages the configured addresses |

cf uses its own login. For explicit rules outside cf-managed triggers,
`cf email-routing rules create` maps to
`POST /zones/{zone_id}/email/routing/rules`, and
`cf email-routing rules catch-all update` maps to
`PUT /zones/{zone_id}/email/routing/rules/catch_all`; the generated README gives
request examples. Use `cf cli search` and `cf schema` to inspect current names.

Back in your afmail workspace:

```bash
afmail config set cloudflare.account_id 0123456789abcdef0123456789abcdef
afmail config set cloudflare.r2.bucket afmail-spool
afmail config set cloudflare.r2.access_key_id YOUR_ACCESS_KEY_ID
afmail config set cloudflare.r2.secret_access_key_secret "file:$HOME/.config/afmail/secrets.json#r2_secret_access_key"
afmail config set actions.draft.save.steps '[]'
afmail config set actions.draft.send.steps '[{"cloudflare_send": {}}]'
afmail config set cloudflare.sending.api_token_secret "file:$HOME/.config/afmail/secrets.json#cloudflare_email_token"
afmail remote test
afmail pull --cloudflare
```

Omit the sending token for a receiving-only workspace. Empty save steps disable
remote draft uploads with `draft_upload_unavailable`; sending requires exactly
one `smtp_send` or `cloudflare_send` step. Both write the local sent EML and
case record without requiring IMAP. The default action lists remain suitable
for IMAP-backed Drafts/Sent.

`cloudflare_send` submits the generated MIME directly to Cloudflare's HTTPS
`send_raw` API, including the complete envelope recipient list while stripping
Bcc from transmitted headers. It requires the `cloudflare` Cargo feature;
`smtp_send` requires the `smtp` feature. Both are enabled by default. For a
Cloudflare workspace without SMTP transport, build with
`cargo build --no-default-features --features cloudflare`.

Cloudflare submission receipts appear in successful push output under
`send_receipts` and are retained in the `push_succeeded` audit event after queue
cleanup. Each receipt records the provider `message_id`, `delivered`, `queued`,
`permanent_bounces`, and `suppressed_recipients`. A successful submission can
include failed recipients: inspect these arrays before considering delivery
complete. No accepted recipients fails the step and leaves its receipt in the
queue. Never resend a partially accepted message to the entire recipient list.
Failed pushes also retain available receipts in their result and audit event.
Timeouts, lost responses, and malformed receipts retain `remote_outcome_unknown`
and refuse automatic resubmission. Message-ID is not an idempotency key.

SMTP, REST, and Worker submissions use the same Cloudflare delivery pipeline.
This transport does not subscribe to later delivery events; `queued` describes
provider acceptance, not final delivery. `afmail remote test` checks R2 and/or
SMTP connections and validates local sending configuration. It reports
`cloudflare_sending.status: not_tested` and `permissions_verified: false`
because it does not send mail or verify Email Sending permissions.

Email Routing accepts inbound messages up to 25 MiB; afmail's local parser limit
remains 100 MiB. For `cloudflare_send`, afmail itself rejects a message over
5 MiB (`message_too_large`) and requires 1–50 envelope recipients
(`draft_invalid`) before submitting.
The Worker stores MIME bytes without adding headers and records envelope
recipients as R2 metadata, preserving Bcc identity matching without altering
content hashes. Identical bytes from multiple deliveries or IMAP merge into one
local message; different bytes remain separate even with the same Message-ID.
Pull reports those shared-Message-ID, distinct-byte messages as weak
`duplicate_candidate` warnings, never as automatic merges. A real two-address
delivery can differ in transport and authentication headers; see
[`cloudflare-live-testing.md`](cloudflare-live-testing.md) for the fixed live
test entrypoint and observed behavior.

The scaffold limits messages to 10 MiB, total reserved/stored content to 1 GiB,
objects to 10,000, and admissions to 100 per 60-second fixed window. Its singleton
SQLite Durable Object enforces reservations before R2 writes; D1 is unnecessary.
Archives count toward both limits and need temporary copy headroom. Configure
`MAX_MESSAGE_BYTES`, `MAX_SPOOL_BYTES`, `MAX_SPOOL_MESSAGES`, `RATE_WINDOW_S`, and
`MAX_MESSAGES_PER_WINDOW` in the Worker. The generated README explains migration,
quota inspection, recipient allowlists, and optional independently verified
sender restrictions. Mail authentication alone does not bound storage.

Provision `SPOOL_CONTROL_TOKEN_SECRET` on the Worker and the same value in a
local private secret file. Then configure:

```bash
afmail config set cloudflare.spool.control_url 'https://YOUR_WORKER.YOUR_SUBDOMAIN.workers.dev'
afmail config set cloudflare.spool.token_secret "file:$HOME/.config/afmail/secrets.json#spool_control_token_secret"
```

The importer uses this authenticated endpoint to acknowledge managed objects,
including archive copies, so storage quota and cleanup remain consistent. Managed
objects without control configuration remain remotely available after local
import, with a retryable warning. Older unmanaged spools retain S3 cleanup.

For event-driven receipt and agent wakeups, configure an R2 creation notification
and HTTP pull Queue, then run `afmail cloudflare watch`; see
[the consumer design and deployment guide](cloudflare-consumer.md). Idle checks
run without a model, and durable new-mail batches trigger a configured command.

A periodic `afmail pull --cloudflare` can run once per minute. Successful import
acknowledges spool objects by deletion; set `cloudflare.r2.imported_prefix` to
`imported/` to archive instead. Prefixes must not overlap. Failed cleanup leaves
the object for a later retry. Cloudflare-only messages have no IMAP locations,
so spam/trash/archive are local, and `remote folders` correctly reports missing
IMAP configuration. `doctor` reports `no_mail_source` when no IMAP, Cloudflare,
or file source is configured. Cloudflare support is included in the default build;
minimal builds without that feature return `feature_disabled`.

Before relying on a new real account, check actual Worker delivery, compare the
raw hashes for the same message sent to two addresses in the zone, and test the
configured SMTP authentication and a confirmed send. Local fixture and MinIO
tests do not prove those provider/account-specific behaviors.

### Gmail

Gmail accepts IMAP and SMTP logins with an **app password**, which Google only
issues on accounts with 2-Step Verification turned on. Your normal account
password will not work, and afmail never needs it.

1. Turn on 2-Step Verification at <https://myaccount.google.com/security>.
2. Create an app password at <https://myaccount.google.com/apppasswords>. Name
   it `afmail` and copy the 16 characters without the spaces. Google shows it
   once. Google Workspace accounts may have app passwords disabled by an
   administrator; without one, afmail cannot connect.
3. Store it outside the workspace as shown above, then:

```bash
afmail config set imap.host imap.gmail.com
afmail config set imap.username ada@gmail.com
afmail config set imap.password_secret "file:$HOME/.config/afmail/secrets.json#gmail_app_password"
afmail config set smtp.host smtp.gmail.com
afmail config set smtp.username ada@gmail.com
afmail config set smtp.password_secret "file:$HOME/.config/afmail/secrets.json#gmail_app_password"
```

Ports and TLS defaults already match Gmail (993 implicit TLS, 587 STARTTLS).

**Archive.** Gmail has no archive folder. Its "archive" is removing the
`INBOX` label, and `[Gmail]/All Mail` is every message you own, not a place
to file anything into. Give it a real folder instead: in Gmail, open
Settings → Labels → Create new label, call it `Archive`, and keep "Show in
IMAP" on. afmail's default `mailboxes.archive` rules match that name, so
archiving a message moves it out of the inbox and under that label with no
further config. Until the label exists, `afmail pull` stops with
`imap_mailbox_unresolved` for `archive`.

**Sent copies.** Gmail stores its own copy of every message sent through
`smtp.gmail.com`. afmail's default send action also appends one to the sent
mailbox, so drop that step to avoid a second copy:

```bash
afmail config set actions.draft.send.steps \
  '{"append_to_mailbox_id":"drafts"}' \
  '{"smtp_send":{}}' \
  '{"add_flags":["\\Seen","\\Answered"],"on":"reply_to_message"}'
```

How Gmail's folders map onto afmail's mailbox ids:

| afmail id | Gmail mailbox | Resolved by |
|---|---|---|
| `inbox` | `INBOX` | name |
| `sent` | `[Gmail]/Sent Mail` | `\Sent` attribute |
| `junk` | `[Gmail]/Spam` | `\Junk` attribute |
| `trash` | `[Gmail]/Trash` | `\Trash` attribute |
| `drafts` | `[Gmail]/Drafts` | `\Drafts` attribute |
| `archive` | `Archive` | the label you created |

Marking a message spam adds `$Junk` and moves it to Spam; trashing moves it to
Trash, where Gmail deletes it after 30 days. Both are queued effects that wait
for `afmail push --confirm`.

If `afmail remote test` fails with `imap_login_failed` mentioning
"Application-specific password required" or "Invalid credentials", the account
has no 2-Step Verification, the app password was mistyped, or an administrator
has disabled app passwords.

### Fastmail

Create an app password under Settings → Privacy & Security → Integrations
with IMAP and SMTP access. Servers are `imap.fastmail.com` on 993 and
`smtp.fastmail.com` on 587 with STARTTLS (or 465 with
`smtp.tls_wrapper true`). Fastmail advertises every special-use folder,
including `\Archive`, so no mailbox mapping is needed.

### iCloud Mail

Generate an app-specific password at <https://account.apple.com> under
Sign-In and Security; two-factor authentication must be on. Servers are
`imap.mail.me.com` on 993 and `smtp.mail.me.com` on 587 with STARTTLS. Use
your full iCloud address as the username for both. iCloud provides an
`Archive` folder, so defaults apply.

### Any other IMAP and SMTP server

Set host, port, and TLS as the provider documents them. For a server with a
private certificate authority, put its PEM root in `imap.tls_root_cert_path`.
Run `afmail remote folders` to see the exact mailbox names and attributes,
then map any id whose default rules do not match:

```bash
afmail config set mailboxes.archive.resolve '[{"mailbox_name":"Archived Mail"}]'
```

`resolve` is an ordered list of rules; each rule matches a mailbox by exact
`mailbox_name` or by an RFC 6154 `special_use` attribute such as `\Archive`.
Mailboxes carrying `\All` or `\Flagged` can be read but never used as move
targets. `actions.pull.default_mailbox_ids` lists which ids a bare
`afmail pull` reads from IMAP. An empty list requires explicit mailbox ids for
IMAP pulls; file-only and Cloudflare-only pulls remain independent of this list.

### Outlook.com and Microsoft 365

Not supported. Microsoft has turned off password login for IMAP and SMTP on
both personal and business accounts, leaving OAuth 2.0 as the only way in, and
afmail does not implement OAuth yet. An Outlook address entered here fails at
`afmail remote test` rather than at send time, so nothing is lost by trying,
but there is no workaround to configure.
