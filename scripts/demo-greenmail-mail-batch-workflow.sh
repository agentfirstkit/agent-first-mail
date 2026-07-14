#!/usr/bin/env bash
set -euo pipefail

# Runs a human-facing GreenMail workflow demo against the mail-batch fixtures.
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
FIXTURE_DIR="$CRATE_DIR/tests/fixtures/mail-batch"
MODE=${1:-prepare}
WORKSPACE=${AFMAIL_DEMO_WORKSPACE:-$(mktemp -d "${TMPDIR:-/tmp}/afmail-demo.XXXXXX")}
IMAGE=greenmail/standalone:2.1.8
CONTAINER=${AFMAIL_DEMO_CONTAINER:-afmail-demo-greenmail-$(date +%s)-$$}
STEP_DELAY_S=${AFMAIL_DEMO_STEP_DELAY_S:-0}

case "$MODE" in
  prepare | ui | run) ;;
  *)
    printf 'usage: %s [prepare|ui|run]\n' "$0" >&2
    exit 2
    ;;
esac

if [[ "$MODE" == "prepare" || "$MODE" == "ui" ]]; then
  KEEP_CONTAINER=1
else
  KEEP_CONTAINER=0
fi

cleanup() {
  if [[ "$KEEP_CONTAINER" != "1" ]]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

say() {
  printf '\n==> %s\n' "$*"
}

pause() {
  python3 - "$STEP_DELAY_S" <<'PY'
import sys, time
try:
    delay = float(sys.argv[1])
except ValueError:
    delay = 0.0
if delay > 0:
    time.sleep(delay)
PY
}

run_afmail() {
  if [[ -x "$WORKSPACE/bin/afmail" ]]; then
    (cd "$WORKSPACE" && "$WORKSPACE/bin/afmail" "$@")
  elif [[ -n "${AFMAIL_BIN:-}" ]]; then
    (cd "$WORKSPACE" && "$AFMAIL_BIN" "$@")
  else
    (cd "$WORKSPACE" && cargo run --quiet --manifest-path "$CRATE_DIR/Cargo.toml" -- "$@")
  fi
}

demo_cmd() {
  printf '\n$ afmail'
  printf ' %q' "$@"
  printf '\n'
  run_afmail "$@"
  pause
}

demo_capture() {
  printf '\n$ afmail' >&2
  printf ' %q' "$@" >&2
  printf '\n' >&2
  local output
  output=$(run_afmail "$@")
  printf '%s\n' "$output" >&2
  pause
  printf '%s' "$output"
}

json_field() {
  python3 -c '
import json, sys
field = sys.argv[1]
value = json.load(sys.stdin)
kind = value.get("kind") if isinstance(value, dict) else None
if kind in ("result", "error", "progress", "log"):
    value = value[kind]
for part in field.split("."):
    value = value[part]
print(value)
' "$1"
}

message_id_by_subject() {
  python3 - "$WORKSPACE" "$1" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1])
subject = sys.argv[2]
found = []
for path in sorted((root / 'messages').glob('*.json')):
    data = json.loads(path.read_text())
    if data.get('subject') == subject:
        found.append(data.get('message_id', path.stem))
if len(found) != 1:
    raise SystemExit(f'expected one message with subject {subject!r}, found {found}')
print(found[0])
PY
}

