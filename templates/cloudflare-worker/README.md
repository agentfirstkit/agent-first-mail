# Email Routing → R2 → afmail

This Worker stores the exact `message.raw` MIME bytes in `MAIL_SPOOL`, under
`inbox/`. It records the SMTP envelope sender, recipient, and receipt timestamp
as object metadata. Storage failures propagate; optional forwarding failures
leave the stored message available for import.

Use Node.js 24 or newer. Both CLI configurations use `__WORKER_NAME__` and the
`__BUCKET_NAME__` bucket. Change both files together if you rename either resource.
Before deploying with cf, replace `support@example.com` in
`cloudflare.config.ts` with your receiving addresses or a `*@domain` catch-all.
The Worker writes `inbox/`; the prefix is hard-coded in `src/quota.ts`. Keep
afmail's `cloudflare.r2.prefix` aligned if you change it there.

Enable Email Routing for a domain in the Cloudflare dashboard, then choose one
CLI. `cf` has its own login and does not reuse Wrangler credentials.

| Step | Wrangler | cf |
|---|---|---|
| Install locked tools | `npm ci` | `npm ci` |
| Login | `npx wrangler login` | `npx cf auth login` |
| Create bucket | `npx wrangler r2 bucket create __BUCKET_NAME__` | `npx cf r2 buckets create --name __BUCKET_NAME__` |
| Verify build | `npm run dry-run:wrangler` | `npm run dry-run:cf` |
| Deploy | `npx wrangler deploy` | `npx cf deploy` |
| Route mail | Dashboard → Email Routing → Routing Rules → Send to a Worker | `triggers.email({ addresses: [...] })` manages the configured addresses |

For an explicit rule outside cf-managed triggers, the API operation is
`POST /zones/{zone_id}/email/routing/rules`. Its generated CLI command is:

```bash
npx cf email-routing rules create --zone '<ZONE_ID>' --name __WORKER_NAME__ --enabled \
  --actions '[{"type":"worker","value":["__WORKER_NAME__"]}]' \
  --matchers '[{"type":"literal","field":"to","value":"<RECIPIENT_ADDRESS>"}]'
```

Catch-all rules use `cf email-routing rules catch-all update`, corresponding to
`PUT /zones/{zone_id}/email/routing/rules/catch_all`, with `[{"type":"all"}]`
as matchers. Do not create the same rule through both cf triggers and the API.
For future CLI changes, use anonymous task descriptions with `cf cli search`,
and inspect arguments with `cf schema r2 buckets create` or
`cf schema email-routing rules create`.

Optional forwarding uses `FORWARD_TO`; add it as a Worker variable in the chosen
configuration. Forward destinations must meet Cloudflare's verification rules.
The R2 binding grants storage access. Import acknowledgements additionally require
a private control token (at least 32 printable characters). Save the same token in
your local secret file and provision it with `npx wrangler secret put
SPOOL_CONTROL_TOKEN_SECRET`, or `cf workers secrets bulk --worker
__WORKER_NAME__ --secrets @worker-secrets.secret.json`. The private JSON file for
cf contains `{"SPOOL_CONTROL_TOKEN_SECRET":{"type":"secret_text","text":"YOUR_TOKEN"}}`;
remove it after provisioning. Never commit tokens.

Create an R2 Object Read & Write token scoped to this bucket, and save its secret
outside the afmail workspace. Configure afmail using the Access Key ID and
Secret Access Key produced for the S3 API:

```bash
afmail config set cloudflare.account_id '<ACCOUNT_ID>'
afmail config set cloudflare.r2.bucket __BUCKET_NAME__
afmail config set cloudflare.r2.access_key_id '<ACCESS_KEY_ID>'
afmail config set cloudflare.r2.secret_access_key_secret "file:$HOME/.config/afmail/secrets.json#r2_secret_access_key"
afmail config set cloudflare.spool.control_url "https://YOUR_WORKER.YOUR_SUBDOMAIN.workers.dev"
afmail config set cloudflare.spool.token_secret "file:$HOME/.config/afmail/secrets.json#spool_control_token_secret"
afmail remote test
afmail pull --cloudflare
```

Import acknowledges successfully stored or merged objects by deleting them.
Set `cloudflare.r2.imported_prefix` to `imported/` to copy them there before
removing the inbox object. The import and archive prefixes must not overlap.
Interrupted imports and cleanup failures are recovered by rerunning the pull.
Keep this bucket private, dedicated to this Worker, and restrict writes to this
coordinator. New objects carry `quota_managed` metadata: afmail refuses direct S3
cleanup without control configuration, preserving imported local evidence for retry.

Email Routing accepts inbound messages up to 25 MiB. Cloudflare Email Sending
is separate: configure `cloudflare_send` for its REST API, or `smtp_send` for
SMTP, in afmail. This Worker never sends drafted replies. See afmail's
`docs/setup.md` for sending configuration.

