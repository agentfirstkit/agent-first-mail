# Connect a Mailbox

afmail works one mailbox per workspace directory. Setting one up is four
steps: create the workspace, point it at your IMAP and SMTP servers, name the
identity you send as, then check readiness before the first pull. Every step is
an ordinary `afmail` command, so your agent can run all of it for you; the only
part that needs a person is creating the app password at your mail provider.

Nothing here sends mail. `afmail remote test` logs in and logs out;
`afmail pull --dry-run` counts what a pull would fetch without downloading
bodies; a real `afmail pull` is a read that leaves the remote mailbox exactly
as it was.

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

afmail speaks plain IMAP and SMTP with password login. Any server that offers
those works. The providers below are the ones people ask about.

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
`afmail pull` reads.

### Outlook.com and Microsoft 365

Not supported. Microsoft has turned off password login for IMAP and SMTP on
both personal and business accounts, leaving OAuth 2.0 as the only way in, and
afmail does not implement OAuth yet. An Outlook address entered here fails at
`afmail remote test` rather than at send time, so nothing is lost by trying,
but there is no workaround to configure.
