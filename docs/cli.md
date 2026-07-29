<!-- Generated. Do not edit by hand. Regenerate: afmail --help --recursive --output markdown -->

# afmail CLI Reference

# afmail - Let your AI agent work your inbox — email pulled into plain files it reads, sorts, and drafts on your machine, with nothing sent until you confirm.

### Interface Policy

- Files are the read interface; CLI is for effects.
- One workspace represents one mailbox account.
- Message commands use `afmail message ACTION MESSAGE_ID ...`.
- Case commands use `afmail case ACTION CASE_REF ...`.
- Active and archived cases are readable with `afmail case show REF`.
- `--output-to split` sends results to stdout and errors/logs/progress to stderr.
- `--output-to stdout|stderr` selects one ordered Agent-First Data event stream.

### Workspace Shape

```text
.afmail/messages/        raw .eml plus canonical message metadata
.afmail/workspace.json   immutable workspace identity used by plan digests
messages/               rebuildable parsed message cache
.afmail/logs/events.jsonl append-only audit log
.afmail/transactions/    transient local write transaction sentinels
.afmail/cache/progress.json latest push/pull runtime snapshot
templates/               user-editable Markdown and Review templates
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
afmail api export --directory openapi --force
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
afmail message list --sender alice@example.com --subject-contains invoice --since 2026-05-21 --in all
afmail message attachment fetch message_inbox_607146690_21 2
```

### Exit Codes

- `0`: command completed successfully
- `1`: command failed, including an aggregate command where no item completed successfully
- `2`: invalid CLI arguments
- `3`: command completed partially; inspect the canonical error for successful and failed items
- `4`: a canonical output event was not fully written

Only completely successful commands emit `kind: result`; their payload has no generic `code`, `ok`,
or `outcome` field. Typed review results retain `outcome` as a business lifecycle field. Any item
failure emits `kind: error`. Partial failures use `error.code: partial`,
and the complete item list is carried in `error.items`; a failed item has an `error` child object.
Exit code `4` means the command could not deliver a complete protocol event;
inspect the selected output sink for the fallback diagnostic.

```text
afmail [OPTIONS] [COMMAND]
```

| Argument | Description |
| --- | --- |
| `--output <OUTPUT>` | Output format: json (default), yaml, plain *(global, default: `json`)* |
| `--output-to <OUTPUT_TO>` | Where protocol events go: split, stdout, or stderr *(global)* |
| `--log <LOG>...` | Log categories (comma-separated): startup, request, progress, retry *(global)* |
| `--verbose` | Enable all log categories. Startup argv redacts values and non-command positional tokens *(global)* |
| `--stdout-file <PATH>` | Redirect stdout bytes to this file *(global)* |
| `--stderr-file <PATH>` | Redirect stderr bytes to this file *(global)* |
| `--workspace <PATH>` | Operate on this existing afmail workspace instead of discovering from cwd *(global)* |
| `--request-id <ID>` | Idempotency key for supported non-idempotent local mutations *(global)* |
| `--version` | Print structured version event *(global)* |
| `--help` | Print help. Add --recursive to expand every nested subcommand; add --output plain\|json\|yaml\|markdown to choose the format. |

| Command | Summary |
| --- | --- |
| `afmail api` | Serve or export the afmail OpenAPI 3.2 contract and JSON Schemas |
| `afmail demo` | Create and run a complete local mailbox demo |
| `afmail init` | Initialize the current directory or a child path as an afmail workspace |
| `afmail pull` | Read configured IMAP mailbox ids into local message files without changing remote mail |
| `afmail config` | Read or update local afmail configuration |
| `afmail remote` | Inspect remote IMAP state for configuring mailboxes |
| `afmail push` | Push queued local work or manage the local push queue |
| `afmail status` | Report workspace health, counts, and latest pull/push progress |
| `afmail doctor` | Check afmail workspace consistency without inspecting Git |
| `afmail purge` | Permanently delete old local spam, trash, and remote-deleted records |
| `afmail skill` | Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes |
| `afmail triage` | Inspect the triage queue |
| `afmail message` | Operate on any local message id |
| `afmail case` | Create a case or operate on an existing case ref |
| `afmail contact` | Create a contact or operate on an existing contact ref |
| `afmail notification` | Inspect and manage notification collections |
| `afmail render` | Rebuild generated read views from local workspace state |
| `afmail log` | Inspect the workspace audit log |

## afmail api - Serve or export the afmail OpenAPI 3.2 contract and JSON Schemas

```text
afmail api <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail api serve` | Serve the typed afmail HTTP API and its OpenAPI/JSON Schema documents |
| `afmail api export` | Write the generated OpenAPI document and standalone JSON Schemas |

### afmail api serve - Serve the typed afmail HTTP API and its OpenAPI/JSON Schema documents

```text
afmail api serve [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--mode <MODE>` | Network exposure mode *(default: `local`)* |
| `--port <PORT>` | TCP port. Use 0 to let the operating system choose an available port *(default: `9417`)* |
| `--access-token-secret <ACCESS_TOKEN_SECRET>` | Bearer credential. Prefer AFMAIL_API_ACCESS_TOKEN_SECRET to avoid argv exposure |

### afmail api export - Write the generated OpenAPI document and standalone JSON Schemas

```text
afmail api export [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--directory <DIRECTORY>` | Destination directory *(default: `openapi`)* |
| `--force` | Replace existing generated files |

## afmail demo - Create and run a complete local mailbox demo

```text
afmail demo <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail demo init` | Create a normal workspace wired to the local demo mail server |
| `afmail demo serve` | Run the local demo IMAP and SMTP servers until interrupted |