write_config() {
  local imap_port=$1
  local smtp_port=$2
  local junk_flag='$Junk'
  local default_mailbox_ids
  if [[ "$MODE" == "prepare" ]]; then
    # User-facing demos should start from INBOX only. The INBOX fixture
    # already includes messages the agent should recommend as spam/trash.
    default_mailbox_ids='["inbox"]'
  else
    default_mailbox_ids='["inbox", "sent", "archive", "junk", "trash"]'
  fi
  cat > "$WORKSPACE/.afmail/config.json" <<JSON
{
  "schema_name": "config",
  "schema_version": 1,
  "imap": {
    "host": "127.0.0.1",
    "port": $imap_port,
    "tls": false,
    "allow_unbounded_body_fetch": true,
    "username": "me",
    "password_secret": "secret"
  },
  "mailboxes": {
    "inbox": {"resolve": [{"mailbox_name": "INBOX"}]},
    "sent": {"resolve": [{"mailbox_name": "Sent"}]},
    "archive": {"resolve": [{"mailbox_name": "Archive"}]},
    "junk": {"resolve": [{"mailbox_name": "Junk"}]},
    "trash": {"resolve": [{"mailbox_name": "Trash"}]},
    "drafts": {"resolve": [{"mailbox_name": "Drafts"}]}
  },
  "actions": {
    "pull": {
      "default_mailbox_ids": $default_mailbox_ids,
      "by_mailbox_id": {
        "inbox": {"import_as": "triage", "direction": "inbound"},
        "sent": {"import_as": "triage", "direction": "outbound"},
        "archive": {"import_as": "triage", "direction": "inbound"},
        "junk": {"import_as": "spam", "direction": "inbound"},
        "trash": {"import_as": "trashed", "direction": "inbound"},
        "drafts": {"import_as": "triage", "direction": "outbound"}
      }
    },
    "case.add": {"steps": []},
    "draft.save": {"steps": [{"append_to_mailbox_id": "drafts"}]},
    "draft.send": {"steps": [
      {"smtp_send": {}},
      {"append_to_mailbox_id": "sent"},
      {"add_flags": ["\\\\Answered"], "on": "reply_to_message"}
    ]},
    "message.spam": {"steps": [
      {"add_flags": ["\\\\Seen", "$junk_flag"]},
      {"move_to_mailbox_id": "junk"}
    ]},
    "message.trash": {"steps": [{"move_to_mailbox_id": "trash"}]},
    "message.notification": {"by_source_mailbox_id": {
      "inbox": {"steps": [{"move_to_mailbox_id": "archive"}]},
      "sent": {"steps": []},
      "archive": {"steps": []},
      "junk": {"steps": []},
      "trash": {"steps": []},
      "drafts": {"steps": []}
    }}
  },
  "identities": [
    {"identity": "me", "name": "Me", "email": "me@localhost", "default": true}
  ],
  "case": {},
  "audit": {"reason_mode": "required"},
  "smtp": {
    "host": "127.0.0.1",
    "port": $smtp_port,
    "starttls": false,
    "tls_wrapper": false,
    "username": null,
    "password_secret": null
  },
  "workspace": {"language_bcp47": null, "timezone_utc_offset": "UTC"}
}
JSON
}

wait_and_seed_greenmail() {
  local imap_port=$1
  FIXTURE_DIR="$FIXTURE_DIR" IMAP_PORT="$imap_port" python3 <<'PY'
import imaplib, json, os, pathlib, sys, time
fixture_dir = pathlib.Path(os.environ['FIXTURE_DIR'])
imap_port = int(os.environ['IMAP_PORT'])
manifest = json.loads((fixture_dir / 'manifest.json').read_text())
deadline = time.time() + 45
last_error = None
while time.time() < deadline:
    try:
        client = imaplib.IMAP4('127.0.0.1', imap_port)
        client.login('me', 'secret')
        break
    except Exception as exc:
        last_error = exc
        time.sleep(0.25)
else:
    raise SystemExit(f'GreenMail IMAP did not become ready: {last_error}')

for mailbox in manifest['mailboxes'].values():
    folder = mailbox['mailbox_name']
    if folder != 'INBOX':
        client.create(folder)

for message in manifest['messages']:
    folder = manifest['mailboxes'][message['mailbox_id']]['mailbox_name']
    raw = (fixture_dir / message['eml_path']).read_bytes()
    status, data = client.append(folder, None, None, raw)
    if status != 'OK':
        raise SystemExit(f"append {message['fixture_id']} to {folder} failed: {data!r}")
client.logout()
print(f"seeded {len(manifest['messages'])} fixture messages")
PY
}

prepare_afmail_wrapper() {
  say "Building local afmail binary wrapper"
  cargo build --quiet --manifest-path "$CRATE_DIR/Cargo.toml" --bin afmail
  mkdir -p "$WORKSPACE/bin"
  cat > "$WORKSPACE/bin/afmail" <<SH
#!/usr/bin/env bash
exec "$CRATE_DIR/target/debug/afmail" "\$@"
SH
  chmod +x "$WORKSPACE/bin/afmail"
}

