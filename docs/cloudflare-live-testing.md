# Cloudflare live tests

The opt-in entrypoint is `scripts/test-cloudflare-live.py`. It uses the real
`cf` CLI, an Email Routing zone, private Standard R2 storage, and a temporary
sending subdomain. It sends synthetic messages only. Ordinary test gates never
provision cloud resources or send real mail.

Use a Cloudflare account with R2 and Email Sending enabled, a zone already
configured for Email Routing, Node.js 24+, Python 3, and the authenticated `cf`
CLI. Build and validate the default-feature binary with `scripts/test.sh all`
first. The caller needs permission to create user API tokens; account-owned
token administration is not required. The script creates a bucket-scoped R2
token and an account-scoped Email Sending token, both expiring in two hours.

Choose a private directory outside any Git checkout. Keep the same directory
for every stage; `manifest.json` records the exact resources owned by the run.
The directory is mode 0700 and its credential file is mode 0600. Credentials
are never passed in argv, printed, or stored inside the mailbox workspace.

```bash
python3 scripts/test-cloudflare-live.py prepare-storage \
  --state-dir /tmp/mail-live-test --account-id '<ACCOUNT_ID>' --domain example.com
python3 scripts/test-cloudflare-live.py prepare-sender --state-dir /tmp/mail-live-test
python3 scripts/test-cloudflare-live.py deliver --state-dir /tmp/mail-live-test
python3 scripts/test-cloudflare-live.py consume --state-dir /tmp/mail-live-test
python3 scripts/test-cloudflare-live.py verify-duplicate-hint --state-dir /tmp/mail-live-test
python3 scripts/test-cloudflare-live.py smtp-preview --state-dir /tmp/mail-live-test
```

`prepare-storage` provisions a private spool-control token, configures quota-aware
acknowledgements, and scaffolds and deploys the shipped Worker through `cf`, with
two exact-match receiving addresses. It does not change existing routes or the
catch-all. `prepare-sender` enables only the run's new sending subdomain and its
authentication DNS records. Initial R2 S3 endpoint activation can lag behind
bucket creation: inspect the failure and retry `prepare-storage` after the
endpoint becomes ready. Do not weaken TLS validation.

Allow newly created receiving rules to propagate before `deliver`; their
presence in the management API does not prove that delivery servers recognize
them yet. A live run observed `routing_unknown_address` delivery failures while
both enabled rules were already listed. Inspect the Email Service activity log
after an observation timeout, and retain the original submission evidence rather
than clearing the attempted-send marker or blindly resending.
An isolated follow-up run allowed about 90 seconds after receiving setup before
submission and completed both deliveries, archive acknowledgement, exact-byte
replay deduplication, and hidden-recipient identity/Bcc checks. This delay is a
propagation allowance, not a delivery-readiness guarantee.

`deliver` submits one raw message with two SMTP-envelope recipients. The second
recipient is absent from the visible headers, exercising Bcc delivery metadata.
It records the submission attempt before sending and refuses to send it again.
If observation times out, run `observe` to check the same submission. A network
error does not establish that no email was sent.

`consume` verifies that a dry run leaves objects untouched, imports original
bytes, retains envelope recipients, and acknowledges deliveries with an R2
copy followed by deletion. It then checks an empty pull and replays identical
bytes, which must merge without creating another local message. The replay
uses deletion acknowledgement. Results and raw synthetic EML evidence remain
in the private run directory.

`verify-duplicate-hint` imports the archived deliveries into a second clean
workspace and checks `duplicate_candidate` warnings. It consumes those archive
objects, matches the hidden recipient to a configured identity, and verifies
the shared `possible_bcc` analysis. This stage expects distinct raw bytes carrying the same Message-ID;
inspect a changed provider behavior rather than forcing this assertion to pass.

`smtp-preview` authenticates to `smtp.mx.cloudflare.net:465`, prepares one
SMTP-only draft, and outputs the exact push plan. Review that plan before
confirming it:

```bash
python3 scripts/test-cloudflare-live.py smtp-send --state-dir /tmp/mail-live-test \
  --confirm-plan-id '<EXACT_REVIEWED_PLAN_ID>'
```

The send stage refuses a changed plan or a second submission attempt. It checks
actual delivery through the Worker, imports that delivery, verifies an empty
push queue, and checks the local sent record. Use `smtp-observe` after an
observation timeout; never blindly repeat a send whose result is uncertain.