### afmail demo init - Create a normal workspace wired to the local demo mail server

```text
afmail demo init [OPTIONS] [PATH]
```

| Argument | Description |
| --- | --- |
| `PATH` | Workspace path under the current directory *(default: `afmail-demo`)* |
| `--scenario <SCENARIO>` | Built-in mailbox scenario *(default: `founder-inbox`)* |
| `--imap-port <IMAP_PORT>` | Local IMAP port *(default: `14300`)* |
| `--smtp-port <SMTP_PORT>` | Local SMTP port *(default: `14301`)* |

### afmail demo serve - Run the local demo IMAP and SMTP servers until interrupted

```text
afmail demo serve
```

## afmail init - Initialize the current directory or a child path as an afmail workspace

```text
afmail init [PATH]
```

| Argument | Description |
| --- | --- |
| `PATH` | Optional workspace path to initialize under the current directory |

## afmail pull - Read configured IMAP mailbox ids into local message files without changing remote mail

```text
afmail pull [OPTIONS] [IDS]...
```

| Argument | Description |
| --- | --- |
| `IDS...` | Configured mailbox ids to pull. With none, pulls actions.pull.default_mailbox_ids |
| `--dry-run` | Estimate remote candidates and budgets without downloading or writing locally |
| `--since-days <DAYS>` | Limit the remote candidate range to messages from the last DAYS days |
| `--max-new-messages <COUNT>` | Maximum number of new messages stored across all selected mailboxes |
| `--max-total-bytes <BYTES>` | Maximum new raw message bytes stored across all selected mailboxes |
| `--all-history` | Explicitly include all remote history instead of the default initial window |

## afmail config - Read or update local afmail configuration

```text
afmail config <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail config show` | Show effective config after applying built-in defaults |
| `afmail config get` | Get one config key |
| `afmail config set` | Set one config key. Multiple values become an array for array keys |
| `afmail config add` | Add an element to a keyed list (e.g. `identities`). FIELDS are key=value |
| `afmail config remove` | Remove a keyed-list element by its slug |

### afmail config show - Show effective config after applying built-in defaults

```text
afmail config show
```

### afmail config get - Get one config key

```text
afmail config get <KEY>
```

| Argument | Description |
| --- | --- |
| `KEY` | *(required)* |

### afmail config set - Set one config key. Multiple values become an array for array keys

```text
afmail config set <KEY> [VALUES]...
```

| Argument | Description |
| --- | --- |
| `KEY` | *(required)* |
| `VALUES...` |  |

### afmail config add - Add an element to a keyed list (e.g. `identities`). FIELDS are key=value

```text
afmail config add <KEY> <SLUG> [FIELDS]...
```

| Argument | Description |
| --- | --- |
| `KEY` | *(required)* |
| `SLUG` | *(required)* |
| `FIELDS...` |  |

### afmail config remove - Remove a keyed-list element by its slug

```text
afmail config remove <KEY> <SLUG>
```

| Argument | Description |
| --- | --- |
| `KEY` | *(required)* |
| `SLUG` | *(required)* |

## afmail remote - Inspect remote IMAP state for configuring mailboxes

```text
afmail remote <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail remote test` | Test configured IMAP and SMTP access without sending mail |
| `afmail remote folders` | List IMAP folders/mailboxes |

### afmail remote test - Test configured IMAP and SMTP access without sending mail

```text
afmail remote test
```

### afmail remote folders - List IMAP folders/mailboxes

```text
afmail remote folders
```

## afmail push - Push queued local work or manage the local push queue

```text
afmail push [OPTIONS] [COMMAND]
```

| Argument | Description |
| --- | --- |
| `--dry-run` | Show planned push actions without IMAP/SMTP writes. This is the default |
| `--confirm <PLAN_ID>` | Apply exactly the reviewed plan returned by push preview |

| Command | Summary |
| --- | --- |
| `afmail push list` | Preview the complete resolved push plan without writing |

### afmail push list - Preview the complete resolved push plan without writing

```text
afmail push list
```

## afmail status - Report workspace health, counts, and latest pull/push progress

```text
afmail status
```

## afmail doctor - Check afmail workspace consistency without inspecting Git

```text
afmail doctor [COMMAND]
```

| Command | Summary |
| --- | --- |
| `afmail doctor repair` | Repair only unambiguous afmail-generated state |

### afmail doctor repair - Repair only unambiguous afmail-generated state

```text
afmail doctor repair [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--confirm` | Apply repair actions. Without this flag, repair is refused |

## afmail purge - Permanently delete old local spam, trash, and remote-deleted records

```text
afmail purge [OPTIONS] [COMMAND]
```

| Argument | Description |
| --- | --- |
| `--older-than-days <DAYS>` | Only purge messages in a discard state at least this many days ago. Defaults to 30 |
| `--confirm <PLAN_ID>` | Permanently delete exactly the reviewed purge plan *(global)* |

| Command | Summary |
| --- | --- |
| `afmail purge spam` | Permanently delete old local spam records |
| `afmail purge trash` | Permanently delete old local trash records |
| `afmail purge deleted` | Permanently delete old local records whose remote message disappeared |

### afmail purge spam - Permanently delete old local spam records

```text
afmail purge spam [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--older-than-days <DAYS>` | Only purge messages marked spam at least this many days ago. Defaults to 30 |

### afmail purge trash - Permanently delete old local trash records

```text
afmail purge trash [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--older-than-days <DAYS>` | Only purge messages marked trash at least this many days ago. Defaults to 30 |

### afmail purge deleted - Permanently delete old local records whose remote message disappeared

