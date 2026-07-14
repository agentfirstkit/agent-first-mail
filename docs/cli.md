<!-- Generated. Do not edit by hand. Regenerate: afmail --help --recursive --output markdown -->

# afmail CLI Reference

# Agent-First Mail - Let your AI agent work your inbox — email pulled into plain files it reads, sorts, and drafts on your machine, with nothing sent until you confirm.

### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF`.
- stdout carries structured Agent-First Data events; stderr is not a protocol channel.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus durable remote sidecars
.afmail/workspace.json   immutable workspace identity used by plan digests
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/cache/progress.json latest push/pull runtime snapshot
templates/               user-editable generated Markdown templates
triage/message_*.md      active unprocessed message views
spam/*.md                generated spam review views
trash/*.md               generated trash review views
deleted/*.md             generated remote-deleted review views
cases/<group>/<case_uid>-<name>/case.md generated case entry view
cases/<group>/<case_uid>-<name>/data/ canonical case state
cases/<group>/<case_uid>-<name>/views/ generated case detail views
archived-cases/<group>/<case_uid>-<name>/ archived case workspaces; <group> is a case group
contacts/<group>/<contact_uid>-<name>.md canonical contact cards
notifications/<notification_uid>-<name>/notification.md generated notification entry view
notifications/<notification_uid>-<name>/data/ canonical notification state
notifications/<notification_uid>-<name>/views/ generated notification detail views
```

### Examples

```text
afmail init
afmail init email-a
afmail skill status
afmail skill install --agent codex --scope workspace
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --group support --message message_inbox_607146690_21 --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 message_inbox_607146690_23 --reason "follow-up belongs to same feedback case"
afmail notification create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail notification add n20260603001 message_inbox_607146690_24 message_inbox_607146690_25 --summary "billing notification" --reason "billing notification"
afmail notification show n20260603001
afmail notification remove n20260603001 message_inbox_607146690_23 --reason "does not belong in this notification"
afmail message spam message_inbox_607146690_23 message_inbox_607146690_24 --reason "phishing attempt"
afmail message trash message_inbox_607146690_25 message_inbox_607146690_26 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 --group waiting --reason "better category"
afmail case archive c20260603001 --reason "feedback handled"
afmail case unarchive c20260603001 --group support --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail contact create --name "Zhang San" --email zhang@example.com --org Acme --role Buyer --reason "new contact"
afmail contact list --group people
afmail contact email add p20260603001 zhang-alt@example.com --reason "alternate email"
afmail contact rename p20260603001 --name "Zhang Sanfeng" --reason "better display name"
afmail contact extract --from-triage
afmail contact delete p20260603001 --reason "duplicate contact"
afmail case draft reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft change c20260603001 reply-message_inbox_607146690_22.md --body "Thanks, I will check this."
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case draft list c20260603001
afmail case draft send c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push list
afmail push
afmail push --confirm push_plan_<REVIEWED_PLAN_ID>
afmail purge
afmail purge spam --older-than-days 30
afmail purge deleted
afmail log list --limit 20
afmail triage list --limit 10 --details
afmail message show message_inbox_607146690_21
afmail message show message_inbox_607146690_21 --meta
afmail message list --sender alice@example.com --subject-contains invoice --since 2026-05-21 --in all
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: command failed, including an aggregate command where no item completed successfully
- `2`: invalid CLI arguments
- `3`: command completed partially; inspect the canonical error for successful and failed items
- `4`: canonical stdout output was not fully written

Only completely successful commands emit `kind: result`; their payload has no generic `code`, `ok`,
or `outcome` field. Any item failure emits `kind: error`. Partial failures use `error.code: partial`,
and the complete item list is carried in `error.items`; a failed item has an `error` child object.
Exit code `4` means the command could not deliver a complete stdout protocol event; inspect stderr
for the non-protocol fallback diagnostic.

```text
Usage: afmail [OPTIONS] [COMMAND]

Commands:
  init          Initialize the current directory or a child path as an afmail workspace
  pull          Read configured IMAP mailbox ids into local message files without changing remote mail
  config        Read or update local afmail configuration
  remote        Inspect remote IMAP state for configuring mailboxes
  push          Push queued local work or manage the local push queue
  status        Report workspace health, counts, and latest pull/push progress
  doctor        Check afmail workspace consistency without inspecting Git
  purge         Permanently delete old local spam, trash, and remote-deleted records
  skill         Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes
  triage        Inspect the triage queue
  message       Operate on any local message id
  case          Create a case or operate on an existing case ref
  contact       Create a contact or operate on an existing contact ref
  notification  Inspect and manage notification collections
  render        Rebuild generated read views from local workspace state
  log           Inspect the workspace audit log
  ui            Emit or open an Agent-First UI operation surface for this workspace
  help          Print this message or the help of the given subcommand(s)

Options:
      --output <OUTPUT>
          Output format: json (default), yaml, plain

          [default: json]

      --log <LOG>
          Log categories (comma-separated): startup, request, progress, retry

      --verbose
          Enable all log categories. Startup argv redacts values and non-command positional tokens

      --stdout-file <PATH>
          Redirect stdout bytes to this file

      --stderr-file <PATH>
          Redirect stderr bytes to this file

      --workspace <PATH>
          Operate on this existing afmail workspace instead of discovering from cwd

      --request-id <ID>
          Idempotency key for supported non-idempotent local mutations

  -V, --version
          Print structured version event

  -h, --help
          Print help. Add --recursive to expand every nested subcommand; add --output json|yaml|markdown to render this help in another format.
```

## Agent-First Mail init - Initialize the current directory or a child path as an afmail workspace

```text
Usage: init [PATH]

Arguments:
  [PATH]
          Optional workspace path to initialize under the current directory

Options:
  -h, --help
          Print help
```

## Agent-First Mail pull - Read configured IMAP mailbox ids into local message files without changing remote mail

```text
Usage: pull [OPTIONS] [IDS]...

Arguments:
  [IDS]...
          Configured mailbox ids to pull. With none, pulls actions.pull.default_mailbox_ids

Options:
      --dry-run
          Estimate remote candidates and budgets without downloading or writing locally

      --since-days <DAYS>
          Limit the remote candidate range to messages from the last DAYS days

      --max-new-messages <COUNT>
          Maximum number of new messages stored across all selected mailboxes

      --max-total-bytes <BYTES>
          Maximum new raw message bytes stored across all selected mailboxes

      --all-history
          Explicitly include all remote history instead of the default initial window

  -h, --help
          Print help
```

## Agent-First Mail config - Read or update local afmail configuration

```text
Usage: config <COMMAND>

Commands:
  show    Show effective config after applying built-in defaults
  get     Get one config key
  set     Set one config key. Multiple values become an array for array keys
  add     Add an element to a keyed list (e.g. `identities`). FIELDS are key=value
  remove  Remove a keyed-list element by its slug
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail config show - Show effective config after applying built-in defaults

```text
Usage: show

Options:
  -h, --help
          Print help
```

### Agent-First Mail config get - Get one config key

```text
Usage: get <KEY>

Arguments:
  <KEY>


Options:
  -h, --help
          Print help
```

### Agent-First Mail config set - Set one config key. Multiple values become an array for array keys

```text
Usage: set <KEY> [VALUES]...

Arguments:
  <KEY>


  [VALUES]...


Options:
  -h, --help
          Print help
```

### Agent-First Mail config add - Add an element to a keyed list (e.g. `identities`). FIELDS are key=value

```text
Usage: add <KEY> <SLUG> [FIELDS]...

Arguments:
  <KEY>


  <SLUG>


  [FIELDS]...


Options:
  -h, --help
          Print help
```

### Agent-First Mail config remove - Remove a keyed-list element by its slug

```text
Usage: remove <KEY> <SLUG>

Arguments:
  <KEY>


  <SLUG>


Options:
  -h, --help
          Print help
```

## Agent-First Mail remote - Inspect remote IMAP state for configuring mailboxes

```text
Usage: remote <COMMAND>

Commands:
  test     Test configured IMAP and SMTP access without sending mail
  folders  List IMAP folders/mailboxes
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail remote test - Test configured IMAP and SMTP access without sending mail

```text
Usage: test

Options:
  -h, --help
          Print help
```

### Agent-First Mail remote folders - List IMAP folders/mailboxes

```text
Usage: folders

Options:
  -h, --help
          Print help
```

## Agent-First Mail push - Push queued local work or manage the local push queue

```text
Usage: push [OPTIONS] [COMMAND]

Commands:
  list  Preview the complete resolved push plan without writing
  help  Print this message or the help of the given subcommand(s)

Options:
      --dry-run
          Show planned push actions without IMAP/SMTP writes. This is the default

      --confirm <PLAN_ID>
          Apply exactly the reviewed plan returned by push preview

  -h, --help
          Print help
```

### Agent-First Mail push list - Preview the complete resolved push plan without writing

```text
Usage: list

Options:
  -h, --help
          Print help
```

## Agent-First Mail status - Report workspace health, counts, and latest pull/push progress

```text
Usage: status

Options:
  -h, --help
          Print help
```

## Agent-First Mail doctor - Check afmail workspace consistency without inspecting Git

```text
Usage: doctor [COMMAND]

Commands:
  repair  Repair only unambiguous afmail-generated state
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail doctor repair - Repair only unambiguous afmail-generated state

```text
Usage: repair [OPTIONS]

Options:
      --confirm
          Apply repair actions. Without this flag, repair is refused

  -h, --help
          Print help
```

## Agent-First Mail purge - Permanently delete old local spam, trash, and remote-deleted records

```text
Usage: purge [OPTIONS] [COMMAND]

Commands:
  spam     Permanently delete old local spam records
  trash    Permanently delete old local trash records
  deleted  Permanently delete old local records whose remote message disappeared
  help     Print this message or the help of the given subcommand(s)

Options:
      --older-than-days <DAYS>
          Only purge messages in a discard state at least this many days ago. Defaults to 30

      --confirm <PLAN_ID>
          Permanently delete exactly the reviewed purge plan

  -h, --help
          Print help
```

### Agent-First Mail purge spam - Permanently delete old local spam records

```text
Usage: spam [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked spam at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

### Agent-First Mail purge trash - Permanently delete old local trash records

```text
Usage: trash [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked trash at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

### Agent-First Mail purge deleted - Permanently delete old local records whose remote message disappeared

```text
Usage: deleted [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked remote-deleted at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

## Agent-First Mail skill - Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes

```text
Usage: skill <COMMAND>

Commands:
  status     Show whether the Agent-First Mail skill is installed and valid
  install    Install the Agent-First Mail skill
  uninstall  Remove an afmail-managed Agent-First Mail skill
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail skill status - Show whether the Agent-First Mail skill is installed and valid

```text
Usage: status [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex skill under $CODEX_HOME/skills or .codex/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills
          - hermes:      Manage the Hermes skill under $HERMES_HOME/skills or ~/.hermes/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Workspace scope installs under the current workspace's skill directory

          Possible values:
          - personal:  Install under the user-level skills directory
          - workspace: Install under the current workspace's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

  -h, --help
          Print help (see a summary with '-h')
```

### Agent-First Mail skill install - Install the Agent-First Mail skill

```text
Usage: install [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex skill under $CODEX_HOME/skills or .codex/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills
          - hermes:      Manage the Hermes skill under $HERMES_HOME/skills or ~/.hermes/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Workspace scope installs under the current workspace's skill directory

          Possible values:
          - personal:  Install under the user-level skills directory
          - workspace: Install under the current workspace's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

      --force
          Overwrite or remove an unmanaged Agent-First Mail skill at the target path

  -h, --help
          Print help (see a summary with '-h')
```

### Agent-First Mail skill uninstall - Remove an afmail-managed Agent-First Mail skill

```text
Usage: uninstall [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex skill under $CODEX_HOME/skills or .codex/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills
          - hermes:      Manage the Hermes skill under $HERMES_HOME/skills or ~/.hermes/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Workspace scope installs under the current workspace's skill directory

          Possible values:
          - personal:  Install under the user-level skills directory
          - workspace: Install under the current workspace's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

      --force
          Overwrite or remove an unmanaged Agent-First Mail skill at the target path

  -h, --help
          Print help (see a summary with '-h')
```

## Agent-First Mail triage - Inspect the triage queue

```text
Usage: triage <COMMAND>

Commands:
  list  List compact untriaged message locators
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail triage list - List compact untriaged message locators

```text
Usage: list [OPTIONS]

Options:
      --limit <LIMIT>
          Maximum number of newest triage messages to return

      --details
          Include message metadata, related message ids, and case refs

  -h, --help
          Print help
```

## Agent-First Mail message - Operate on any local message id

```text
Usage: message <COMMAND>

Commands:
  show        Show the full local message metadata, body text, and attachment metadata
  list        List messages by sender, subject, date, or workspace scope
  spam        Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move
  trash       Explicitly discard this message locally, show it under trash/, and queue a Trash move
  unspam      Mark a spam false positive as triage and queue a move out of Junk when needed
  untrash     Mark a trash false positive as triage and queue a move out of Trash when needed
  attachment  Fetch an attachment for this message
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail message show - Show the full local message metadata, body text, and attachment metadata

```text
Usage: show [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --meta
          Show metadata only and omit large body/conversation fields

  -h, --help
          Print help
```

### Agent-First Mail message list - List messages by sender, subject, date, or workspace scope

```text
Usage: list [OPTIONS]

Options:
      --sender <SENDER>
          Only include messages whose sender or from line contains this text

      --subject-contains <SUBJECT_CONTAINS>
          Only include messages whose subject contains this text

      --since <DATE>
          Only include messages on or after this date or RFC3339 timestamp

      --in <SCOPE>
          Limit results to one workspace scope

          [default: all]
          [possible values: triage, case, notification, all]

  -h, --help
          Print help
```

### Agent-First Mail message spam - Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move

```text
Usage: spam [OPTIONS] <MESSAGE_ID>...

Arguments:
  <MESSAGE_ID>...
          Message ids, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this disposition is correct; required by default

  -h, --help
          Print help
```

### Agent-First Mail message trash - Explicitly discard this message locally, show it under trash/, and queue a Trash move

```text
Usage: trash [OPTIONS] <MESSAGE_ID>...

Arguments:
  <MESSAGE_ID>...
          Message ids, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this disposition is correct; required by default

  -h, --help
          Print help
```

### Agent-First Mail message unspam - Mark a spam false positive as triage and queue a move out of Junk when needed

```text
Usage: unspam [OPTIONS] <MESSAGE_ID>...

Arguments:
  <MESSAGE_ID>...
          Message ids, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this message is not spam; required by default

  -h, --help
          Print help
```

### Agent-First Mail message untrash - Mark a trash false positive as triage and queue a move out of Trash when needed

```text
Usage: untrash [OPTIONS] <MESSAGE_ID>...

Arguments:
  <MESSAGE_ID>...
          Message ids, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this message should not be trashed; required by default

  -h, --help
          Print help
```

### Agent-First Mail message attachment - Fetch an attachment for this message

```text
Usage: attachment <COMMAND>

Commands:
  fetch  Fetch one attachment by MIME part id, or all attachments when omitted
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail message attachment fetch - Fetch one attachment by MIME part id, or all attachments when omitted

```text
Usage: fetch <MESSAGE_ID> [PART_ID]

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

  [PART_ID]


Options:
  -h, --help
          Print help
```

## Agent-First Mail case - Create a case or operate on an existing case ref

```text
Usage: case <COMMAND>

Commands:
  create       Create a new case and return its stable UID/ref
  list         List compact case locators
  show         Show a case's case.md without changing workspace state
  add          Add a message to this existing case
  set-summary  Set or replace one case entry summary
  remove       Remove one message from this case and return it to triage
  move         Move this case to another group
  rename       Rename this case's human-readable name without changing its UID
  notes        Show or edit case notes
  archive      Archive this active case
  unarchive    Move this archived case back to an active case group
  trash        Move every message in this case to local trash and queue Trash moves
  delete       Delete an empty case
  tag          Add a case organization tag
  untag        Remove a case organization tag
  draft        Create, edit, validate, queue, or remove local case drafts
  merge        Merge another case into this case
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail case create - Create a new case and return its stable UID/ref

```text
Usage: create [OPTIONS] --name <NAME> --group <GROUP>

Options:
      --name <NAME>
          Human-readable case name used in case.md and the directory suffix

      --group <GROUP>
          Required case category/group, for example support, invoice, 客服, 发票, 合同, or 退款

      --message <MESSAGE>
          Optional first message to add to the case

      --summary <SUMMARY>
          Short summary for the first message

      --reason <REASON>
          Why this case is being created

  -h, --help
          Print help
```

### Agent-First Mail case list - List compact case locators

```text
Usage: list [OPTIONS]

Options:
      --archived
          List archived cases instead of active cases

      --all
          List active and archived cases together

  -h, --help
          Print help
```

### Agent-First Mail case show - Show a case's case.md without changing workspace state

```text
Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

### Agent-First Mail case add - Add a message to this existing case

```text
Usage: add [OPTIONS] <CASE_REF> <MESSAGE_ID>...

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>...
          Message ids to add

Options:
      --summary <SUMMARY>
          Short summary for this message in the case

      --reason <REASON>
          Why this message belongs in this case; required by default

  -h, --help
          Print help
```

### Agent-First Mail case set-summary - Set or replace one case entry summary

```text
Usage: set-summary [OPTIONS] --summary <SUMMARY> <CASE_REF> <MESSAGE_ID>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>
          Message id whose summary should be changed

Options:
      --summary <SUMMARY>
          New summary text

      --reason <REASON>
          Why this summary is useful; required by default

  -h, --help
          Print help
```

### Agent-First Mail case remove - Remove one message from this case and return it to triage

```text
Usage: remove [OPTIONS] <CASE_REF> <MESSAGE_ID>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>
          Message id to remove

Options:
      --reason <REASON>
          Why this message does not belong in this case; required by default

  -h, --help
          Print help
```

### Agent-First Mail case move - Move this case to another group

```text
Usage: move [OPTIONS] --group <GROUP> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --group <GROUP>
          Destination group

      --reason <REASON>
          Why this group better represents the case; required by default

  -h, --help
          Print help
```

### Agent-First Mail case rename - Rename this case's human-readable name without changing its UID

```text
Usage: rename [OPTIONS] --name <NAME> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --name <NAME>
          New human-readable case name

      --reason <REASON>
          Why this name better represents the case; required by default

  -h, --help
          Print help
```

### Agent-First Mail case notes - Show or edit case notes

```text
Usage: notes <COMMAND>

Commands:
  show     Show notes markdown
  append   Append text to notes markdown
  replace  Replace notes markdown with text
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case notes show - Show notes markdown

```text
Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case notes append - Append text to notes markdown

```text
Usage: append --text <TEXT> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to append

  -h, --help
          Print help
```

#### Agent-First Mail case notes replace - Replace notes markdown with text

```text
Usage: replace --text <TEXT> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to write

  -h, --help
          Print help
```

### Agent-First Mail case archive - Archive this active case

```text
Usage: archive [OPTIONS] <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --reason <REASON>
          Why this case is ready to archive; required by default

  -h, --help
          Print help
```

### Agent-First Mail case unarchive - Move this archived case back to an active case group

```text
Usage: unarchive [OPTIONS] --group <GROUP> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --group <GROUP>
          Active case group to unarchive into

      --reason <REASON>
          Why this case needs active attention again; required by default

  -h, --help
          Print help
```

### Agent-First Mail case trash - Move every message in this case to local trash and queue Trash moves

```text
Usage: trash [OPTIONS] <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --reason <REASON>
          Why this whole case should be discarded; required by default

  -h, --help
          Print help
```

### Agent-First Mail case delete - Delete an empty case

```text
Usage: delete [OPTIONS] <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --allow-notes
          Also delete notes.md when it contains user-authored notes

      --reason <REASON>
          Why this case should be deleted; required by default

  -h, --help
          Print help
```

### Agent-First Mail case tag - Add a case organization tag

```text
Usage: tag [OPTIONS] <CASE_REF> <TAG>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <TAG>
          Case organization tag

Options:
      --reason <REASON>
          Why this tag is useful; required by default

  -h, --help
          Print help
```

### Agent-First Mail case untag - Remove a case organization tag

```text
Usage: untag [OPTIONS] <CASE_REF> <TAG>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <TAG>
          Case organization tag

Options:
      --reason <REASON>
          Why this tag should be removed; required by default

  -h, --help
          Print help
```

### Agent-First Mail case draft - Create, edit, validate, queue, or remove local case drafts

```text
Usage: draft <COMMAND>

Commands:
  new       Scaffold a new outbound draft (not a reply) in this case
  reply     Scaffold a reply draft to a message, prefilled and quoting the original
  change    Change an existing editable draft in place
  show      Show an existing draft in a review-friendly JSON shape
  validate  Validate a draft under the case drafts directory
  list      List local drafts with their current status
  upload    Queue this draft to be uploaded to the remote Drafts mailbox
  send      Queue this draft to be sent and recorded in the case after push succeeds
  attach    Copy or reference a file and add it to a draft's attachments
  remove    Remove a local draft and any queued outbound item for it
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft new - Scaffold a new outbound draft (not a reply) in this case

```text
Usage: new [OPTIONS] --to <TO> --subject <SUBJECT> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --to <TO>
          Recipient address. Repeatable

      --cc <CC>
          Cc address. Repeatable

      --subject <SUBJECT>
          Draft subject

      --identity <IDENTITY>
          Workspace identity slug to send as. Defaults to the configured default identity

      --body <BODY>
          Draft body text

      --body-file <BODY_FILE>
          Path to a file whose contents become the draft body

  -h, --help
          Print help
```

#### Agent-First Mail case draft reply - Scaffold a reply draft to a message, prefilled and quoting the original

```text
Usage: reply [OPTIONS] <CASE_REF> <MESSAGE_ID>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>
          Message id in this case to reply to

Options:
      --identity <IDENTITY>
          Workspace identity slug to send as. Defaults to the configured default identity

      --body <BODY>
          Draft body text. Replaces the default reply template when provided

      --body-file <BODY_FILE>
          Path to a file whose contents become the draft body

      --all
          Reply to all original recipients (To and Cc), not just the sender

  -h, --help
          Print help
```

#### Agent-First Mail case draft change - Change an existing editable draft in place

```text
Usage: change [OPTIONS] <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
      --subject <SUBJECT>
          Replacement subject

      --to <TO>
          Replacement To list. Repeatable; when provided it replaces all To recipients

      --cc <CC>
          Replacement Cc list. Repeatable; when provided it replaces all Cc recipients

      --clear-cc
          Clear all Cc recipients

      --identity <IDENTITY>
          Change the workspace identity slug used by this draft

      --body <BODY>
          Replacement draft body text

      --body-file <BODY_FILE>
          Path to a file whose contents replace the draft body

  -h, --help
          Print help
```

#### Agent-First Mail case draft show - Show an existing draft in a review-friendly JSON shape

```text
Usage: show <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft validate - Validate a draft under the case drafts directory

```text
Usage: validate <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft list - List local drafts with their current status

```text
Usage: list <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft upload - Queue this draft to be uploaded to the remote Drafts mailbox

```text
Usage: upload <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft send - Queue this draft to be sent and recorded in the case after push succeeds

```text
Usage: send <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft attach - Copy or reference a file and add it to a draft's attachments

```text
Usage: attach <CASE_REF> <DRAFT_NAME> <PATH>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

  <PATH>
          Local file path to attach

Options:
  -h, --help
          Print help
```

#### Agent-First Mail case draft remove - Remove a local draft and any queued outbound item for it

```text
Usage: remove [OPTIONS] <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
      --reason <REASON>
          Why this draft should be removed; required by default

  -h, --help
          Print help
```

### Agent-First Mail case merge - Merge another case into this case

```text
Usage: merge [OPTIONS] <CASE_REF> <OTHER_CASE_REF>

Arguments:
  <CASE_REF>
          Primary case ref

  <OTHER_CASE_REF>
          Case ref to merge into the primary case

Options:
      --reason <REASON>
          Why these cases should be merged; required by default

  -h, --help
          Print help
```

## Agent-First Mail contact - Create a contact or operate on an existing contact ref

```text
Usage: contact <COMMAND>

Commands:
  create   Create a new contact record
  list     List contacts, optionally filtered by group, tag, org, or role
  show     Show a contact's full file content
  move     Move a contact to a different group
  rename   Rename a contact's display name
  email    Add or remove email addresses for a contact
  phone    Add or remove phone numbers for a contact
  tag      Add a tag to a contact
  untag    Remove a tag from a contact
  notes    Show or edit contact notes
  delete   Delete a contact
  extract  Extract stub contacts from message senders not yet in the contact list
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail contact create - Create a new contact record

```text
Usage: create [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          Display name for the contact

      --group <GROUP>
          Group. Defaults to the configured default contact group

      --email <EMAILS>
          Email address. Repeatable

      --phone <PHONES>
          Phone number. Repeatable

      --org <ORG>
          Organization name

      --role <ROLE>
          Role or title

      --tag <TAGS>
          Tag. Repeatable

      --reason <REASON>
          Why this contact is being created; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact list - List contacts, optionally filtered by group, tag, org, or role

```text
Usage: list [OPTIONS]

Options:
      --group <GROUP>
          Filter to a specific group

      --tag <TAG>
          Filter by tag

      --org <ORG>
          Filter by organization

      --role <ROLE>
          Filter by role

  -h, --help
          Print help
```

### Agent-First Mail contact show - Show a contact's full file content

```text
Usage: show <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

### Agent-First Mail contact move - Move a contact to a different group

```text
Usage: move [OPTIONS] --group <GROUP> <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
      --group <GROUP>
          Destination group

      --reason <REASON>
          Why this contact should move groups; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact rename - Rename a contact's display name

```text
Usage: rename [OPTIONS] --name <NAME> <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
      --name <NAME>
          New display name

      --reason <REASON>
          Why this name better represents the contact; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact email - Add or remove email addresses for a contact

```text
Usage: email <COMMAND>

Commands:
  add     Add an email address to the contact
  remove  Remove an email address from the contact
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail contact email add - Add an email address to the contact

```text
Usage: add [OPTIONS] <CONTACT_REF> <EMAIL>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <EMAIL>
          Email address to add

Options:
      --reason <REASON>
          Why this email should be associated with the contact; required by default

  -h, --help
          Print help
```

#### Agent-First Mail contact email remove - Remove an email address from the contact

```text
Usage: remove [OPTIONS] <CONTACT_REF> <EMAIL>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <EMAIL>
          Email address to remove

Options:
      --reason <REASON>
          Why this email should be removed; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact phone - Add or remove phone numbers for a contact

```text
Usage: phone <COMMAND>

Commands:
  add     Add a phone number to the contact
  remove  Remove a phone number from the contact
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail contact phone add - Add a phone number to the contact

```text
Usage: add [OPTIONS] <CONTACT_REF> <PHONE>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <PHONE>
          Phone number to add

Options:
      --reason <REASON>
          Why this phone number should be associated with the contact; required by default

  -h, --help
          Print help
```

#### Agent-First Mail contact phone remove - Remove a phone number from the contact

```text
Usage: remove [OPTIONS] <CONTACT_REF> <PHONE>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <PHONE>
          Phone number to remove

Options:
      --reason <REASON>
          Why this phone number should be removed; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact tag - Add a tag to a contact

```text
Usage: tag [OPTIONS] <CONTACT_REF> <TAG>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <TAG>
          Tag to add

Options:
      --reason <REASON>
          Why this tag is useful; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact untag - Remove a tag from a contact

```text
Usage: untag [OPTIONS] <CONTACT_REF> <TAG>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

  <TAG>
          Tag to remove

Options:
      --reason <REASON>
          Why this tag should be removed; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact notes - Show or edit contact notes

```text
Usage: notes <COMMAND>

Commands:
  show     Show notes markdown
  append   Append text to notes markdown
  replace  Replace notes markdown with text
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail contact notes show - Show notes markdown

```text
Usage: show <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### Agent-First Mail contact notes append - Append text to notes markdown

```text
Usage: append --text <TEXT> <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to append

  -h, --help
          Print help
```

#### Agent-First Mail contact notes replace - Replace notes markdown with text

```text
Usage: replace --text <TEXT> <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to write

  -h, --help
          Print help
```

### Agent-First Mail contact delete - Delete a contact

```text
Usage: delete [OPTIONS] <CONTACT_REF>

Arguments:
  <CONTACT_REF>
          Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix

Options:
      --allow-notes
          Also delete the contact body when it contains user-authored notes

      --reason <REASON>
          Why this contact should be deleted; required by default

  -h, --help
          Print help
```

### Agent-First Mail contact extract - Extract stub contacts from message senders not yet in the contact list

```text
Usage: extract [OPTIONS]

Options:
      --from-triage
          Extract from triage messages only

      --from-case <CASE_REF>
          Extract from a specific case's messages

      --all
          Extract from all triage and case messages

      --group <GROUP>
          Destination group for created contacts. Defaults to contact.default_group

  -h, --help
          Print help
```

## Agent-First Mail notification - Inspect and manage notification collections

```text
Usage: notification <COMMAND>

Commands:
  create       Create a notification collection and optionally file one message
  add          File an existing message into this notification collection
  list         List compact notification collections
  show         Show notification collection index and entries
  remove       Remove a notification message from this collection and return it to triage
  move         Move a notification message to another notification collection
  rename       Rename this notification collection's human-readable name without changing its UID
  delete       Delete an empty notification collection
  set-summary  Set or replace one notification entry summary
  notes        Show or edit notification collection notes
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail notification create - Create a notification collection and optionally file one message

```text
Usage: create [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          Human-readable notification name used in metadata and the directory suffix

      --message <MESSAGE>
          Optional message to immediately file into the new notification collection

      --summary <SUMMARY>
          Required when --message is supplied

      --reason <REASON>
          Why this notification collection is being created

  -h, --help
          Print help
```

### Agent-First Mail notification add - File an existing message into this notification collection

```text
Usage: add [OPTIONS] --summary <SUMMARY> <NOTIFICATION_REF> <MESSAGE_ID>...

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>...
          Message ids to file

Options:
      --summary <SUMMARY>
          Human/agent-authored summary for this notification entry

      --reason <REASON>
          Why this message belongs in this notification; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification list - List compact notification collections

```text
Usage: list

Options:
  -h, --help
          Print help
```

### Agent-First Mail notification show - Show notification collection index and entries

```text
Usage: show <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

### Agent-First Mail notification remove - Remove a notification message from this collection and return it to triage

```text
Usage: remove [OPTIONS] <NOTIFICATION_REF> <MESSAGE_ID>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>


Options:
      --reason <REASON>
          Why this message does not belong in this notification; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification move - Move a notification message to another notification collection

```text
Usage: move [OPTIONS] <NOTIFICATION_REF> <MESSAGE_ID> <NEW_NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Source notification collection ref

  <MESSAGE_ID>


  <NEW_NOTIFICATION_REF>
          Destination notification collection ref

Options:
      --reason <REASON>
          Why this collection is better; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification rename - Rename this notification collection's human-readable name without changing its UID

```text
Usage: rename [OPTIONS] --name <NAME> <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
      --name <NAME>
          New human-readable notification name

      --reason <REASON>
          Why this name better represents the collection; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification delete - Delete an empty notification collection

```text
Usage: delete [OPTIONS] <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
      --allow-notes
          Also delete notes.md when it contains user-authored notes

      --reason <REASON>
          Why this notification collection should be deleted; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification set-summary - Set or replace one notification entry summary

```text
Usage: set-summary [OPTIONS] --summary <SUMMARY> <NOTIFICATION_REF> <MESSAGE_ID>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>


Options:
      --summary <SUMMARY>


      --reason <REASON>
          Why this summary is useful; required by default

  -h, --help
          Print help
```

### Agent-First Mail notification notes - Show or edit notification collection notes

```text
Usage: notes <COMMAND>

Commands:
  show     Show notes markdown
  append   Append text to notes markdown
  replace  Replace notes markdown with text
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### Agent-First Mail notification notes show - Show notes markdown

```text
Usage: show <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### Agent-First Mail notification notes append - Append text to notes markdown

```text
Usage: append --text <TEXT> <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to append

  -h, --help
          Print help
```

#### Agent-First Mail notification notes replace - Replace notes markdown with text

```text
Usage: replace --text <TEXT> <NOTIFICATION_REF>

Arguments:
  <NOTIFICATION_REF>
          Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to write

  -h, --help
          Print help
```

## Agent-First Mail render - Rebuild generated read views from local workspace state

```text
Usage: render <COMMAND>

Commands:
  refresh    Rebuild generated case and notification read views
  templates  Export built-in language templates, keeping existing files unless forced
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail render refresh - Rebuild generated case and notification read views

```text
Usage: refresh

Options:
  -h, --help
          Print help
```

### Agent-First Mail render templates - Export built-in language templates, keeping existing files unless forced

```text
Usage: templates [OPTIONS]

Options:
      --force
          Overwrite existing workspace templates with built-in defaults

  -h, --help
          Print help
```

## Agent-First Mail log - Inspect the workspace audit log

```text
Usage: log <COMMAND>

Commands:
  list          List recent audit events
  tail          Tail recent audit events as JSON data
  message       List events for one message id
  case          List events for one case id
  notification  List events for one notification collection
  help          Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### Agent-First Mail log list - List recent audit events

```text
Usage: list [OPTIONS]

Options:
      --limit <LIMIT>
          Maximum number of events to return

          [default: 50]

  -h, --help
          Print help
```

### Agent-First Mail log tail - Tail recent audit events as JSON data

```text
Usage: tail

Options:
  -h, --help
          Print help
```

### Agent-First Mail log message - List events for one message id

```text
Usage: message <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>


Options:
  -h, --help
          Print help
```

### Agent-First Mail log case - List events for one case id

```text
Usage: case <CASE_UID>

Arguments:
  <CASE_UID>


Options:
  -h, --help
          Print help
```

### Agent-First Mail log notification - List events for one notification collection

```text
Usage: notification <NOTIFICATION_UID>

Arguments:
  <NOTIFICATION_UID>


Options:
  -h, --help
          Print help
```

## Agent-First Mail ui - Emit or open an Agent-First UI operation surface for this workspace

```text
Usage: ui [OPTIONS] [COMMAND]

Commands:
  snapshot  Emit a complete AFUI snapshot for this workspace
  help      Print this message or the help of the given subcommand(s)

Options:
      --no-open
          Do not open a browser when delegating to the AFUI Web Host

      --port <PORT>
          Web Host bind port. Defaults to 0, letting the OS choose a free port

          [default: 0]

      --host <HOST>
          Web Host bind address

          [default: 127.0.0.1]

      --afui-bin <AFUI_BIN>
          Explicit `afui` binary path or command name for `afmail ui`

      --no-termail
          Do not open the host-owned termail companion

  -h, --help
          Print help
```

### Agent-First Mail ui snapshot - Emit a complete AFUI snapshot for this workspace

```text
Usage: snapshot

Options:
  -h, --help
          Print help
```
AFDATA: 0.16.0