write_mock_crm() {
  say "Writing mock CRM tool"
  mkdir -p "$WORKSPACE/bin" "$WORKSPACE/crm"
  cat > "$WORKSPACE/crm/customers.json" <<'JSON'
{
  "alice@localhost": {
    "customer_id": "cust_alice",
    "display_name": "Alice Customer",
    "email": "alice@localhost",
    "tier": "standard",
    "lifetime_order_count": 4,
    "support_notes": [
      "Prefers short, direct updates.",
      "Had one late-delivery complaint in 2025; resolved with a shipping credit."
    ]
  }
}
JSON
  cat > "$WORKSPACE/crm/orders.json" <<'JSON'
{
  "4471": {
    "order_id": "4471",
    "customer_email": "alice@localhost",
    "status": "delivered",
    "order_total_usd": 86.40,
    "shipping_fee_usd": 12.99,
    "shipping_method": "Ground",
    "tracking_id": "SHIP-4471",
    "expected_delivery_date": "2026-05-19",
    "actual_delivery_date": "2026-05-21",
    "days_late": 2,
    "cold_pack_required": true,
    "delivery_temperature_note": "Carrier scan notes package was warm on arrival.",
    "refund_eligible": true,
    "recommended_action": "Refund the shipping fee, apologize for the late warm delivery, and note the order for QA review."
  }
}
JSON
  cat > "$WORKSPACE/crm/policies.json" <<'JSON'
{
  "shipping_refund": {
    "policy_id": "shipping_refund",
    "summary": "Refund shipping when a perishable order arrives late and cold-pack integrity is affected.",
    "max_auto_refund_usd": 20.00,
    "requires_manager_approval": false
  }
}
JSON
  cat > "$WORKSPACE/crm/refunds.json" <<'JSON'
{}
JSON
  cat > "$WORKSPACE/bin/crm" <<'PY'
#!/usr/bin/env python3
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
CRM = ROOT / "crm"


def load(name):
    with (CRM / name).open() as handle:
        return json.load(handle)


def emit(payload):
    print(json.dumps(payload, separators=(",", ":"), sort_keys=True))


def usage():
    emit({
        "code": "crm_usage",
        "commands": [
            "crm customer show <email>",
            "crm order show <order_id>",
            "crm refund policy <order_id>",
            "crm refund order <order_id>"
        ]
    })
    return 2


def main(argv):
    if len(argv) < 2:
        return usage()
    if argv[:2] == ["customer", "show"] and len(argv) == 3:
        customers = load("customers.json")
        customer = customers.get(argv[2])
        if not customer:
            emit({"code": "crm_not_found", "kind": "customer", "email": argv[2]})
            return 1
        emit({"code": "crm_customer", "customer": customer})
        return 0
    if argv[:2] == ["order", "show"] and len(argv) == 3:
        orders = load("orders.json")
        order = orders.get(argv[2])
        if not order:
            emit({"code": "crm_not_found", "kind": "order", "order_id": argv[2]})
            return 1
        customer = load("customers.json").get(order["customer_email"])
        emit({"code": "crm_order", "order": order, "customer": customer})
        return 0
    if argv[:2] == ["refund", "policy"] and len(argv) == 3:
        orders = load("orders.json")
        order = orders.get(argv[2])
        if not order:
            emit({"code": "crm_not_found", "kind": "order", "order_id": argv[2]})
            return 1
        policy = load("policies.json")["shipping_refund"]
        emit({
            "code": "crm_refund_policy",
            "order_id": argv[2],
            "eligible": bool(order.get("refund_eligible")),
            "shipping_fee_usd": order.get("shipping_fee_usd"),
            "recommended_action": order.get("recommended_action"),
            "policy": policy
        })
        return 0
    if argv[:2] == ["refund", "order"] and len(argv) == 3:
        orders = load("orders.json")
        order = orders.get(argv[2])
        if not order:
            emit({"code": "crm_not_found", "kind": "order", "order_id": argv[2]})
            return 1
        policy = load("policies.json")["shipping_refund"]
        if not order.get("refund_eligible"):
            emit({
                "code": "crm_refund_rejected",
                "order_id": argv[2],
                "reason": "order is not eligible for the configured refund policy"
            })
            return 1
        refund = {
            "refund_id": f"rfnd_{argv[2]}_shipping",
            "order_id": argv[2],
            "amount_usd": order.get("shipping_fee_usd"),
            "reason": "late warm delivery",
            "policy_id": policy["policy_id"],
            "status": "issued",
            "customer_email": order.get("customer_email")
        }
        order["refund"] = refund
        order["status"] = "refunded_shipping"
        with (CRM / "orders.json").open("w") as handle:
            json.dump(orders, handle, indent=2, sort_keys=True)
            handle.write("\n")
        refunds_path = CRM / "refunds.json"
        if refunds_path.exists():
            refunds = load("refunds.json")
        else:
            refunds = {}
        refunds[refund["refund_id"]] = refund
        with refunds_path.open("w") as handle:
            json.dump(refunds, handle, indent=2, sort_keys=True)
            handle.write("\n")
        emit({
            "code": "crm_refund_issued",
            "refund": refund,
            "recommended_reply_note": "Tell Alice the $12.99 shipping refund has been issued and apologize for the late warm delivery."
        })
        return 0
    return usage()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
PY
  chmod +x "$WORKSPACE/bin/crm"
}