```text
afmail purge deleted [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--older-than-days <DAYS>` | Only purge messages marked remote-deleted at least this many days ago. Defaults to 30 |

## afmail skill - Manage the Agent-First Mail skill for Codex, Claude Code, opencode, and Hermes

```text
afmail skill <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail skill status` | Show whether the Agent-First Mail skill is installed and valid |
| `afmail skill install` | Install the Agent-First Mail skill |
| `afmail skill uninstall` | Remove an afmail-managed Agent-First Mail skill |

### afmail skill status - Show whether the Agent-First Mail skill is installed and valid

```text
afmail skill status [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--agent <AGENT>` | Agent to manage. Defaults to all personal skill targets *(default: `all`)* |
| `--scope <SCOPE>` | Skill scope. Workspace scope installs under the current workspace's skill directory *(default: `personal`)* |
| `--skills-dir <SKILLS_DIR>` | Directory that contains skill folders. Requires an explicit single --agent |

### afmail skill install - Install the Agent-First Mail skill

```text
afmail skill install [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--agent <AGENT>` | Agent to manage. Defaults to all personal skill targets *(default: `all`)* |
| `--scope <SCOPE>` | Skill scope. Workspace scope installs under the current workspace's skill directory *(default: `personal`)* |
| `--skills-dir <SKILLS_DIR>` | Directory that contains skill folders. Requires an explicit single --agent |
| `--force` | Overwrite or remove an unmanaged Agent-First Mail skill at the target path |

### afmail skill uninstall - Remove an afmail-managed Agent-First Mail skill

```text
afmail skill uninstall [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--agent <AGENT>` | Agent to manage. Defaults to all personal skill targets *(default: `all`)* |
| `--scope <SCOPE>` | Skill scope. Workspace scope installs under the current workspace's skill directory *(default: `personal`)* |
| `--skills-dir <SKILLS_DIR>` | Directory that contains skill folders. Requires an explicit single --agent |
| `--force` | Overwrite or remove an unmanaged Agent-First Mail skill at the target path |

## afmail triage - Inspect the triage queue

```text
afmail triage <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail triage list` | List compact untriaged message locators |
| `afmail triage suggestion` | Set or clear the current typed Agent suggestion for a message |
| `afmail triage review` | Open a review session for persisted triage suggestions |

### afmail triage list - List compact untriaged message locators

```text
afmail triage list [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--limit <LIMIT>` | Maximum number of newest triage messages to return |
| `--details` | Include message metadata, related message ids, and case refs |

### afmail triage suggestion - Set or clear the current typed Agent suggestion for a message

```text
afmail triage suggestion <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail triage suggestion set` | Replace the current suggestion and clear any human review of the prior round |
| `afmail triage suggestion clear` | Clear exactly the current suggestion round |

#### afmail triage suggestion set - Replace the current suggestion and clear any human review of the prior round

```text
afmail triage suggestion set [OPTIONS] --message-review-base-hash <MESSAGE_REVIEW_BASE_HASH> --reason <REASON> <MESSAGE_ID> <COMMAND>
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID` | Message id currently in triage *(required)* |
| `--message-review-base-hash <MESSAGE_REVIEW_BASE_HASH>` | Current hash returned by `afmail message show` *(required)* |
| `--reason <REASON>` | Why this complete action is being suggested *(required)* |
| `--confidence-percent <CONFIDENCE_PERCENT>` | Optional confidence from 0 through 100 |

| Command | Summary |
| --- | --- |
| `afmail triage suggestion set case-create` |  |
| `afmail triage suggestion set case-add` |  |
| `afmail triage suggestion set notification-create` |  |
| `afmail triage suggestion set notification-add` |  |
| `afmail triage suggestion set spam` |  |
| `afmail triage suggestion set trash` |  |

##### afmail triage suggestion set case-create

```text
afmail triage suggestion set case-create --name <NAME> --group <GROUP>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | *(required)* |
| `--group <GROUP>` | *(required)* |

##### afmail triage suggestion set case-add

```text
afmail triage suggestion set case-add <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | *(required)* |

##### afmail triage suggestion set notification-create

```text
afmail triage suggestion set notification-create --name <NAME> --summary <SUMMARY>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | *(required)* |
| `--summary <SUMMARY>` | *(required)* |

##### afmail triage suggestion set notification-add

```text
afmail triage suggestion set notification-add --summary <SUMMARY> <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | *(required)* |
| `--summary <SUMMARY>` | *(required)* |

##### afmail triage suggestion set spam

```text
afmail triage suggestion set spam
```

##### afmail triage suggestion set trash

```text
afmail triage suggestion set trash
```

#### afmail triage suggestion clear - Clear exactly the current suggestion round

```text
afmail triage suggestion clear --agent-suggestion-uid <AGENT_SUGGESTION_UID> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID` | Message id whose current suggestion round should be cleared *(required)* |
| `--agent-suggestion-uid <AGENT_SUGGESTION_UID>` | UID returned by the current suggestion set command *(required)* |

### afmail triage review - Open a review session for persisted triage suggestions

```text
afmail triage review [OPTIONS] [MESSAGE_ID]...
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID...` | Message ids to review. With none, review every current triage suggestion |
| `--mode <MODE>` | Review presentation mode *(default: `local`)* |

## afmail message - Operate on any local message id

```text
afmail message <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail message show` | Show the exact typed message document used by Markdown and Review templates |
| `afmail message list` | List messages by sender, subject, date, or workspace scope |
| `afmail message spam` | Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move |
| `afmail message trash` | Explicitly discard this message locally, show it under trash/, and queue a Trash move |
| `afmail message unspam` | Mark a spam false positive as triage and queue a move out of Junk when needed |
| `afmail message untrash` | Mark a trash false positive as triage and queue a move out of Trash when needed |
| `afmail message attachment` | Fetch an attachment for this message |

