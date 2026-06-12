<!-- Generated. Do not edit by hand. -->

# afmail CLI Reference

> Regenerate with `afmail --help --recursive --output markdown`.

# afmail - Agent-First Mail: local-first email case workspace for agents.

Agent-First Mail: local-first email case workspace for agents.

### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF` and
 `afmail archive case show REF`.
- stdout carries structured Agent-First Data events; stderr is not a protocol channel.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus durable state/remote sidecars
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/workspace.progress.json latest push/pull runtime snapshot
triage/message_*.md      active unprocessed message views
spam/*.md                generated spam review views
trash/*.md               generated trash review views
deleted/*.md             generated remote-deleted review views
cases/<group>/<case_uid>-<name>/case.md generated case entry view
cases/<group>/<case_uid>-<name>/data/ canonical case state
cases/<group>/<case_uid>-<name>/views/ generated case detail views
archive/cases/<case_uid>-<name>/ archived case workspaces
archive/notifications/<archive_uid>-<name>/archive.md generated archive entry view
archive/notifications/<archive_uid>-<name>/data/ canonical archive state
archive/notifications/<archive_uid>-<name>/views/ generated archive detail views
```

### Examples

```text
afmail init
afmail skill status
afmail skill install
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --message message_inbox_607146690_21 --group open --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 --reason "follow-up belongs to same feedback case"
afmail archive message create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail archive message show a20260603001
afmail archive message restore a20260603001 message_inbox_607146690_23 --reason "needs triage again"
afmail message spam message_inbox_607146690_23 --reason "phishing attempt"
afmail message trash message_inbox_607146690_24 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 waiting
afmail case archive c20260603001 --reason "feedback handled"
afmail archive case restore c20260603001 --group open --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail case reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case compose c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push drafts-send --dry-run
afmail push drafts-send
afmail push archive
afmail push spam
afmail push trash
afmail push list
afmail purge
afmail purge spam --older-than-days 30
afmail purge deleted
afmail log list --limit 20
afmail message show message_inbox_607146690_21
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: runtime/store/protocol error
- `2`: invalid CLI arguments

```text
Agent-First Mail: local-first email case workspace for agents.

### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF` and
 `afmail archive case show REF`.
- stdout carries structured Agent-First Data events; stderr is not a protocol channel.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus durable state/remote sidecars
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/workspace.progress.json latest push/pull runtime snapshot
triage/message_*.md      active unprocessed message views
spam/*.md                generated spam review views
trash/*.md               generated trash review views
deleted/*.md             generated remote-deleted review views
cases/<group>/<case_uid>-<name>/case.md generated case entry view
cases/<group>/<case_uid>-<name>/data/ canonical case state
cases/<group>/<case_uid>-<name>/views/ generated case detail views
archive/cases/<case_uid>-<name>/ archived case workspaces
archive/notifications/<archive_uid>-<name>/archive.md generated archive entry view
archive/notifications/<archive_uid>-<name>/data/ canonical archive state
archive/notifications/<archive_uid>-<name>/views/ generated archive detail views
```

### Examples

```text
afmail init
afmail skill status
afmail skill install
afmail status
afmail doctor
afmail pull
afmail pull sent archive
afmail remote folders
afmail case create --name 应用反馈-肥料登记 --message message_inbox_607146690_21 --group open --reason "new feedback thread"
afmail case show c20260603001
afmail case add c20260603001 message_inbox_607146690_22 --reason "follow-up belongs to same feedback case"
afmail archive message create --name 服务通知 --message message_inbox_607146690_23 --summary "billing notification" --reason "billing notification"
afmail archive message show a20260603001
afmail archive message restore a20260603001 message_inbox_607146690_23 --reason "needs triage again"
afmail message spam message_inbox_607146690_23 --reason "phishing attempt"
afmail message trash message_inbox_607146690_24 --reason "duplicate no longer needed"
afmail render refresh
afmail doctor repair --confirm
afmail case move c20260603001 waiting
afmail case archive c20260603001 --reason "feedback handled"
afmail archive case restore c20260603001 --group open --reason "customer replied"
afmail case tag c20260603001 legal --reason "legal review needed"
afmail case reply c20260603001 message_inbox_607146690_22
afmail case draft attach c20260603001 reply-message_inbox_607146690_22.md ./screenshot.png
afmail case draft validate c20260603001 reply-message_inbox_607146690_22.md
afmail case compose c20260603001 reply-message_inbox_607146690_22.md
afmail case draft remove c20260603001 reply-message_inbox_607146690_22.md --reason "mistaken draft"
afmail push drafts-send --dry-run
afmail push drafts-send
afmail push archive
afmail push spam
afmail push trash
afmail push list
afmail purge
afmail purge spam --older-than-days 30
afmail purge deleted
afmail log list --limit 20
afmail message show message_inbox_607146690_21
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: runtime/store/protocol error
- `2`: invalid CLI arguments

Usage: afmail [OPTIONS] [COMMAND]

Commands:
  init     Initialize the current directory as an afmail workspace
  pull     Read configured IMAP mailbox ids into local message files without changing remote mail
  config   Read or update local afmail configuration
  remote   Inspect remote IMAP state for configuring mailboxes
  push     Push queued local work or manage the local push queue
  status   Report workspace health, counts, and latest pull/push progress
  doctor   Check afmail workspace consistency without inspecting Git
  purge    Permanently delete old local spam, trash, and remote-deleted records
  skill    Manage the Agent-First Mail skill for Codex, Claude Code, and opencode
  triage   Inspect the triage queue
  message  Operate on any local message id
  case     Create a case or operate on an existing case ref
  archive  Inspect and manage archived cases and direct-message archive categories
  render   Rebuild generated read views from local workspace state
  log      Inspect the workspace audit log
  help     Print this message or the help of the given subcommand(s)

Options:
      --output <OUTPUT>
          Output format: json (default), yaml, plain

          [default: json]

      --log <LOG>
          Log categories (comma-separated): startup, request, progress, retry

      --verbose
          Enable all log categories

  -V, --version
          Print structured version event

  -h, --help
          Print help. Add --recursive to expand every nested subcommand; add --output json|yaml|markdown to render this help in another format.
```

## afmail init - Initialize the current directory as an afmail workspace

```text
Initialize the current directory as an afmail workspace

Usage: init

Options:
  -h, --help
          Print help
```

## afmail pull - Read configured IMAP mailbox ids into local message files without changing remote mail

```text
Read configured IMAP mailbox ids into local message files without changing remote mail

Usage: pull [IDS]...

Arguments:
  [IDS]...
          Configured mailbox ids to pull. With none, pulls actions.pull.default_mailbox_ids

Options:
  -h, --help
          Print help
```

## afmail config - Read or update local afmail configuration

```text
Read or update local afmail configuration

Usage: config <COMMAND>

Commands:
  show  Show local .afmail/config.json
  get   Get one config key
  set   Set one config key. Multiple values become an array for array keys
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail config show - Show local .afmail/config.json

```text
Show local .afmail/config.json

Usage: show

Options:
  -h, --help
          Print help
```

### afmail config get - Get one config key

```text
Get one config key

Usage: get <KEY>

Arguments:
  <KEY>


Options:
  -h, --help
          Print help
```

### afmail config set - Set one config key. Multiple values become an array for array keys

```text
Set one config key. Multiple values become an array for array keys

Usage: set <KEY> [VALUES]...

Arguments:
  <KEY>


  [VALUES]...


Options:
  -h, --help
          Print help
```

## afmail remote - Inspect remote IMAP state for configuring mailboxes

```text
Inspect remote IMAP state for configuring mailboxes

Usage: remote <COMMAND>

Commands:
  test     Test IMAP login using local config
  folders  List IMAP folders/mailboxes
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail remote test - Test IMAP login using local config

```text
Test IMAP login using local config

Usage: test

Options:
  -h, --help
          Print help
```

### afmail remote folders - List IMAP folders/mailboxes

```text
List IMAP folders/mailboxes

Usage: folders

Options:
  -h, --help
          Print help
```

## afmail push - Push queued local work or manage the local push queue

```text
Push queued local work or manage the local push queue

Usage: push [OPTIONS] [COMMAND]

Commands:
  list         List queued local push items
  drafts       Push queued outbound drafts through configured draft.save actions
  drafts-send  Push queued outbound drafts through configured draft.send actions
  archive      Push queued archive actions to their configured IMAP mailboxes
  spam         Push queued spam actions to the configured Junk mailbox
  trash        Push queued trash actions to the configured Trash mailbox
  help         Print this message or the help of the given subcommand(s)

Options:
      --dry-run
          Show planned push actions without IMAP/SMTP writes. This is the default

      --confirm
          Apply queued IMAP/SMTP effects

  -h, --help
          Print help
```

### afmail push list - List queued local push items

```text
List queued local push items

Usage: list

Options:
  -h, --help
          Print help
```

### afmail push drafts - Push queued outbound drafts through configured draft.save actions

```text
Push queued outbound drafts through configured draft.save actions

Usage: drafts [OPTIONS]

Options:
      --dry-run
          Show planned push actions without IMAP/SMTP writes. This is the default

      --confirm
          Apply queued IMAP/SMTP effects

  -h, --help
          Print help
```

### afmail push drafts-send - Push queued outbound drafts through configured draft.send actions

```text
Push queued outbound drafts through configured draft.send actions

Usage: drafts-send [OPTIONS]

Options:
      --dry-run
          Show planned push actions without IMAP/SMTP writes. This is the default

      --confirm
          Apply queued IMAP/SMTP effects, including sending mail

  -h, --help
          Print help
```

### afmail push archive - Push queued archive actions to their configured IMAP mailboxes

```text
Push queued archive actions to their configured IMAP mailboxes

Usage: archive [OPTIONS]

Options:
      --dry-run
          Show planned push actions without IMAP writes. This is the default

      --confirm
          Apply queued IMAP effects

  -h, --help
          Print help
```

### afmail push spam - Push queued spam actions to the configured Junk mailbox

```text
Push queued spam actions to the configured Junk mailbox

Usage: spam [OPTIONS]

Options:
      --dry-run
          Show planned push actions without IMAP writes. This is the default

      --confirm
          Apply queued IMAP effects

  -h, --help
          Print help
```

### afmail push trash - Push queued trash actions to the configured Trash mailbox

```text
Push queued trash actions to the configured Trash mailbox

Usage: trash [OPTIONS]

Options:
      --dry-run
          Show planned push actions without IMAP writes. This is the default

      --confirm
          Apply queued IMAP effects

  -h, --help
          Print help
```

## afmail status - Report workspace health, counts, and latest pull/push progress

```text
Report workspace health, counts, and latest pull/push progress

Usage: status

Options:
  -h, --help
          Print help
```

## afmail doctor - Check afmail workspace consistency without inspecting Git

```text
Check afmail workspace consistency without inspecting Git

Usage: doctor [COMMAND]

Commands:
  repair  Repair only unambiguous afmail-generated state
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail doctor repair - Repair only unambiguous afmail-generated state

```text
Repair only unambiguous afmail-generated state

Usage: repair [OPTIONS]

Options:
      --confirm
          Apply repair actions. Without this flag, repair is refused

  -h, --help
          Print help
```

## afmail purge - Permanently delete old local spam, trash, and remote-deleted records

```text
Permanently delete old local spam, trash, and remote-deleted records

Usage: purge [OPTIONS] [COMMAND]

Commands:
  spam     Permanently delete old local spam records
  trash    Permanently delete old local trash records
  deleted  Permanently delete old local records whose remote message disappeared
  help     Print this message or the help of the given subcommand(s)

Options:
      --older-than-days <DAYS>
          Only purge messages in a discard state at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

### afmail purge spam - Permanently delete old local spam records

```text
Permanently delete old local spam records

Usage: spam [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked spam at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

### afmail purge trash - Permanently delete old local trash records

```text
Permanently delete old local trash records

Usage: trash [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked trash at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

### afmail purge deleted - Permanently delete old local records whose remote message disappeared

```text
Permanently delete old local records whose remote message disappeared

Usage: deleted [OPTIONS]

Options:
      --older-than-days <DAYS>
          Only purge messages marked remote-deleted at least this many days ago. Defaults to 30

  -h, --help
          Print help
```

## afmail skill - Manage the Agent-First Mail skill for Codex, Claude Code, and opencode

```text
Manage the Agent-First Mail skill for Codex, Claude Code, and opencode

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

### afmail skill status - Show whether the Agent-First Mail skill is installed and valid

```text
Show whether the Agent-First Mail skill is installed and valid

Usage: status [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex local skill under $CODEX_HOME/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Project scope is supported for Claude Code and opencode, not Codex

          Possible values:
          - personal: Install under the user-level skills directory
          - project:  Install under the current project's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

  -h, --help
          Print help (see a summary with '-h')
```

### afmail skill install - Install the Agent-First Mail skill

```text
Install the Agent-First Mail skill

Usage: install [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex local skill under $CODEX_HOME/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Project scope is supported for Claude Code and opencode, not Codex

          Possible values:
          - personal: Install under the user-level skills directory
          - project:  Install under the current project's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

      --force
          Overwrite or remove an unmanaged Agent-First Mail skill at the target path

  -h, --help
          Print help (see a summary with '-h')
```

### afmail skill uninstall - Remove an afmail-managed Agent-First Mail skill

```text
Remove an afmail-managed Agent-First Mail skill

Usage: uninstall [OPTIONS]

Options:
      --agent <AGENT>
          Agent to manage. Defaults to all personal skill targets

          Possible values:
          - all:         Manage every agent that supports the requested scope
          - codex:       Manage the Codex local skill under $CODEX_HOME/skills
          - claude-code: Manage the Claude Code skill under ~/.claude/skills or .claude/skills
          - opencode:    Manage the opencode skill under ~/.config/opencode/skills or .opencode/skills

          [default: all]

      --scope <SCOPE>
          Skill scope. Project scope is supported for Claude Code and opencode, not Codex

          Possible values:
          - personal: Install under the user-level skills directory
          - project:  Install under the current project's skills directory

          [default: personal]

      --skills-dir <SKILLS_DIR>
          Directory that contains skill folders. Requires an explicit single --agent

      --force
          Overwrite or remove an unmanaged Agent-First Mail skill at the target path

  -h, --help
          Print help (see a summary with '-h')
```

## afmail triage - Inspect the triage queue

```text
Inspect the triage queue

Usage: triage <COMMAND>

Commands:
  list  List compact untriaged message locators
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail triage list - List compact untriaged message locators

```text
List compact untriaged message locators

Usage: list

Options:
  -h, --help
          Print help
```

## afmail message - Operate on any local message id

```text
Operate on any local message id

Usage: message <COMMAND>

Commands:
  show        Show the full local message metadata, body text, and attachment metadata
  archive     File this message into a direct-message archive category and queue eligible IMAP moves
  spam        Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move
  unspam      Undo a local spam mark before queued remote effects are pushed
  trash       Explicitly discard this message locally, show it under trash/, and queue a Trash move
  untrash     Undo a local trash mark before queued remote effects are pushed
  unarchive   Restore a directly archived message to triage
  attachment  Fetch an attachment for this message
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail message show - Show the full local message metadata, body text, and attachment metadata

```text
Show the full local message metadata, body text, and attachment metadata

Usage: show <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
  -h, --help
          Print help
```

### afmail message archive - File this message into a direct-message archive category and queue eligible IMAP moves

```text
File this message into a direct-message archive category and queue eligible IMAP moves

Usage: archive [OPTIONS] --summary <SUMMARY> <MESSAGE_ID> <ARCHIVE_REF>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
      --summary <SUMMARY>
          Human/agent-authored summary for this archive entry

      --reason <REASON>
          Why this disposition is correct; required by default

  -h, --help
          Print help
```

### afmail message spam - Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move

```text
Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move

Usage: spam [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this disposition is correct; required by default

  -h, --help
          Print help
```

### afmail message unspam - Undo a local spam mark before queued remote effects are pushed

```text
Undo a local spam mark before queued remote effects are pushed

Usage: unspam [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this spam mark should be undone; required by default

  -h, --help
          Print help
```

### afmail message trash - Explicitly discard this message locally, show it under trash/, and queue a Trash move

```text
Explicitly discard this message locally, show it under trash/, and queue a Trash move

Usage: trash [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this disposition is correct; required by default

  -h, --help
          Print help
```

### afmail message untrash - Undo a local trash mark before queued remote effects are pushed

```text
Undo a local trash mark before queued remote effects are pushed

Usage: untrash [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this trash mark should be undone; required by default

  -h, --help
          Print help
```

### afmail message unarchive - Restore a directly archived message to triage

```text
Restore a directly archived message to triage

Usage: unarchive [OPTIONS] <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

Options:
      --reason <REASON>
          Why this archive should be undone; required by default

  -h, --help
          Print help
```

### afmail message attachment - Fetch an attachment for this message

```text
Fetch an attachment for this message

Usage: attachment <COMMAND>

Commands:
  fetch  Fetch one attachment by MIME part id, or all attachments when omitted
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### afmail message attachment fetch - Fetch one attachment by MIME part id, or all attachments when omitted

```text
Fetch one attachment by MIME part id, or all attachments when omitted

Usage: fetch <MESSAGE_ID> [PART_ID]

Arguments:
  <MESSAGE_ID>
          Message id, for example message_inbox_607146690_22

  [PART_ID]


Options:
  -h, --help
          Print help
```

## afmail case - Create a case or operate on an existing case ref

```text
Create a case or operate on an existing case ref

Usage: case <COMMAND>

Commands:
  create   Create a new case and return its stable UID/ref
  list     List compact active case locators
  show     Show the active case's case.md without changing workspace state
  add      Add a message to this existing case
  move     Move this case to another group
  rename   Rename this active case's human-readable name without changing its UID
  notes    Show or edit active case notes
  archive  Archive this active case
  reopen   Reopen this case as active work without changing its tags
  tag      Add a case organization tag
  untag    Remove a case organization tag
  draft    Create, validate, attach to, or remove local case drafts
  compose  Compose an existing case draft into the local push queue
  reply    Scaffold a reply draft to a message, prefilled and quoting the original
  merge    Merge another case into this case
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail case create - Create a new case and return its stable UID/ref

```text
Create a new case and return its stable UID/ref

Usage: create [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          Human-readable case name used in case.md and the directory suffix

      --group <GROUP>
          Destination group. Defaults to the configured default group

      --message <MESSAGE>
          Optional first message to add to the case

      --reason <REASON>
          Why this case is being created

  -h, --help
          Print help
```

### afmail case list - List compact active case locators

```text
List compact active case locators

Usage: list

Options:
  -h, --help
          Print help
```

### afmail case show - Show the active case's case.md without changing workspace state

```text
Show the active case's case.md without changing workspace state

Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

### afmail case add - Add a message to this existing case

```text
Add a message to this existing case

Usage: add [OPTIONS] <CASE_REF> <MESSAGE_ID>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>
          Message id to add

Options:
      --reason <REASON>
          Why this message belongs in this case; required by default

  -h, --help
          Print help
```

### afmail case move - Move this case to another group

```text
Move this case to another group

Usage: move <CASE_REF> <GROUP>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <GROUP>
          Destination group

Options:
  -h, --help
          Print help
```

### afmail case rename - Rename this active case's human-readable name without changing its UID

```text
Rename this active case's human-readable name without changing its UID

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

### afmail case notes - Show or edit active case notes

```text
Show or edit active case notes

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

#### afmail case notes show - Show notes markdown

```text
Show notes markdown

Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### afmail case notes append - Append text to notes markdown

```text
Append text to notes markdown

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

#### afmail case notes replace - Replace notes markdown with text

```text
Replace notes markdown with text

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

### afmail case archive - Archive this active case

```text
Archive this active case

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

### afmail case reopen - Reopen this case as active work without changing its tags

```text
Reopen this case as active work without changing its tags

Usage: reopen [OPTIONS] <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --reason <REASON>
          Why this case should be reopened; required by default

  -h, --help
          Print help
```

### afmail case tag - Add a case organization tag

```text
Add a case organization tag

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

### afmail case untag - Remove a case organization tag

```text
Remove a case organization tag

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

### afmail case draft - Create, validate, attach to, or remove local case drafts

```text
Create, validate, attach to, or remove local case drafts

Usage: draft <COMMAND>

Commands:
  new       Scaffold a new outbound draft (not a reply) in this case
  validate  Validate a draft under the case drafts directory
  attach    Copy or reference a file and add it to a draft's attachments
  remove    Remove a local draft and any queued outbound item for it
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### afmail case draft new - Scaffold a new outbound draft (not a reply) in this case

```text
Scaffold a new outbound draft (not a reply) in this case

Usage: new [OPTIONS] <CASE_REF>

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

  -h, --help
          Print help
```

#### afmail case draft validate - Validate a draft under the case drafts directory

```text
Validate a draft under the case drafts directory

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

#### afmail case draft attach - Copy or reference a file and add it to a draft's attachments

```text
Copy or reference a file and add it to a draft's attachments

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

#### afmail case draft remove - Remove a local draft and any queued outbound item for it

```text
Remove a local draft and any queued outbound item for it

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

### afmail case compose - Compose an existing case draft into the local push queue

```text
Compose an existing case draft into the local push queue

Usage: compose <CASE_REF> <DRAFT_NAME>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <DRAFT_NAME>
          Draft markdown file under the case drafts directory

Options:
  -h, --help
          Print help
```

### afmail case reply - Scaffold a reply draft to a message, prefilled and quoting the original

```text
Scaffold a reply draft to a message, prefilled and quoting the original

Usage: reply [OPTIONS] <CASE_REF> <MESSAGE_ID>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>
          Message id in this case to reply to

Options:
      --all
          Reply to all original recipients (To and Cc), not just the sender

  -h, --help
          Print help
```

### afmail case merge - Merge another case into this case

```text
Merge another case into this case

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

## afmail archive - Inspect and manage archived cases and direct-message archive categories

```text
Inspect and manage archived cases and direct-message archive categories

Usage: archive <COMMAND>

Commands:
  list     List compact archived cases and/or direct-message archive categories
  message  Operate on a direct-message archive category
  case     Operate on an archived case
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail archive list - List compact archived cases and/or direct-message archive categories

```text
List compact archived cases and/or direct-message archive categories

Usage: list [COMMAND]

Commands:
  cases     List compact archived cases
  messages  List compact direct-message archive categories
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### afmail archive list cases - List compact archived cases

```text
List compact archived cases

Usage: cases

Options:
  -h, --help
          Print help
```

#### afmail archive list messages - List compact direct-message archive categories

```text
List compact direct-message archive categories

Usage: messages

Options:
  -h, --help
          Print help
```

### afmail archive message - Operate on a direct-message archive category

```text
Operate on a direct-message archive category

Usage: message <COMMAND>

Commands:
  create       Create a direct-message archive category and optionally file one message
  show         Show archive category index and entries
  restore      Restore a direct archived message to triage
  move         Move a direct archived message to another archive category
  rename       Rename this archive category's human-readable name without changing its UID
  set-summary  Set or replace one direct archive entry summary
  notes        Show or edit archive category notes
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### afmail archive message create - Create a direct-message archive category and optionally file one message

```text
Create a direct-message archive category and optionally file one message

Usage: create [OPTIONS] --name <NAME>

Options:
      --name <NAME>
          Human-readable archive category name used in metadata and the directory suffix

      --message <MESSAGE>
          Optional message to immediately file into the new archive category

      --summary <SUMMARY>
          Required when --message is supplied

      --reason <REASON>
          Why this archive category is being created

  -h, --help
          Print help
```

#### afmail archive message show - Show archive category index and entries

```text
Show archive category index and entries

Usage: show <ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### afmail archive message restore - Restore a direct archived message to triage

```text
Restore a direct archived message to triage

Usage: restore [OPTIONS] <ARCHIVE_REF> <MESSAGE_ID>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>


Options:
      --reason <REASON>
          Why this message needs active triage again; required by default

  -h, --help
          Print help
```

#### afmail archive message move - Move a direct archived message to another archive category

```text
Move a direct archived message to another archive category

Usage: move [OPTIONS] <ARCHIVE_REF> <MESSAGE_ID> <NEW_ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Source direct-message archive category ref

  <MESSAGE_ID>


  <NEW_ARCHIVE_REF>
          Destination archive category ref

Options:
      --reason <REASON>
          Why this category is better; required by default

  -h, --help
          Print help
```

#### afmail archive message rename - Rename this archive category's human-readable name without changing its UID

```text
Rename this archive category's human-readable name without changing its UID

Usage: rename [OPTIONS] --name <NAME> <ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
      --name <NAME>
          New human-readable archive name

      --reason <REASON>
          Why this name better represents the category; required by default

  -h, --help
          Print help
```

#### afmail archive message set-summary - Set or replace one direct archive entry summary

```text
Set or replace one direct archive entry summary

Usage: set-summary [OPTIONS] --summary <SUMMARY> <ARCHIVE_REF> <MESSAGE_ID>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

  <MESSAGE_ID>


Options:
      --summary <SUMMARY>


      --reason <REASON>
          Why this summary is useful; required by default

  -h, --help
          Print help
```

#### afmail archive message notes - Show or edit archive category notes

```text
Show or edit archive category notes

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

##### afmail archive message notes show - Show notes markdown

```text
Show notes markdown

Usage: show <ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

##### afmail archive message notes append - Append text to notes markdown

```text
Append text to notes markdown

Usage: append --text <TEXT> <ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to append

  -h, --help
          Print help
```

##### afmail archive message notes replace - Replace notes markdown with text

```text
Replace notes markdown with text

Usage: replace --text <TEXT> <ARCHIVE_REF>

Arguments:
  <ARCHIVE_REF>
          Direct-message archive category ref: aYYYYMMDDNNN or aYYYYMMDDNNN-any-suffix

Options:
      --text <TEXT>
          Markdown text to write

  -h, --help
          Print help
```

### afmail archive case - Operate on an archived case

```text
Operate on an archived case

Usage: case <COMMAND>

Commands:
  show     Show the archived case
  restore  Restore an archived case to an active case group
  rename   Rename this archived case's human-readable name without changing its UID
  notes    Show or edit archived case notes
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### afmail archive case show - Show the archived case

```text
Show the archived case

Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

#### afmail archive case restore - Restore an archived case to an active case group

```text
Restore an archived case to an active case group

Usage: restore [OPTIONS] --group <GROUP> <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
      --group <GROUP>
          Active case group to restore into

      --reason <REASON>
          Why this case needs active attention again; required by default

  -h, --help
          Print help
```

#### afmail archive case rename - Rename this archived case's human-readable name without changing its UID

```text
Rename this archived case's human-readable name without changing its UID

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

#### afmail archive case notes - Show or edit archived case notes

```text
Show or edit archived case notes

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

##### afmail archive case notes show - Show notes markdown

```text
Show notes markdown

Usage: show <CASE_REF>

Arguments:
  <CASE_REF>
          Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix

Options:
  -h, --help
          Print help
```

##### afmail archive case notes append - Append text to notes markdown

```text
Append text to notes markdown

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

##### afmail archive case notes replace - Replace notes markdown with text

```text
Replace notes markdown with text

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

## afmail render - Rebuild generated read views from local workspace state

```text
Rebuild generated read views from local workspace state

Usage: render <COMMAND>

Commands:
  refresh    Rebuild generated case and direct-message archive read views
  templates  Export built-in language templates, keeping existing files unless forced
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail render refresh - Rebuild generated case and direct-message archive read views

```text
Rebuild generated case and direct-message archive read views

Usage: refresh

Options:
  -h, --help
          Print help
```

### afmail render templates - Export built-in language templates, keeping existing files unless forced

```text
Export built-in language templates, keeping existing files unless forced

Usage: templates [OPTIONS]

Options:
      --force
          Overwrite existing workspace templates with built-in defaults

  -h, --help
          Print help
```

## afmail log - Inspect the workspace audit log

```text
Inspect the workspace audit log

Usage: log <COMMAND>

Commands:
  list     List recent audit events
  tail     Tail recent audit events as JSON data
  message  List events for one message id
  case     List events for one case id
  archive  List events for one archive category
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

### afmail log list - List recent audit events

```text
List recent audit events

Usage: list [OPTIONS]

Options:
      --limit <LIMIT>
          Maximum number of events to return

          [default: 50]

  -h, --help
          Print help
```

### afmail log tail - Tail recent audit events as JSON data

```text
Tail recent audit events as JSON data

Usage: tail

Options:
  -h, --help
          Print help
```

### afmail log message - List events for one message id

```text
List events for one message id

Usage: message <MESSAGE_ID>

Arguments:
  <MESSAGE_ID>


Options:
  -h, --help
          Print help
```

### afmail log case - List events for one case id

```text
List events for one case id

Usage: case <CASE_UID>

Arguments:
  <CASE_UID>


Options:
  -h, --help
          Print help
```

### afmail log archive - List events for one archive category

```text
List events for one archive category

Usage: archive <ARCHIVE_UID>

Arguments:
  <ARCHIVE_UID>


Options:
  -h, --help
          Print help
```