Run `npm run check` and `npm test` for local validation. Tests execute in the
Workers runtime against a local R2 binding, without cloud credentials. Both
deployment dry-runs build locally and do not deploy resources. Real delivery
must additionally verify recipient metadata and compare raw hashes for a message
sent to two addresses in the same zone; content deduplication merges only exact
bytes, never just matching Message-ID headers.
Real two-address deliveries can differ in Received and authentication headers.
The importer keeps those originals separately and emits a weak
`duplicate_candidate` warning when their Message-ID matches.

## Admission and storage limits

A single SQLite Durable Object named `spool` serializes reservations for this
bucket. R2 stores content; D1 is not required. Both CLI configurations include the
binding and migration/export. For an existing deployment, retain its migration
history and add the new class rather than replacing old migrations.

| Worker variable | Default | Meaning |
|---|---:|---|
| `MAX_MESSAGE_BYTES` | 10485760 | Maximum MIME bytes; declared and actual size checked, maximum 25 MiB |
| `MAX_SPOOL_BYTES` | 1073741824 | Total reserved/stored content bytes in the bucket |
| `MAX_SPOOL_MESSAGES` | 10000 | Total reserved/stored objects, including archive copies |
| `RATE_WINDOW_S` | 60 | Fixed admission window in seconds |
| `MAX_MESSAGES_PER_WINDOW` | 100 | New writes per window; retries consume admissions too |
| `ARCHIVE_PREFIX` | `imported/` | Permitted archive destination; align with afmail's imported prefix |
| `ALLOWED_RECIPIENTS` | empty | Comma-separated envelope recipient allowlist |
| `ALLOWED_SENDER_DOMAINS` | empty | Comma-separated independently authenticated aligned domains |
| `REQUIRE_DMARC_PASS` | false | Require independent aligned DMARC pass |

Set variables in both configurations. Invalid limits fail closed. Admission
reserves bytes and object count before R2 writes. Archiving reserves an extra
copy before deleting the original, so leave at least one message of headroom.
Archives retain capacity until removed through the authenticated control API
(`POST /v1/ack` with its archive key and no archive destination). Total bytes
counts content, not R2 metadata, Durable Object storage, requests, or billing.
Fixed windows can admit twice the configured count across a window boundary.

`GET /v1/quota` accepts `Authorization: Bearer TOKEN` and returns reserved/stored
usage and configured limits. Only this endpoint and `POST /v1/ack` are public;
inbound writes are available only to the email handler. Acknowledgements use
the same token, account for archive copies, and are idempotent after confirmed
completion; a retried acknowledgement rewrites an archive copy whose earlier
write outcome was unknown, never overwriting one that landed. Existing bucket
objects are adopted before admission opens. An alarm reconciles confirmed
objects deleted externally and settles writes whose outcome was unknown by
observation, never by age. A visible object keeps its charge at its actual
size. Otherwise, once this instance no longer awaits the write, the alarm
creates an empty fence at the key with the same conditional create (`onlyIf`
`etagDoesNotMatch: "*"`) every spool write uses. R2 evaluates conditional writes
atomically and is strongly consistent, so a refused fence means the original
landed (charge kept) and an accepted one means it had not committed; the fence
is then deleted and the reservation released. This relies on R2 not applying a
write after reporting its failure. If it ever did, afmail would still list and
import the late object, so the effect is a temporary undercount until it is
acknowledged, never lost mail. Out-of-band writers bypass admission and are
unsupported.

Recipient allowlists work without an additional service. Strict sender policy
requires a trusted `SENDER_VERIFIER` Worker service binding. It receives raw MIME
via POST, with `X-Envelope-From` and `X-Envelope-To`, and must independently verify
signatures/alignment and return JSON such as
`{"aligned":true,"authenticated_domain":"example.com","dmarc":"pass"}`.
Do not implement it by trusting sender-provided `Authentication-Results` headers.
No verifier is bundled: enabling strict policy without the binding rejects mail.
An authenticated sender can still generate spam, so retain size, rate, and total
limits even with verification enabled. Forwarding happens only after storage.

## Optional agent wakeups

After deployment, connect the bucket pending prefix (`inbox/`) to a Cloudflare
Queue with an `object-create` notification and enable HTTP pull on that Queue.
Do not include the archive prefix. On the mailbox host, configure
`cloudflare.queue.queue_id`, `cloudflare.queue.api_token_secret`, and
`cloudflare.consumer.command`, then run `afmail cloudflare watch`. The consumer
keeps the existing spool control acknowledgement for quota accounting and
invokes the configured command only for durable new-message tasks. See the
afmail repository guide `docs/cloudflare-consumer.md` for the handler contract,
service setup, and recovery boundaries.