write_agent_operating_notes() {
  cat >> "$WORKSPACE/AGENTS.md" <<'EOF'

## Demo CRM Order Lookup

A small mock CRM is available for order context:

- `./bin/crm order show 4471`
- `./bin/crm customer show alice@localhost`
- `./bin/crm refund policy 4471`
- `./bin/crm refund order 4471`

Use the returned JSON as order/customer context when it helps with the Alice
refund case.
EOF
}

print_prepare_summary() {
  cat <<EOF

Ready for real agent operation.

Workspace:  $WORKSPACE
Command:    cd "$WORKSPACE"
Mailbox:    GreenMail container=$CONTAINER imap=$IMAP_PORT smtp=$SMTP_PORT
afmail:     ./bin/afmail
crm:        ./bin/crm

Optional global source install:

cargo install --path "$CRATE_DIR" --bin afmail --locked --force

Now open your agent in that workspace and say exactly:

Check mail.

Then continue with short demo prompts:

Clean spam.
Check the refund request.
refund this order, reply.

The 30 fixture emails are still only in the remote IMAP mailbox. The default
demo flow pulls INBOX only; INBOX includes several messages the agent should
recommend as spam or trash. A small mock CRM is available for the Alice order
case. Nothing will be sent unless you explicitly confirm a push/send action.

Stop the mailbox when done:

docker rm -f $CONTAINER
EOF
}

print_ui_summary() {
  cat <<EOF

Ready for AFUI product verification.

Workspace:  $WORKSPACE
Mailbox:    GreenMail container=$CONTAINER imap=$IMAP_PORT smtp=$SMTP_PORT
Draft:      case=$CASE_UID file=$DRAFT_NAME

Build or locate the generic afui Host, then run inside the workspace:

./bin/afmail ui --afui-bin /path/to/afui

The UI workspace contains the 30-message fixture set and one editable local
reply draft. Stop the mailbox when done:

docker rm -f $CONTAINER
EOF
}

say "Starting GreenMail container $CONTAINER"
docker run -d --rm --name "$CONTAINER" \
  -e "GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled -Dgreenmail.users=me:secret@localhost,alice:secret@localhost" \
  -p 127.0.0.1::3025 -p 127.0.0.1::3143 "$IMAGE" >/dev/null

SMTP_PORT=$(docker port "$CONTAINER" 3025/tcp | awk -F: '{print $NF}')
IMAP_PORT=$(docker port "$CONTAINER" 3143/tcp | awk -F: '{print $NF}')
say "Seeding 30-message fixture batch into GreenMail IMAP on port $IMAP_PORT"
wait_and_seed_greenmail "$IMAP_PORT"