To exercise afmail's Cloudflare REST sender, use the same isolated run and token:

```bash
python3 scripts/test-cloudflare-live.py api-preview --state-dir /tmp/mail-live-test
python3 scripts/test-cloudflare-live.py api-send --state-dir /tmp/mail-live-test \
  --confirm-plan-id '<EXACT_REVIEWED_PLAN_ID>'
```

Finish and observe each transport before preparing the other. `api-preview`
selects exactly one `cloudflare_send` step and clears the SMTP host. Its
`remote test` validates local sending configuration without proving sending
permission. `api-send` additionally requires one provider receipt with no
rejected recipients. It checks actual Worker receipt, import acknowledgement,
the local sent record, and an empty push queue. Use `api-observe` after an
observation timeout; it never submits another message. These stages are opt-in
and are not run by ordinary local gates.

After verification, remove the run's routes, Worker, sending subdomain and its
DNS, bucket and remaining synthetic objects, and revoke both tokens:

```bash
python3 scripts/test-cloudflare-live.py cleanup --state-dir /tmp/mail-live-test
```

Cleanup preserves local evidence and deletes its private credential file.
It also removes a generated DMARC record if the sending API leaves that record
behind, restricted to the exact DNS names captured during this run's setup.
If the bootstrap credential can create scoped tokens but cannot revoke them,
cleanup attempts self-revocation without adding permissions. A second 403 is
reported as `token_revocation_denied`, and the recorded two-hour expiration is
retained in `deferred_token_revocations`. This is not a successful revocation.
Cloud resources and local credentials are still removed; the short-lived tokens
expire at their original timestamps. `verify-cleanup` rechecks resource removal
without requiring the deleted credentials.
`cleanup` refuses while either sending test is pending unless `--discard-unsent`
explicitly abandons that test. `inspect` provides a read-only manifest and inbox listing.

## Observed delivery behavior

A live same-zone, two-recipient test on 2026-10-21 produced two distinct raw
messages. Their Message-ID and body matched, but Received, Received-SPF,
Authentication-Results, ARC-Authentication-Results, and ARC-Seal differed.
Consequently, whole-message content hashes cannot collapse those deliveries.

The importer preserves both originals and emits only a weak
`duplicate_candidate` warning with evidence
`same_rfc822_message_id_distinct_raw`. This is not proof that messages are
interchangeable. It never merges, drops, or rewrites messages based on headers.
An exact-byte replay still merges and acknowledges successfully.

The same live run verified SMTP authentication with implicit TLS on port 465,
then sent a confirmed plan containing exactly one `smtp_send` step. Actual
delivery through the receiving Worker, unchanged subject/body, local sent EML,
the case's sent record, an empty push queue, and receipt import all passed.
Temporary routing rules, Worker, sending subdomain and DNS, and the R2 bucket
were removed and their absence rechecked. Token management returned 403 for
both the bootstrap credential and self-revocation; the private credential file
was deleted, with the original short expirations recorded explicitly.

A subsequent live run also verified the REST sender against a deployed
quota-enabled Worker. Its confirmed plan contained exactly one `cloudflare_send`
step. The provider receipt reported the test recipient as delivered, with no
permanent bounces or suppressed recipients. Actual Worker receipt, unchanged
subject/body, acknowledged import, local sent evidence, and an empty push queue
all passed. The run's Durable Object class was retired during cleanup, and the
absence of its Worker, routes, sending DNS, and R2 bucket was rechecked.

The Worker includes a SQLite Durable Object quota ledger. The run requires
Durable Objects access as well as Worker/R2 permissions. The control URL is read
from deployment output; use `--spool-control-url https://WORKER.SUBDOMAIN.workers.dev`
on `prepare-storage` if your deployment hides that URL. Temporary secret JSON is
removed after provisioning, and the token stays in the private credentials file
until cleanup. No live resources are created by `scripts/test.sh worker`.

Cleanup removes routes, deploys a `state: "deleted"` tombstone for the owned quota
class, and only then deletes the Worker. This retires its Durable Object
namespace and storage; script deletion alone is not used as proof of cleanup.
See [Cloudflare class deletion](https://developers.cloudflare.com/durable-objects/reference/durable-objects-migrations/#delete-a-durable-object-class).
