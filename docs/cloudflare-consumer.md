# Event-driven mail and agent work

The Cloudflare consumer keeps idle mailbox checks out of the model context.
`afmail cloudflare watch` is an ordinary program: it polls Cloudflare Queues,
imports mail into the local workspace, and invokes a configured command only
when durable new-message work exists. No model, model credential, or agent SDK
is needed to wait for mail.

```text
Email Worker -> immutable raw EML in R2
                         |
                  R2 create notification
                         |
                  Cloudflare Queue
                         |
                  VPS: afmail cloudflare watch
                         |
           local import + durable notification task
                         |
           configured command -> agent scheduler
```

## Setup

First complete the [Cloudflare mailbox setup](setup.md#cloudflare-email-routing--r2--email-service).
Keep the existing spool control endpoint configured: Queue acknowledgement does
not replace R2 cleanup or the Durable Object quota ledger.

Create a queue and enable its HTTP pull consumer. In the R2 bucket settings,
add an `object-create` notification for the pending prefix (`inbox/` by default)
with this queue as its destination. Exclude the imported/archive prefix: copying
an imported object must not create new mail work. See Cloudflare's
[R2 event notifications](https://developers.cloudflare.com/r2/buckets/event-notifications/)
and [HTTP pull consumers](https://developers.cloudflare.com/queues/configuration/pull-consumers/).
The queue token needs Queues read and write permissions; acknowledgement changes
queue state. It is separate from the R2 S3 credentials and spool control token.

Configure the workspace using secret sources rather than live tokens in argv:

```bash
afmail config set cloudflare.account_id YOUR_ACCOUNT_ID
afmail config set cloudflare.queue.queue_id YOUR_QUEUE_ID
afmail config set cloudflare.queue.api_token_secret 'file:/etc/afmail/secrets.json#queue_token_secret'
afmail config set cloudflare.consumer.command '["/usr/local/bin/enqueue-mail-agent"]'
afmail cloudflare consume --reconcile
afmail cloudflare watch
```

`cloudflare consume --reconcile` runs one finite tick with the same effects as
a `watch` tick: it acknowledges Queue events, deletes or archives imported R2 objects, and
may launch the configured command. It is not a read-only inspection.
`cloudflare watch` stays in the foreground; run it under a service manager on the VPS. An example systemd unit:

```ini
[Unit]
Description=Receive mail and enqueue agent work
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=mailagent
WorkingDirectory=/srv/private-mail
ExecStart=/usr/local/bin/afmail cloudflare watch
Restart=on-failure
RestartSec=10
KillMode=control-group

[Install]
WantedBy=multi-user.target
```

The service account needs access to the private workspace, credentials, and
configured handler. Installing a unit and provisioning Cloudflare resources are
operator deployment steps; running the local tests does neither.

## Configuration

| Key | Default | Meaning |
|---|---|---|
| `cloudflare.queue.queue_id` | unset | Queue with HTTP pull enabled |
| `cloudflare.queue.api_token_secret` | unset | Queues read/write token or secret source |
| `cloudflare.queue.endpoint_url` | `https://api.cloudflare.com/client/v4` | REST API base; HTTPS, with HTTP allowed only on loopback |
| `cloudflare.queue.batch_size` | `20` | Events per pull and maximum messages per agent task, from 1 to 100 |
| `cloudflare.queue.visibility_timeout_s` | `300` | Queue lease duration, from 1 to 43,200 seconds |
| `cloudflare.consumer.command` | `[]` | Trusted executable and argument array; an empty array retains tasks without launching a handler |
| `cloudflare.consumer.command_timeout_s` | `300` | Maximum handler execution time |
| `cloudflare.consumer.retry_delay_s` | `60` | Delay before retrying a failed local task or event |
| `cloudflare.consumer.poll_interval_s` | `5` | Pause between watch ticks |
| `cloudflare.consumer.reconcile_interval_s` | `300` | Interval between R2 scans; watch also scans on startup |

HTTP pull uses short polling, not a held-open connection. Latency depends on the
poll interval and ongoing work. A handler runs synchronously; use a quick durable
scheduler handoff to keep receiving mail while the agent works. No subsecond or
exactly-once delivery guarantee is made.

`watch` reloads configuration between ticks, emits `cloudflare_consumer_ready`
once and `cloudflare_consumer_tick` for work or warnings, and stays quiet on idle
ticks. These are structured progress events on its ordered output stream. Stop
it with the service manager or Ctrl-C. The finite `consume` command accepts
`--reconcile` to scan immediately; without it, it consumes events and retries
local work without listing the R2 bucket.

## Handler contract

The command is executed directly, without a shell, in the workspace root.
No subject, sender, body, recipient, object key, or attachment name is inserted
into argv. Stdin and child stdout/stderr are disconnected from the afmail
protocol stream; the handler should manage its own logs without logging secrets.
Two environment variables supply trusted task locators:

- `AFMAIL_TASK_PATH`: absolute path to a persisted JSON batch.
- `AFMAIL_TASK_ID`: stable idempotency key for that batch, unchanged on retries.

A task has this shape (the example ID and message ID are illustrative):

```json
{
  "schema_name": "mail_received_task",
  "schema_version": 1,
  "task_id": "mail_0123456789abcdef",
  "messages": [
    {
      "message_id": "message_20261024_0123456789abcdef",
      "raw_path": ".afmail/messages/message_20261024_0123456789abcdef.eml"
    }
  ]
}
```

The agent can read the referenced raw mail or use `afmail message show MESSAGE_ID`.
The workspace lock is released while the handler runs so ordinary afmail
commands remain available. Several new messages received in one tick can share
a task. Repeated deliveries with identical raw bytes merge into the existing
message, including envelope recipients, and do not create another task.

Exit zero only after the scheduler durably accepts the task (or after inline
processing completes). The scheduler must deduplicate by `AFMAIL_TASK_ID`:
a crash after successful handoff but before recording completion can invoke the
handler again. The trigger is at-least-once. A nonzero exit, start failure, or
timeout leaves the same active batch pending. Fix the handler and resume the
consumer; the retry uses the same task ID and inputs. An active failed batch is
retried before creating another batch. Remaining new mail stays pending locally.
Scheduling acceptance and agent processing are separate: after durable scheduler
acceptance, the scheduler owns agent execution retries.

A handler's ordinary descendants are stopped when it exits, times out, or is
interrupted, before completion or retry is recorded. Unix handlers run in their own process group; Windows
handlers are assigned to a Job Object before their first instruction runs.
This bounds handler execution, not work already accepted by an independent
scheduler. A handler must finish inline work or durably hand off to an
independently managed scheduler before exiting; background children in its
owned tree do not outlive it. Handlers should not deliberately escape that tree.

On Unix, the consumer CLI handles SIGINT, SIGTERM, SIGHUP, and SIGQUIT while
watching or consuming. It reclaims an active handler tree and exits with the
structured `consumer_interrupted` error (exit 1), leaving unfinished tasks
pending. Idle watches also stop promptly during the polling interval. Signal
handlers are CLI-owned; library dispatch does not install process-wide handlers.

`afmail status` includes a read-only `cloudflare_consumer` snapshot: pending,
preparing, and not-yet-recorded completed message counts, receipt and pending-cleanup counts,
and the active batch's task id, message ids, attempt count, last attempt time,
last error code, exit code when available, and next retry time. Older batches
default to zero known attempts and absent error details. Completed markers are
recognized even before the journal has been folded; inspection never folds
or rewrites it on disk. An unreadable journal appears as an explicit nested
error while the rest of status remains available.

Completion markers must be regular JSON files of at most 4096 bytes with
`schema_name: "mail_received_task_completed"`, `schema_version: 1`, and the
active batch's exact `task_id`. An empty, malformed, oversized, or mismatched
marker reports `consumer_completion_invalid`; its mere presence never settles
tasks. Active batches must reference distinct pending tasks. Status and doctor
report these problems without rewriting the marker or the journal, and
consumption refuses to dispatch or discard the affected active batch. Restore
the matching evidence from a trusted backup before resuming.

`afmail doctor` checks the consumer journal even in a build without Cloudflare
transport support. It reports corrupt state, invalid active-task references,
missing pending-message evidence, and a failed batch blocking later work.
These issues are not automatically repaired by discarding notification state.
Fix the handler and resume normal consumption; failed batches retain their
task ids and still run before later batches. Consumer journal, batch files,
and completion markers are included in `status.storage.queue_bytes`.

Treat all mail content as untrusted data. A trigger authorizes the configured
handler, not commands embedded in a message. Existing push/send approval
requirements remain in force. This consumer does not automatically push mail.

## Persistence and recovery

The order is:

1. Persist a `preparing` notification intent for previously unseen raw bytes.
2. Import raw mail and its canonical delivery metadata; make the task `pending`
   and persist the imported-object receipt.
3. Delete/archive the R2 spool object through the existing control interface
   when managed; persist cleanup completion.
4. Acknowledge the Queue event. If acknowledgement fails, a repeated event can
   be settled from its receipt without scheduling the agent again.
5. Persist a stable batch and run the handler. Record task completion only after
   the handler exits zero.

A crash between import and task readiness leaves the intent in place; replay
merges the imported bytes and makes that intent ready. Failed remote cleanup
keeps its event retryable while the local task can already be dispatched.
Receipts repair cleanup after an ambiguous response. A missing R2 object with no
local receipt is an error and is never acknowledged as successful import.

State lives in `.afmail/cloudflare-consumer/journal.json`, with task input files
in `.afmail/cloudflare-consumer/batches/`. Preserve these as private canonical
workspace state; they are not disposable render caches. The journal stays
bounded: completed tasks are dropped when their completion is recorded, since the
workspace index already deduplicates their mail, and a receipt whose cleanup
finished is dropped once it is older than 15 days, past Cloudflare's 14-day cap
on Queue message retention, so no event can still ask about its object. Restore the journal and mail evidence together. Do not delete or
hand-edit the journal to retry work. A corrupt/unsupported journal is reported,
not silently reset. The process lock is machine-local and is not a distributed
claim across cloned workspaces.

R2 remains the recovery source until cleanup. `watch` periodically scans pending
objects even when the Queue API is temporarily unavailable. This recovers mail
whose event expired, reached a dead-letter queue, or was never configured.
Scanning retains the ordinary pull count/byte limits. An object that cannot be
imported — unparseable MIME, invalid spool metadata, or a key that vanished
after listing — is reported as a warning with its `key` and left in R2, and the
scan continues with the next object; authentication, network, and local storage
failures still stop the tick. A task whose message was purged before dispatch
is dropped with a `consumer_message_missing` warning carrying its `message_id`,
so it cannot hold a batch open. Inspect `warnings` and
`pending_message_count` in finite results and watch events; a successful tick
with warnings does not mean every event or handler succeeded. Configure and
monitor a dead-letter queue for repeatedly failing events, and retain pending R2
objects long enough for recovery.

Use the generated Worker or another spool writer with unique, immutable object
keys. Reusing an already consumed key violates the receipt identity contract.
One consumer tick per workspace is protected through dispatch by a process lock;
the normal workspace lock protects import and journal mutations. Run one watch
service for a workspace. One Queue and R2 spool prefix serve exactly one
workspace: multiple hosts or workspaces pulling one Queue compete for messages,
and each pull deletes or archives the objects it imports, so none receives a
complete mailbox copy. This feature does not add
cross-host workspace coordination or a shared helpdesk.

## Validation

Run the repository entrypoints:

```bash
bash scripts/test.sh consumer
bash scripts/test.sh pull
bash scripts/test.sh static
bash scripts/test.sh check-target x86_64-pc-windows-gnu
```

The consumer gate uses local HTTP Queue/R2 fixtures and a real subprocess handler.
It checks idle behavior, direct object import, batched duplicate suppression,
handler access to afmail while dispatch runs, stable retries, remote cleanup and
Queue acknowledgement failures, malformed/missing events, R2 recovery during a
Queue outage, crash recovery, and credential redaction. No Cloudflare account or
model is contacted. Provisioned R2 notifications and live Queue delivery still
need deployment verification against the operator's account.