say "Creating afmail demo workspace: $WORKSPACE"
mkdir -p "$WORKSPACE"
prepare_afmail_wrapper
write_mock_crm
demo_cmd init
write_config "$IMAP_PORT" "$SMTP_PORT"
write_agent_operating_notes

demo_cmd remote test
demo_cmd remote folders

if [[ "$MODE" == "prepare" ]]; then
  print_prepare_summary
  exit 0
fi

demo_cmd pull
demo_cmd status
demo_cmd triage list

REFUND_INITIAL=$(message_id_by_subject 'Refund request for order 4471')
REFUND_FOLLOWUP=$(message_id_by_subject 'Re: Refund request for order 4471')
RECEIPT_CLOUD=$(message_id_by_subject 'Receipt INV-1001 from Cloudbox')
INBOX_PHISH=$(message_id_by_subject 'Action required: mailbox password expires')
INBOX_DUPLICATE=$(message_id_by_subject 'Duplicate notification')
CONTRACT_ATTACHMENT=$(message_id_by_subject 'Acme renewal contract')

demo_cmd message show "$REFUND_INITIAL"
demo_cmd message attachment fetch "$CONTRACT_ATTACHMENT"

CASE_JSON=$(demo_capture case create --name refund-order-4471 --message "$REFUND_INITIAL" --group support --reason 'customer refund thread needs a reply')
CASE_UID=$(printf '%s' "$CASE_JSON" | json_field case_uid)
CASE_PATH=$(printf '%s' "$CASE_JSON" | json_field case_path)
demo_cmd case add "$CASE_UID" "$REFUND_FOLLOWUP" --reason 'same refund thread'

REPLY_JSON=$(demo_capture case draft reply "$CASE_UID" "$REFUND_FOLLOWUP")
DRAFT_NAME=$(printf '%s' "$REPLY_JSON" | json_field draft_name)
cat > "$WORKSPACE/$CASE_PATH/drafts/$DRAFT_NAME" <<EOF
---
kind: draft
case_uid: $CASE_UID
send_intent: reply
reply_to_message_id: $REFUND_FOLLOWUP
to:
  - Alice Customer <alice@localhost>
cc: []
subject: "Re: Refund request for order 4471"
attachments:
---

<!-- afmail:compose:start -->
Hi Alice,

The shipping refund has been approved. I am sorry the cold pack arrived warm, and I have added a note to the order.

Best,
Me
<!-- afmail:compose:end -->
EOF

demo_cmd case draft validate "$CASE_UID" "$DRAFT_NAME"
if [[ "$MODE" == "ui" ]]; then
  print_ui_summary
  exit 0
fi
demo_cmd case draft send "$CASE_UID" "$DRAFT_NAME"
demo_cmd notification create --name receipts --message "$RECEIPT_CLOUD" --summary 'cloud service receipt' --reason 'receipt can be filed'
demo_cmd message spam "$INBOX_PHISH" --reason 'obvious phishing in inbox'
demo_cmd message trash "$INBOX_DUPLICATE" --reason 'duplicate notification from inbox'

PUSH_PLAN_JSON=$(demo_capture push --dry-run)
PUSH_PLAN_ID=$(printf '%s' "$PUSH_PLAN_JSON" | json_field plan_id)
demo_cmd push --confirm "$PUSH_PLAN_ID"
demo_cmd pull
demo_cmd status

cat <<EOF

Demo workspace: $WORKSPACE
GreenMail:      container=$CONTAINER imap=$IMAP_PORT smtp=$SMTP_PORT

Suggested GIF beats:
1. Pull 30 realistic messages from GreenMail.
2. Inspect triage and a refund thread.
3. Create a case, draft a reply, and queue it.
4. File a receipt, mark an INBOX phishing message as spam, trash a duplicate.
5. Dry-run then confirm pushes to SMTP/IMAP.
6. Show final status and the generated workspace files.

From the repository root, use
\`./scripts/projects/agent-first-mail/test.sh demo-prepare\` when you want a live
workspace where an agent can really receive and inspect the mailbox itself.
EOF