### afmail message show - Show the exact typed message document used by Markdown and Review templates

```text
afmail message show <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID` | Message id, for example message_inbox_607146690_22 *(required)* |

### afmail message list - List messages by sender, subject, date, or workspace scope

```text
afmail message list [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--sender <SENDER>` | Only include messages whose sender or from line contains this text |
| `--subject-contains <SUBJECT_CONTAINS>` | Only include messages whose subject contains this text |
| `--since <DATE>` | Only include messages on or after this date or RFC3339 timestamp |
| `--in <SCOPE>` | Limit results to one workspace scope *(default: `all`)* |

### afmail message spam - Mark junk/phishing/malware/suspicious mail locally, show it under spam/, and queue a Junk move

```text
afmail message spam [OPTIONS] <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID...` | Message ids, for example message_inbox_607146690_22 *(required)* |
| `--reason <REASON>` | Why this disposition is correct; required by default |

### afmail message trash - Explicitly discard this message locally, show it under trash/, and queue a Trash move

```text
afmail message trash [OPTIONS] <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID...` | Message ids, for example message_inbox_607146690_22 *(required)* |
| `--reason <REASON>` | Why this disposition is correct; required by default |

### afmail message unspam - Mark a spam false positive as triage and queue a move out of Junk when needed

```text
afmail message unspam [OPTIONS] <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID...` | Message ids, for example message_inbox_607146690_22 *(required)* |
| `--reason <REASON>` | Why this message is not spam; required by default |

### afmail message untrash - Mark a trash false positive as triage and queue a move out of Trash when needed

```text
afmail message untrash [OPTIONS] <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID...` | Message ids, for example message_inbox_607146690_22 *(required)* |
| `--reason <REASON>` | Why this message should not be trashed; required by default |

### afmail message attachment - Fetch an attachment for this message

```text
afmail message attachment <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail message attachment fetch` | Fetch one attachment by MIME part id, or all attachments when omitted |

#### afmail message attachment fetch - Fetch one attachment by MIME part id, or all attachments when omitted

```text
afmail message attachment fetch <MESSAGE_ID> [PART_ID]
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID` | Message id, for example message_inbox_607146690_22 *(required)* |
| `PART_ID` |  |

## afmail case - Create a case or operate on an existing case ref

```text
afmail case <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail case create` | Create a new case and return its stable UID/ref |
| `afmail case list` | List compact case locators |
| `afmail case show` | Show the exact typed case collection document used by Markdown and Review |
| `afmail case review` | Open a review session for this case, including editable notes |
| `afmail case suggestion` | Set or clear the current typed Agent suggestion for this case |
| `afmail case add` | Add a message to this existing case |
| `afmail case set-summary` | Set or replace one case entry summary |
| `afmail case remove` | Remove one message from this case and return it to triage |
| `afmail case move` | Move this case to another group |
| `afmail case rename` | Rename this case's human-readable name without changing its UID |
| `afmail case notes` | Show or edit case notes |
| `afmail case archive` | Archive this active case |
| `afmail case unarchive` | Move this archived case back to an active case group |
| `afmail case trash` | Move every message in this case to local trash and queue Trash moves |
| `afmail case delete` | Delete an empty case |
| `afmail case tag` | Add a case organization tag |
| `afmail case untag` | Remove a case organization tag |
| `afmail case draft` | Create, edit, validate, queue, or remove local case drafts |
| `afmail case merge` | Merge another case into this case |

### afmail case create - Create a new case and return its stable UID/ref

```text
afmail case create [OPTIONS] --name <NAME> --group <GROUP>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | Human-readable case name used in case.md and the directory suffix *(required)* |
| `--group <GROUP>` | Required case category/group, for example support, invoice, 客服, 发票, 合同, or 退款 *(required)* |
| `--message <MESSAGE>` | Optional first message to add to the case |
| `--summary <SUMMARY>` | Short summary for the first message |
| `--reason <REASON>` | Why this case is being created |

### afmail case list - List compact case locators

```text
afmail case list [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--archived` | List archived cases instead of active cases |
| `--all` | List active and archived cases together |

### afmail case show - Show the exact typed case collection document used by Markdown and Review

```text
afmail case show <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |

### afmail case review - Open a review session for this case, including editable notes

```text
afmail case review [OPTIONS] <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--mode <MODE>` | Review presentation mode *(default: `local`)* |

### afmail case suggestion - Set or clear the current typed Agent suggestion for this case

```text
afmail case suggestion <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail case suggestion set` | Replace the current suggestion and clear any human review of the prior round |
| `afmail case suggestion clear` | Clear exactly the current suggestion round |

#### afmail case suggestion set - Replace the current suggestion and clear any human review of the prior round

```text
afmail case suggestion set [OPTIONS] --case-review-base-hash <CASE_REVIEW_BASE_HASH> --reason <REASON> <CASE_REF> <COMMAND>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--case-review-base-hash <CASE_REVIEW_BASE_HASH>` | Current hash returned by `afmail case show` *(required)* |
| `--reason <REASON>` | Why this complete action is being suggested *(required)* |
| `--confidence-percent <CONFIDENCE_PERCENT>` | Optional confidence from 0 through 100 |

| Command | Summary |
| --- | --- |
| `afmail case suggestion set archive` |  |
| `afmail case suggestion set unarchive` |  |
| `afmail case suggestion set move` |  |
| `afmail case suggestion set rename` |  |
| `afmail case suggestion set tag` |  |
| `afmail case suggestion set untag` |  |
| `afmail case suggestion set trash` |  |
| `afmail case suggestion set delete` |  |

##### afmail case suggestion set archive

```text
afmail case suggestion set archive
```

##### afmail case suggestion set unarchive

```text
afmail case suggestion set unarchive --group <GROUP>
```

| Argument | Description |
| --- | --- |
| `--group <GROUP>` | *(required)* |

##### afmail case suggestion set move

```text
afmail case suggestion set move --group <GROUP>
```

| Argument | Description |
| --- | --- |
| `--group <GROUP>` | *(required)* |

##### afmail case suggestion set rename

```text
afmail case suggestion set rename --name <NAME>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | *(required)* |

##### afmail case suggestion set tag

```text
afmail case suggestion set tag <TAG>
```

| Argument | Description |
| --- | --- |
| `TAG` | *(required)* |

##### afmail case suggestion set untag

```text
afmail case suggestion set untag <TAG>
```

| Argument | Description |
| --- | --- |
| `TAG` | *(required)* |

##### afmail case suggestion set trash

```text
afmail case suggestion set trash
```

##### afmail case suggestion set delete

```text
afmail case suggestion set delete [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--allow-notes` |  |

#### afmail case suggestion clear - Clear exactly the current suggestion round

```text
afmail case suggestion clear --agent-suggestion-uid <AGENT_SUGGESTION_UID> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--agent-suggestion-uid <AGENT_SUGGESTION_UID>` | UID returned by the current suggestion set command *(required)* |

### afmail case add - Add a message to this existing case

```text
afmail case add [OPTIONS] <CASE_REF> <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID...` | Message ids to add *(required)* |
| `--summary <SUMMARY>` | Short summary for this message in the case |
| `--reason <REASON>` | Why this message belongs in this case; required by default |

### afmail case set-summary - Set or replace one case entry summary

```text
afmail case set-summary [OPTIONS] --summary <SUMMARY> <CASE_REF> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID` | Message id whose summary should be changed *(required)* |
| `--summary <SUMMARY>` | New summary text *(required)* |
| `--reason <REASON>` | Why this summary is useful; required by default |

### afmail case remove - Remove one message from this case and return it to triage

```text
afmail case remove [OPTIONS] <CASE_REF> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID` | Message id to remove *(required)* |
| `--reason <REASON>` | Why this message does not belong in this case; required by default |

### afmail case move - Move this case to another group

```text
afmail case move [OPTIONS] --group <GROUP> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--group <GROUP>` | Destination group *(required)* |
| `--reason <REASON>` | Why this group better represents the case; required by default |

### afmail case rename - Rename this case's human-readable name without changing its UID

```text
afmail case rename [OPTIONS] --name <NAME> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--name <NAME>` | New human-readable case name *(required)* |
| `--reason <REASON>` | Why this name better represents the case; required by default |

### afmail case notes - Show or edit case notes

```text
afmail case notes <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail case notes show` | Show notes markdown |
| `afmail case notes append` | Append text to notes markdown |
| `afmail case notes replace` | Replace notes markdown with text |

#### afmail case notes show - Show notes markdown

```text
afmail case notes show <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |

#### afmail case notes append - Append text to notes markdown

```text
afmail case notes append --text <TEXT> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to append *(required)* |

#### afmail case notes replace - Replace notes markdown with text

```text
afmail case notes replace --text <TEXT> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to write *(required)* |

### afmail case archive - Archive this active case

```text
afmail case archive [OPTIONS] <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--reason <REASON>` | Why this case is ready to archive; required by default |

### afmail case unarchive - Move this archived case back to an active case group

```text
afmail case unarchive [OPTIONS] --group <GROUP> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--group <GROUP>` | Active case group to unarchive into *(required)* |
| `--reason <REASON>` | Why this case needs active attention again; required by default |

### afmail case trash - Move every message in this case to local trash and queue Trash moves

```text
afmail case trash [OPTIONS] <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--reason <REASON>` | Why this whole case should be discarded; required by default |

### afmail case delete - Delete an empty case

```text
afmail case delete [OPTIONS] <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--allow-notes` | Also delete notes.md when it contains user-authored notes |
| `--reason <REASON>` | Why this case should be deleted; required by default |

### afmail case tag - Add a case organization tag

```text
afmail case tag [OPTIONS] <CASE_REF> <TAG>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `TAG` | Case organization tag *(required)* |
| `--reason <REASON>` | Why this tag is useful; required by default |

### afmail case untag - Remove a case organization tag

```text
afmail case untag [OPTIONS] <CASE_REF> <TAG>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `TAG` | Case organization tag *(required)* |
| `--reason <REASON>` | Why this tag should be removed; required by default |

### afmail case draft - Create, edit, validate, queue, or remove local case drafts

```text
afmail case draft <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail case draft new` | Scaffold a new outbound draft (not a reply) in this case |
| `afmail case draft reply` | Scaffold a reply draft to a message, prefilled and quoting the original |
| `afmail case draft change` | Change an existing editable draft in place |
| `afmail case draft show` | Show an existing draft as a typed render document |
| `afmail case draft review` | Open a review session for this draft |
| `afmail case draft validate` | Validate a draft under the case drafts directory |
| `afmail case draft list` | List local drafts with their current status |
| `afmail case draft upload` | Queue this draft to be uploaded to the remote Drafts mailbox |
| `afmail case draft send` | Queue this draft to be sent and recorded in the case after push succeeds |
| `afmail case draft attach` | Copy or reference a file and add it to a draft's attachments |
| `afmail case draft remove` | Remove a local draft and any queued outbound item for it |

#### afmail case draft new - Scaffold a new outbound draft (not a reply) in this case

```text
afmail case draft new [OPTIONS] --to <TO> --subject <SUBJECT> <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `--to <TO>...` | Recipient address. Repeatable *(required)* |
| `--cc <CC>...` | Cc address. Repeatable |
| `--subject <SUBJECT>` | Draft subject *(required)* |
| `--identity <IDENTITY>` | Workspace identity slug to send as. Defaults to the configured default identity |
| `--body <BODY>` | Draft body text |
| `--body-file <BODY_FILE>` | Path to a file whose contents become the draft body |

#### afmail case draft reply - Scaffold a reply draft to a message, prefilled and quoting the original

```text
afmail case draft reply [OPTIONS] <CASE_REF> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID` | Message id in this case to reply to *(required)* |
| `--identity <IDENTITY>` | Workspace identity slug to send as. Defaults to the configured default identity |
| `--body <BODY>` | Draft body text. Replaces the default reply template when provided |
| `--body-file <BODY_FILE>` | Path to a file whose contents become the draft body |
| `--all` | Reply to all original recipients (To and Cc), not just the sender |

#### afmail case draft change - Change an existing editable draft in place

```text
afmail case draft change [OPTIONS] <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |
| `--subject <SUBJECT>` | Replacement subject |
| `--to <TO>...` | Replacement To list. Repeatable; when provided it replaces all To recipients |
| `--cc <CC>...` | Replacement Cc list. Repeatable; when provided it replaces all Cc recipients |
| `--clear-cc` | Clear all Cc recipients |
| `--identity <IDENTITY>` | Change the workspace identity slug used by this draft |
| `--body <BODY>` | Replacement draft body text |
| `--body-file <BODY_FILE>` | Path to a file whose contents replace the draft body |

#### afmail case draft show - Show an existing draft as a typed render document

```text
afmail case draft show <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |

#### afmail case draft review - Open a review session for this draft

```text
afmail case draft review [OPTIONS] <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |
| `--mode <MODE>` | Review presentation mode *(default: `local`)* |

#### afmail case draft validate - Validate a draft under the case drafts directory

```text
afmail case draft validate <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |

#### afmail case draft list - List local drafts with their current status

```text
afmail case draft list <CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |

#### afmail case draft upload - Queue this draft to be uploaded to the remote Drafts mailbox

```text
afmail case draft upload <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |

#### afmail case draft send - Queue this draft to be sent and recorded in the case after push succeeds

```text
afmail case draft send <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |

#### afmail case draft attach - Copy or reference a file and add it to a draft's attachments

```text
afmail case draft attach <CASE_REF> <DRAFT_NAME> <PATH>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |
| `PATH` | Local file path to attach *(required)* |

#### afmail case draft remove - Remove a local draft and any queued outbound item for it

```text
afmail case draft remove [OPTIONS] <CASE_REF> <DRAFT_NAME>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Case ref: cYYYYMMDDNNN or cYYYYMMDDNNN-any-suffix *(required)* |
| `DRAFT_NAME` | Draft markdown file under the case drafts directory *(required)* |
| `--reason <REASON>` | Why this draft should be removed; required by default |

### afmail case merge - Merge another case into this case

```text
afmail case merge [OPTIONS] <CASE_REF> <OTHER_CASE_REF>
```

| Argument | Description |
| --- | --- |
| `CASE_REF` | Primary case ref *(required)* |
| `OTHER_CASE_REF` | Case ref to merge into the primary case *(required)* |
| `--reason <REASON>` | Why these cases should be merged; required by default |

## afmail contact - Create a contact or operate on an existing contact ref

```text
afmail contact <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail contact create` | Create a new contact record |
| `afmail contact list` | List contacts, optionally filtered by group, tag, org, or role |
| `afmail contact show` | Show the exact typed contact document used by its Markdown template |
| `afmail contact move` | Move a contact to a different group |
| `afmail contact rename` | Rename a contact's display name |
| `afmail contact email` | Add or remove email addresses for a contact |
| `afmail contact phone` | Add or remove phone numbers for a contact |
| `afmail contact tag` | Add a tag to a contact |
| `afmail contact untag` | Remove a tag from a contact |
| `afmail contact notes` | Show or edit contact notes |
| `afmail contact delete` | Delete a contact |
| `afmail contact extract` | Extract stub contacts from message senders not yet in the contact list |

### afmail contact create - Create a new contact record

```text
afmail contact create [OPTIONS] --name <NAME>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | Display name for the contact *(required)* |
| `--group <GROUP>` | Group. Defaults to the configured default contact group |
| `--email <EMAILS>...` | Email address. Repeatable |
| `--phone <PHONES>...` | Phone number. Repeatable |
| `--org <ORG>` | Organization name |
| `--role <ROLE>` | Role or title |
| `--tag <TAGS>...` | Tag. Repeatable |
| `--reason <REASON>` | Why this contact is being created; required by default |

### afmail contact list - List contacts, optionally filtered by group, tag, org, or role

```text
afmail contact list [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--group <GROUP>` | Filter to a specific group |
| `--tag <TAG>` | Filter by tag |
| `--org <ORG>` | Filter by organization |
| `--role <ROLE>` | Filter by role |

### afmail contact show - Show the exact typed contact document used by its Markdown template

```text
afmail contact show <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |

### afmail contact move - Move a contact to a different group

```text
afmail contact move [OPTIONS] --group <GROUP> <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `--group <GROUP>` | Destination group *(required)* |
| `--reason <REASON>` | Why this contact should move groups; required by default |

### afmail contact rename - Rename a contact's display name

```text
afmail contact rename [OPTIONS] --name <NAME> <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `--name <NAME>` | New display name *(required)* |
| `--reason <REASON>` | Why this name better represents the contact; required by default |

### afmail contact email - Add or remove email addresses for a contact

```text
afmail contact email <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail contact email add` | Add an email address to the contact |
| `afmail contact email remove` | Remove an email address from the contact |

#### afmail contact email add - Add an email address to the contact

```text
afmail contact email add [OPTIONS] <CONTACT_REF> <EMAIL>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `EMAIL` | Email address to add *(required)* |
| `--reason <REASON>` | Why this email should be associated with the contact; required by default |

#### afmail contact email remove - Remove an email address from the contact

```text
afmail contact email remove [OPTIONS] <CONTACT_REF> <EMAIL>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `EMAIL` | Email address to remove *(required)* |
| `--reason <REASON>` | Why this email should be removed; required by default |

### afmail contact phone - Add or remove phone numbers for a contact

```text
afmail contact phone <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail contact phone add` | Add a phone number to the contact |
| `afmail contact phone remove` | Remove a phone number from the contact |

#### afmail contact phone add - Add a phone number to the contact

```text
afmail contact phone add [OPTIONS] <CONTACT_REF> <PHONE>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `PHONE` | Phone number to add *(required)* |
| `--reason <REASON>` | Why this phone number should be associated with the contact; required by default |

#### afmail contact phone remove - Remove a phone number from the contact

```text
afmail contact phone remove [OPTIONS] <CONTACT_REF> <PHONE>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `PHONE` | Phone number to remove *(required)* |
| `--reason <REASON>` | Why this phone number should be removed; required by default |

### afmail contact tag - Add a tag to a contact

```text
afmail contact tag [OPTIONS] <CONTACT_REF> <TAG>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `TAG` | Tag to add *(required)* |
| `--reason <REASON>` | Why this tag is useful; required by default |

### afmail contact untag - Remove a tag from a contact

```text
afmail contact untag [OPTIONS] <CONTACT_REF> <TAG>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `TAG` | Tag to remove *(required)* |
| `--reason <REASON>` | Why this tag should be removed; required by default |

### afmail contact notes - Show or edit contact notes

```text
afmail contact notes <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail contact notes show` | Show the typed contact document with raw notes Markdown |
| `afmail contact notes append` | Append text to notes markdown |
| `afmail contact notes replace` | Replace notes markdown with text |

#### afmail contact notes show - Show the typed contact document with raw notes Markdown

```text
afmail contact notes show <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |

#### afmail contact notes append - Append text to notes markdown

```text
afmail contact notes append --text <TEXT> <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to append *(required)* |

#### afmail contact notes replace - Replace notes markdown with text

```text
afmail contact notes replace --text <TEXT> <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to write *(required)* |

### afmail contact delete - Delete a contact

```text
afmail contact delete [OPTIONS] <CONTACT_REF>
```

| Argument | Description |
| --- | --- |
| `CONTACT_REF` | Contact ref: pYYYYMMDDNNN or pYYYYMMDDNNN-any-suffix *(required)* |
| `--allow-notes` | Also delete the contact body when it contains user-authored notes |
| `--reason <REASON>` | Why this contact should be deleted; required by default |

### afmail contact extract - Extract stub contacts from message senders not yet in the contact list

```text
afmail contact extract [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--from-triage` | Extract from triage messages only |
| `--from-case <CASE_REF>` | Extract from a specific case's messages |
| `--all` | Extract from all triage and case messages |
| `--group <GROUP>` | Destination group for created contacts. Defaults to contact.default_group |

## afmail notification - Inspect and manage notification collections

```text
afmail notification <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail notification create` | Create a notification collection and optionally file one message |
| `afmail notification add` | File an existing message into this notification collection |
| `afmail notification list` | List compact notification collections |
| `afmail notification show` | Show the exact typed notification document used by Markdown and Review |
| `afmail notification review` | Open a review session for this notification and its persisted suggestion |
| `afmail notification suggestion` | Set or clear the current typed Agent suggestion for this collection |
| `afmail notification remove` | Remove a notification message from this collection and return it to triage |
| `afmail notification move` | Move a notification message to another notification collection |
| `afmail notification rename` | Rename this notification collection's human-readable name without changing its UID |
| `afmail notification delete` | Delete an empty notification collection |
| `afmail notification set-summary` | Set or replace one notification entry summary |
| `afmail notification notes` | Show or edit notification collection notes |

### afmail notification create - Create a notification collection and optionally file one message

```text
afmail notification create [OPTIONS] --name <NAME>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | Human-readable notification name used in metadata and the directory suffix *(required)* |
| `--message <MESSAGE>` | Optional message to immediately file into the new notification collection |
| `--summary <SUMMARY>` | Required when --message is supplied |
| `--reason <REASON>` | Why this notification collection is being created |

### afmail notification add - File an existing message into this notification collection

```text
afmail notification add [OPTIONS] --summary <SUMMARY> <NOTIFICATION_REF> <MESSAGE_ID>...
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID...` | Message ids to file *(required)* |
| `--summary <SUMMARY>` | Human/agent-authored summary for this notification entry *(required)* |
| `--reason <REASON>` | Why this message belongs in this notification; required by default |

### afmail notification list - List compact notification collections

```text
afmail notification list
```

### afmail notification show - Show the exact typed notification document used by Markdown and Review

```text
afmail notification show <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |

### afmail notification review - Open a review session for this notification and its persisted suggestion

```text
afmail notification review [OPTIONS] <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--mode <MODE>` | Review presentation mode *(default: `local`)* |

### afmail notification suggestion - Set or clear the current typed Agent suggestion for this collection

```text
afmail notification suggestion <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail notification suggestion set` | Replace the current suggestion and clear any human review of the prior round |
| `afmail notification suggestion clear` | Clear exactly the current suggestion round |

#### afmail notification suggestion set - Replace the current suggestion and clear any human review of the prior round

```text
afmail notification suggestion set [OPTIONS] --notification-review-base-hash <NOTIFICATION_REVIEW_BASE_HASH> --reason <REASON> <NOTIFICATION_REF> <COMMAND>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--notification-review-base-hash <NOTIFICATION_REVIEW_BASE_HASH>` | Current hash returned by `afmail notification show` *(required)* |
| `--reason <REASON>` | Why this complete action is being suggested *(required)* |
| `--confidence-percent <CONFIDENCE_PERCENT>` | Optional confidence from 0 through 100 |

| Command | Summary |
| --- | --- |
| `afmail notification suggestion set rename` |  |
| `afmail notification suggestion set delete` |  |

##### afmail notification suggestion set rename

```text
afmail notification suggestion set rename --name <NAME>
```

| Argument | Description |
| --- | --- |
| `--name <NAME>` | *(required)* |

##### afmail notification suggestion set delete

```text
afmail notification suggestion set delete [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--allow-notes` |  |

#### afmail notification suggestion clear - Clear exactly the current suggestion round

```text
afmail notification suggestion clear --agent-suggestion-uid <AGENT_SUGGESTION_UID> <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--agent-suggestion-uid <AGENT_SUGGESTION_UID>` | UID returned by the current suggestion set command *(required)* |

### afmail notification remove - Remove a notification message from this collection and return it to triage

```text
afmail notification remove [OPTIONS] <NOTIFICATION_REF> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID` | *(required)* |
| `--reason <REASON>` | Why this message does not belong in this notification; required by default |

### afmail notification move - Move a notification message to another notification collection

```text
afmail notification move [OPTIONS] <NOTIFICATION_REF> <MESSAGE_ID> <NEW_NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Source notification collection ref *(required)* |
| `MESSAGE_ID` | *(required)* |
| `NEW_NOTIFICATION_REF` | Destination notification collection ref *(required)* |
| `--reason <REASON>` | Why this collection is better; required by default |

### afmail notification rename - Rename this notification collection's human-readable name without changing its UID

```text
afmail notification rename [OPTIONS] --name <NAME> <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--name <NAME>` | New human-readable notification name *(required)* |
| `--reason <REASON>` | Why this name better represents the collection; required by default |

### afmail notification delete - Delete an empty notification collection

```text
afmail notification delete [OPTIONS] <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--allow-notes` | Also delete notes.md when it contains user-authored notes |
| `--reason <REASON>` | Why this notification collection should be deleted; required by default |

### afmail notification set-summary - Set or replace one notification entry summary

```text
afmail notification set-summary [OPTIONS] --summary <SUMMARY> <NOTIFICATION_REF> <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `MESSAGE_ID` | *(required)* |
| `--summary <SUMMARY>` | *(required)* |
| `--reason <REASON>` | Why this summary is useful; required by default |

### afmail notification notes - Show or edit notification collection notes

```text
afmail notification notes <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail notification notes show` | Show notes markdown |
| `afmail notification notes append` | Append text to notes markdown |
| `afmail notification notes replace` | Replace notes markdown with text |

#### afmail notification notes show - Show notes markdown

```text
afmail notification notes show <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |

#### afmail notification notes append - Append text to notes markdown

```text
afmail notification notes append --text <TEXT> <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to append *(required)* |

#### afmail notification notes replace - Replace notes markdown with text

```text
afmail notification notes replace --text <TEXT> <NOTIFICATION_REF>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_REF` | Notification ref: nYYYYMMDDNNN or nYYYYMMDDNNN-any-suffix *(required)* |
| `--text <TEXT>` | Markdown text to write *(required)* |

## afmail render - Rebuild generated read views from local workspace state

```text
afmail render <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail render refresh` | Rebuild generated case and notification read views |
| `afmail render templates` | Export built-in language templates, keeping existing files unless forced |

### afmail render refresh - Rebuild generated case and notification read views

```text
afmail render refresh
```

### afmail render templates - Export built-in language templates, keeping existing files unless forced

```text
afmail render templates [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--force` | Overwrite existing workspace templates with built-in defaults |

## afmail log - Inspect the workspace audit log

```text
afmail log <COMMAND>
```

| Command | Summary |
| --- | --- |
| `afmail log list` | List recent audit events |
| `afmail log tail` | Tail recent audit events as JSON data |
| `afmail log message` | List events for one message id |
| `afmail log case` | List events for one case id |
| `afmail log notification` | List events for one notification collection |

### afmail log list - List recent audit events

```text
afmail log list [OPTIONS]
```

| Argument | Description |
| --- | --- |
| `--limit <LIMIT>` | Maximum number of events to return *(default: `50`)* |

### afmail log tail - Tail recent audit events as JSON data

```text
afmail log tail
```

### afmail log message - List events for one message id

```text
afmail log message <MESSAGE_ID>
```

| Argument | Description |
| --- | --- |
| `MESSAGE_ID` | *(required)* |

### afmail log case - List events for one case id

```text
afmail log case <CASE_UID>
```

| Argument | Description |
| --- | --- |
| `CASE_UID` | *(required)* |

### afmail log notification - List events for one notification collection

```text
afmail log notification <NOTIFICATION_UID>
```

| Argument | Description |
| --- | --- |
| `NOTIFICATION_UID` | *(required)* |
