#!/usr/bin/env python3
"""Opt-in Cloudflare live tests; all resources belong to one private run directory.

Requires cf, npm, and a default-feature target/debug/afmail build. Credentials
never enter argv or the mailbox workspace. Run --help for the staged entrypoints.
"""

import argparse
import datetime as dt
import email.policy
import email.utils
from email.message import EmailMessage
from email.parser import BytesParser
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target/debug/afmail"


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.chmod(0o600)


def event(step, **fields):
    print(json.dumps({"step": step, **fields}), flush=True)


class LiveTest:
    def __init__(self, args):
        self.args = args
        self.root = args.state_dir.resolve()
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.root.chmod(0o700)
        self.manifest_path = self.root / "manifest.json"
        if self.manifest_path.exists():
            self.state = json.loads(self.manifest_path.read_text())
            if args.account_id and args.account_id != self.state["account_id"]:
                raise RuntimeError("account_id differs from the saved run")
            if args.domain and args.domain != self.state["domain"]:
                raise RuntimeError("domain differs from the saved run")
        else:
            if not args.account_id or not args.domain:
                raise RuntimeError("new runs require --account-id and --domain")
            if not re.fullmatch(r"[a-f0-9]{32}", args.account_id):
                raise RuntimeError("invalid account id")
            if not re.fullmatch(r"[a-z0-9.-]+", args.domain):
                raise RuntimeError("invalid domain")
            run = "afmail-live-" + uuid.uuid4().hex[:8]
            self.state = {
                "account_id": args.account_id,
                "domain": args.domain,
                "run": run,
                "bucket": run,
                "worker": run,
                "recipients": [f"{run}-a@{args.domain}", f"{run}-b@{args.domain}"],
                "sender_domain": f"{run}.{args.domain}",
                "sender": f"test@{run}.{args.domain}",
            }
            self.save()
        self.control = self.root / "control"
        self.control.mkdir(exist_ok=True)
        (self.control / "cloudflare.config.ts").write_text(
            "export default " + json.dumps({"accountId": self.state["account_id"]}) + ";\n"
        )
        self.workspace = self.root / "mailbox"
        self.worker = self.root / "worker"
        self.secrets_path = self.root / "credentials.secret.json"
        self.secrets = json.loads(self.secrets_path.read_text()) if self.secrets_path.exists() else {}
        self.env = {**os.environ, "CLOUDFLARE_ACCOUNT_ID": self.state["account_id"], "NO_COLOR": "1"}

    def save(self):
        write_json(self.manifest_path, self.state)

    def command(self, argv, cwd=None, raw=False, log=None, env=None):
        p = subprocess.run(argv, cwd=cwd or self.control, env=env or self.env, capture_output=True)
        if log:
            (self.root / log).write_bytes(p.stdout + p.stderr)
        if p.returncode:
            message = (p.stderr or p.stdout).decode(errors="replace")
            for secret in self.secrets.values():
                if isinstance(secret, str):
                    message = message.replace(secret, "***")
            raise RuntimeError(f"{' '.join(argv[:4])} exited {p.returncode}: {message[-5000:]}")
        if raw:
            return p.stdout
        return p.stdout.decode()

    def cf(self, *args):
        result = self.command(["cf", *args])
        return json.loads(result) if result.strip() else None

    def af(self, *args):
        text = self.command([str(BINARY), *args], cwd=self.workspace)
        value = json.loads(text)
        if value.get("kind") != "result":
            raise RuntimeError(f"afmail did not return a result: {text}")
        return value["result"]

    def config(self, key, value):
        if value is None:
            if key != "cloudflare.r2.imported_prefix":
                raise RuntimeError(f"unsupported nullable test config key: {key}")
            # config get deliberately redacts secret references. Reconstruct
            # this test-owned section from the run manifest, never round-trip
            # a redacted config object back into persistent configuration.
            return self.config("cloudflare.r2", {
                "bucket": self.state["bucket"], "prefix": "inbox/", "imported_prefix": None,
                "access_key_id": self.state["r2_token_id"],
                "secret_access_key_secret": f"file:{self.secrets_path}#r2_access_key_secret",
            })
        if isinstance(value, (dict, list, bool)) or value is None:
            value = json.dumps(value)
        self.af("config", "set", key, str(value))

    def token(self, kind, permission_names, resource):
        if f"{kind}_token_id" in self.state:
            if f"{kind}_token_secret" not in self.secrets:
                raise RuntimeError("saved token secret is missing; revoke the run's token before replacing it")
            return
        groups = self.cf("user", "tokens", "permission-groups", "list")
        permissions = []
        for name in permission_names:
            matches = [p for p in groups if p["name"] == name]
            if len(matches) != 1:
                raise RuntimeError(f"permission group not uniquely found: {name}")
            permissions.append({"id": matches[0]["id"]})
        policy = [{"effect": "allow", "resources": {resource: "*"}, "permission_groups": permissions}]
        expires = (dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=2)).strftime("%Y-%m-%dT%H:%M:%SZ")
        token = self.cf("user", "tokens", "create", "--name", f"{self.state['run']}-{kind}",
                        "--expires-on", expires, "--policies", json.dumps(policy))
        self.secrets[f"{kind}_token_secret"] = token["value"]
        self.state[f"{kind}_token_id"] = token["id"]
        self.state[f"{kind}_expires_rfc3339"] = expires
        if kind == "r2":
            self.secrets["r2_access_key_secret"] = hashlib.sha256(token["value"].encode()).hexdigest()
        write_json(self.secrets_path, self.secrets)
        self.save()
        event("token_created", scope=kind, expires_rfc3339=expires)

    def prepare_storage(self):
        if not self.state.get("bucket_created"):
            buckets = self.cf("r2", "buckets", "list")["buckets"]
            if any(b["name"] == self.state["bucket"] for b in buckets):
                raise RuntimeError("bucket already exists but was not created by this run")
            result = self.cf("r2", "buckets", "create", "--name", self.state["bucket"],
                             "--storage-class", "Standard")
            self.state["bucket_created"] = True
            self.save()
            event("bucket_created", bucket=result["name"])
        resource = f"com.cloudflare.edge.r2.bucket.{self.state['account_id']}_default_{self.state['bucket']}"
        self.token("r2", ["Workers R2 Storage Bucket Item Write"], resource)
        if not self.workspace.exists():
            self.workspace.mkdir()
            self.af("init")
        self.config("cloudflare.account_id", self.state["account_id"])
        self.config("cloudflare.r2.bucket", self.state["bucket"])
        self.config("cloudflare.r2.access_key_id", self.state["r2_token_id"])
        self.config("cloudflare.r2.secret_access_key_secret", f"file:{self.secrets_path}#r2_access_key_secret")
        self.config("identities", [{"identity": "test", "name": "Mail Test", "email": self.state["sender"], "default": True},
                                   {"identity": "receiver", "name": "Receiver", "email": self.state["recipients"][0]}])
        if not self.worker.exists():
            self.command([str(BINARY), "cloudflare", "scaffold", str(self.worker)])
            for name in ["cloudflare.config.ts", "wrangler.jsonc"]:
                path = self.worker / name
                path.write_text(path.read_text().replace("afmail-inbound", self.state["worker"])
                                .replace("afmail-spool", self.state["bucket"]))
            path = self.worker / "cloudflare.config.ts"
            path.write_text(path.read_text().replace('  worker: {', f'  accountId: "{self.state["account_id"]}",\n  worker: {{')
                            .replace('["support@example.com"]', json.dumps(self.state["recipients"])))
        if not self.state.get("worker_deployed"):
            self.command(["npm", "ci", "--no-audit", "--no-fund"], cwd=self.worker, log="worker-install.log")
            self.command(["npm", "run", "check"], cwd=self.worker, log="worker-check.log")
            self.command(["cf", "deploy"], cwd=self.worker, log="worker-deploy.log")
            self.state["worker_deployed"] = True
            self.state["quota_namespace_created"] = True
            self.save()
        self.prepare_spool_control()
        rules = self.cf("email-routing", "rules", "list-account", "--zone", self.state["domain"])
        for address in self.state["recipients"]:
            matches = [r for r in rules if r.get("enabled") and
                       any(m.get("value") == address for m in r["matchers"]) and
                       r["actions"] == [{"type": "worker", "value": [self.state["worker"]]}]]
            if len(matches) != 1:
                raise RuntimeError(f"expected one enabled Worker route for {address}")
        event("receiving_ready", bucket=self.state["bucket"], worker=self.state["worker"], recipients=self.state["recipients"])
        result = self.af("remote", "test")
        write_json(self.root / "remote-storage-test.json", result)
        event("s3_connection_verified", result=result)

    def prepare_spool_control(self):
        # Keep the shared secret in private files; never put its value in argv.
        if "spool_control_token_secret" not in self.secrets:
            self.secrets["spool_control_token_secret"] = uuid.uuid4().hex + uuid.uuid4().hex
            write_json(self.secrets_path, self.secrets)
        secret_file = self.root / "worker-secrets.secret.json"
        write_json(secret_file, {"SPOOL_CONTROL_TOKEN_SECRET": {
            "type": "secret_text", "text": self.secrets["spool_control_token_secret"]}})
        try:
            self.command(["cf", "workers", "secrets", "bulk", "--worker", self.state["worker"],
                          "--secrets", "@" + str(secret_file)])
        finally:
            secret_file.unlink(missing_ok=True)
        control_url = self.args.spool_control_url or self.state.get("spool_control_url")
        if not control_url:
            deploy_log = (self.root / "worker-deploy.log").read_text()
            urls = re.findall(r"https://" + re.escape(self.state["worker"]) + r"\.[a-z0-9-]+\.workers\.dev", deploy_log)
            if urls:
                control_url = urls[-1]
        if not control_url:
            raise RuntimeError("Worker URL missing from deployment output; retry with --spool-control-url")
        self.state["spool_control_url"] = control_url
        self.save()
        self.config("cloudflare.spool", {"control_url": control_url,
                    "token_secret": f"file:{self.secrets_path}#spool_control_token_secret"})

    def prepare_sender(self):
        if not self.state.get("sender_subdomain_id"):
            rows = self.cf("email-sending", "subdomains", "list", "--zone", self.state["domain"])
            if any(r.get("name") == self.state["sender_domain"] for r in rows):
                raise RuntimeError("sender domain already exists but is not owned by this run")
            row = self.cf("email-sending", "subdomains", "create", "--zone", self.state["domain"],
                          "--name", self.state["sender_domain"])
            write_json(self.root / "sender-create.json", row)
            self.state["sender_subdomain_id"] = row["id"]
            self.save()
        status = self.cf("email-sending", "subdomains", "dns", "status", self.state["sender_subdomain_id"],
                         "--zone", self.state["domain"])
        write_json(self.root / "sender-dns.json", status)
        event("sender_dns", result=status)
        self.token("smtp", ["Email Sending Write"], f"com.cloudflare.api.account.{self.state['account_id']}")

    def inspect(self):
        event("manifest", **self.state)
        event("objects", result=self.objects())

    def objects(self, prefix="inbox/"):
        return self.cf("r2", "objects", "list", "--bucket-name", self.state["bucket"], "--prefix", prefix)

    def deliver(self):
        if self.state.get("inbound_send_attempted"):
            raise RuntimeError("send was already attempted; use observe instead of sending again")
        if self.objects():
            raise RuntimeError("the test inbox must be empty before the first send")
        message = EmailMessage(policy=email.policy.SMTP)
        message["From"] = f"Mail Test <{self.state['sender']}>"
        message["To"] = self.state["recipients"][0]
        message["Date"] = email.utils.format_datetime(dt.datetime.now(dt.timezone.utc))
        message["Message-ID"] = f"<{self.state['run']}@{self.state['sender_domain']}>"
        message["Subject"] = f"afmail live test {self.state['run']}"
        message.set_content("Synthetic Cloudflare receive test. No user mail or private content.")
        raw = message.as_string()
        request = {"from": self.state["sender"], "recipients": self.state["recipients"], "mime_message": raw}
        write_json(self.root / "delivery-request.json", request)
        self.state["inbound_send_attempted"] = True
        self.save()
        result = self.cf("email-sending", "send-raw", "--body", json.dumps(request))
        write_json(self.root / "delivery-result.json", result)
        event("send_accepted", result=result)
        self.observe()

    def observe(self):
        deadline = time.monotonic() + 180
        while True:
            objects = self.objects()
            if len(objects) >= 2:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError(f"expected two deliveries, found {len(objects)}; inspect sender delivery logs")
            event("waiting_for_deliveries", object_count=len(objects))
            time.sleep(5)
        if len(objects) != 2:
            raise RuntimeError("expected exactly two deliveries in the isolated test bucket")
        observed = []
        for i, obj in enumerate(objects):
            key = obj["key"]
            raw = self.command(["cf", "r2", "objects", "get", key, "--bucket-name", self.state["bucket"]], raw=True)
            (self.root / f"received-{i}.eml").write_bytes(raw)
            observed.append({"key": key, "size_bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest(), "object": obj})
        write_json(self.root / "received.json", observed)
        equal = len({o["sha256"] for o in observed}) == 1
        self.state["delivery_raw_bytes_equal"] = equal
        self.save()
        event("delivery_observed", raw_bytes_equal=equal, objects=observed)

    def consume(self):
        observed = json.loads((self.root / "received.json").read_text())
        self.config("cloudflare.r2.secret_access_key_secret", f"file:{self.secrets_path}#r2_access_key_secret")
        if self.state.get("receive_tests_passed"):
            event("receive_tests_already_passed")
            return
        if self.state.get("consume_attempted") and not (self.root / "pull-import.json").exists():
            raise RuntimeError("interrupted initial consume; inspect remote and local state before resuming")
        self.state["consume_attempted"] = True
        self.save()
        if (self.root / "pull-import.json").exists():
            result = json.loads((self.root / "pull-import.json").read_text())
        else:
            dry = self.af("pull", "--cloudflare", "--dry-run")
            write_json(self.root / "pull-dry-run.json", dry)
            if len(self.objects()) != 2:
                raise RuntimeError("dry run changed the remote inbox")
            self.config("cloudflare.r2.imported_prefix", "imported/")
            result = self.af("pull", "--cloudflare")
            write_json(self.root / "pull-import.json", result)
        local = list((self.workspace / ".afmail/messages").glob("*.eml"))
        expected_hashes = {o["sha256"] for o in observed}
        if {hashlib.sha256(p.read_bytes()).hexdigest() for p in local} != expected_hashes:
            raise RuntimeError("local raw EML bytes differ from the R2 originals")
        pending = self.objects()
        if any(o["key"] != "inbox/replay.eml" for o in pending) or len(self.objects("imported/")) != 2:
            raise RuntimeError("archive acknowledgement did not move both objects")
        envelopes = set()
        for path in local:
            metadata = json.loads(path.with_suffix(".json").read_text())
            envelopes.update(metadata["delivery"]["envelope_to"])
            cached = json.loads((self.workspace / "messages" / path.with_suffix(".json").name).read_text())
            if not set(metadata["delivery"]["envelope_to"]).issubset(cached["envelope_to"]):
                raise RuntimeError("delivery recipients were not projected into cached envelope_to")
        if envelopes != set(self.state["recipients"]):
            raise RuntimeError("SMTP envelope recipients were lost")
        if (self.root / "pull-empty.json").exists():
            empty = json.loads((self.root / "pull-empty.json").read_text())
        else:
            if pending:
                raise RuntimeError("cannot verify an empty pull while replay is pending")
            empty = self.af("pull", "--cloudflare")
            write_json(self.root / "pull-empty.json", empty)
        self.config("cloudflare.r2.imported_prefix", None)
        self.cf("r2", "objects", "put", "inbox/replay.eml", "--bucket-name", self.state["bucket"],
                "--file", str(self.root / "received-0.eml"), "--content-type", "message/rfc822")
        replay = self.af("pull", "--cloudflare")
        write_json(self.root / "pull-replay.json", replay)
        if len(list((self.workspace / ".afmail/messages").glob("*.eml"))) != len(local) or self.objects():
            raise RuntimeError("replayed content was not deduplicated and acknowledged")
        self.state["receive_tests_passed"] = True
        self.save()
        event("receive_tests_passed", unique_message_count=len(local), raw_bytes_equal=self.state["delivery_raw_bytes_equal"],
              import_result=result, empty_result=empty, replay_result=replay)

    def smtp_preview(self):
        self.sending_preview("smtp")

    def smtp_send(self):
        self.sending_send("smtp")

    def smtp_observe(self):
        self.sending_observe("smtp")

    def api_preview(self):
        self.sending_preview("api")

    def api_send(self):
        self.sending_send("api")

    def api_observe(self):
        self.sending_observe("api")

    def sending_preview(self, transport):
        if self.state.get(f"{transport}_plan_id"):
            event(f"{transport}_plan", result=json.loads((self.root / f"{transport}-preview.json").read_text()))
            return
        self.config("actions.draft.save.steps", [])
        operation = "smtp_send" if transport == "smtp" else "cloudflare_send"
        self.config("actions.draft.send.steps", [{operation: {}}])
        if transport == "smtp":
            for key, value in {"smtp.host": "smtp.mx.cloudflare.net", "smtp.port": 465, "smtp.starttls": False,
                               "smtp.tls_wrapper": True, "smtp.username": "api_token",
                               "smtp.password_secret": f"file:{self.secrets_path}#smtp_token_secret"}.items():
                self.config(key, value)
        else:
            self.config("smtp", {})
            self.config("cloudflare.sending.api_token_secret", f"file:{self.secrets_path}#smtp_token_secret")
        result = self.af("remote", "test")
        write_json(self.root / f"remote-{transport}-test.json", result)
        event(f"{transport}_config_test", result=result)
        if (self.root / f"{transport}-case.json").exists():
            case = json.loads((self.root / f"{transport}-case.json").read_text())
        else:
            case = self.af("case", "create", "--name", f"Cloudflare live {transport.upper()} test", "--group", "tests")
            write_json(self.root / f"{transport}-case.json", case)
        self.state[f"{transport}_case_ref"] = case["case_uid"]
        self.save()
        draft = self.af("case", "draft", "new", case["case_uid"], "--to", self.state["recipients"][0],
                        "--subject", f"afmail {transport.upper()} live test {self.state['run']}",
                        "--body", f"Synthetic {transport.upper()} test sent by afmail. No user mail or private content.")
        write_json(self.root / f"{transport}-draft.json", draft)
        self.state[f"{transport}_draft_name"] = draft["draft_name"]
        self.save()
        self.af("case", "draft", "send", case["case_uid"], draft["draft_name"])
        preview = self.af("push")
        write_json(self.root / f"{transport}-preview.json", preview)
        items = preview["items"]
        if len(items) != 1 or items[0]["actions"] != [operation]:
            raise RuntimeError("the live send plan must contain exactly one selected transport step")
        self.state[f"{transport}_plan_id"] = preview["plan_id"]
        self.save()
        event(f"{transport}_plan", result=preview)

    def sending_send(self, transport):
        plan_id = self.args.confirm_plan_id
        if not plan_id or plan_id != self.state.get(f"{transport}_plan_id"):
            raise RuntimeError("--confirm-plan-id must equal the exact reviewed sending preview plan")
        if self.state.get(f"{transport}_send_attempted"):
            raise RuntimeError("sending was already attempted; use the corresponding observe stage and inspect the saved result, never resend blindly")
        preview = self.af("push")
        if preview["plan_id"] != plan_id:
            raise RuntimeError("the plan changed; review and authorize the new preview")
        if self.objects():
            raise RuntimeError("the test inbox must be empty before sending submission")
        self.state[f"{transport}_send_attempted"] = True
        self.save()
        result = self.af("push", "--confirm", plan_id)
        write_json(self.root / f"{transport}-send.json", result)
        if transport == "api":
            receipts = [receipt for item in result["succeeded_items"] for receipt in item.get("send_receipts", [])]
            if len(receipts) != 1 or not receipts[0]["message_id"]:
                raise RuntimeError("API submission did not retain exactly one provider receipt")
            if receipts[0]["permanent_bounces"] or receipts[0]["suppressed_recipients"]:
                raise RuntimeError("API test recipient was rejected")
        event(f"{transport}_submission_result", result=result)
        self.sending_observe(transport)

    def sending_observe(self, transport):
        if not (self.root / f"{transport}-send.json").exists():
            raise RuntimeError("no confirmed sending result exists; inspect push status before proceeding")
        if self.state.get(f"{transport}_tests_passed"):
            event(f"{transport}_tests_already_passed")
            return
        deadline = time.monotonic() + 180
        while True:
            objects = self.objects()
            if objects:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError("sending submission result is saved, but delivery was not observed; do not resend")
            event(f"waiting_for_{transport}_delivery")
            time.sleep(5)
        if len(objects) != 1:
            raise RuntimeError("expected exactly one sending test delivery")
        raw = self.command(["cf", "r2", "objects", "get", objects[0]["key"],
                            "--bucket-name", self.state["bucket"]], raw=True)
        (self.root / f"{transport}-received.eml").write_bytes(raw)
        parsed = BytesParser(policy=email.policy.default).parsebytes(raw)
        if str(parsed["Subject"]) != f"afmail {transport.upper()} live test {self.state['run']}":
            raise RuntimeError("received email is not the sending test message")
        body = parsed.get_body(preferencelist=("plain",))
        if body is None or f"Synthetic {transport.upper()} test sent by afmail." not in body.get_content():
            raise RuntimeError("sending test body did not survive delivery")
        if objects[0]["custom_metadata"]["envelope_to"] != self.state["recipients"][0]:
            raise RuntimeError("sending test was delivered to a different envelope recipient")
        result = self.af("pull", "--cloudflare")
        write_json(self.root / f"{transport}-pull.json", result)
        if self.objects():
            raise RuntimeError("sending delivery was not acknowledged after import")
        queue = self.af("push")
        if queue["count"] != 0:
            raise RuntimeError("sending push queue is not empty after confirmed send")
        case = json.loads((self.root / f"{transport}-case.json").read_text())
        collection = json.loads((self.workspace / case["case_path"] / "data/case.json").read_text())
        if len(collection["items"]) != 1:
            raise RuntimeError("expected exactly one local sent record in the test case")
        sent_path = self.workspace / ".afmail/messages" / f"{collection['items'][0]['message_id']}.eml"
        sent = BytesParser(policy=email.policy.default).parsebytes(sent_path.read_bytes())
        if str(sent["Subject"]) != str(parsed["Subject"]):
            raise RuntimeError("local sent evidence does not match the delivered test message")
        self.state[f"{transport}_tests_passed"] = True
        self.save()
        event(f"{transport}_tests_passed", received_sha256=hashlib.sha256(raw).hexdigest(),
              rfc822_message_id=str(parsed["Message-ID"]), pull_result=result)

    def verify_duplicate_hint(self):
        if not self.state.get("receive_tests_passed"):
            raise RuntimeError("complete the receive tests first")
        if self.state.get("duplicate_hint_verified"):
            event("duplicate_hint_already_verified")
            return
        self.workspace = self.root / "duplicate-mailbox"
        if self.workspace.exists():
            raise RuntimeError("duplicate-mailbox already exists; inspect its saved result before retrying")
        self.workspace.mkdir()
        self.af("init")
        self.config("identities", [{"identity": "hidden", "name": "Hidden Receiver",
                                     "email": self.state["recipients"][1], "default": True}])
        self.config("cloudflare", {"account_id": self.state["account_id"], "r2": {
            "bucket": self.state["bucket"], "prefix": "imported/",
            "access_key_id": self.state["r2_token_id"],
            "secret_access_key_secret": f"file:{self.secrets_path}#r2_access_key_secret"},
            "spool": {"control_url": self.state["spool_control_url"],
                      "token_secret": f"file:{self.secrets_path}#spool_control_token_secret"}})
        result = self.af("pull", "--cloudflare")
        write_json(self.root / "duplicate-hint.json", result)
        if result["new_message_count"] != 2 or result["cloudflare"]["merged_count"] != 0:
            raise RuntimeError("distinct deliveries were unexpectedly merged")
        warnings = result["cloudflare"]["warnings"]
        if len(warnings) != 1 or warnings[0]["code"] != "duplicate_candidate":
            raise RuntimeError("expected a weak duplicate candidate hint")
        hashes = {hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in (self.workspace / ".afmail/messages").glob("*.eml")}
        if hashes != {o["sha256"] for o in json.loads((self.root / "received.json").read_text())}:
            raise RuntimeError("raw evidence changed during duplicate candidate detection")
        hidden_matches = 0
        for path in (self.workspace / ".afmail/messages").glob("*.json"):
            metadata = json.loads(path.read_text())
            if self.state["recipients"][1] in metadata["delivery"]["envelope_to"]:
                document = self.af("message", "show", metadata["message_id"])
                if document["context"]["message"]["message"]["identity_email"] != self.state["recipients"][1]:
                    raise RuntimeError("hidden envelope recipient did not match the configured identity")
                if not document["context"]["message"]["analysis"]["security"]["possible_bcc"]:
                    raise RuntimeError("hidden recipient did not produce possible_bcc analysis")
                hidden_matches += 1
        if hidden_matches != 1:
            raise RuntimeError("expected exactly one hidden-recipient delivery")
        self.state["duplicate_hint_verified"] = True
        self.save()
        event("duplicate_hint_verified", hidden_identity_matched=True, result=result)

    def retire_spool(self):
        # Retire the class before deleting the Worker; deleting a script alone
        # is not proof that its Durable Object namespace/data were removed.
        (self.worker / "src/retire.ts").write_text(
            'export default { fetch() { return new Response(null, { status: 410 }); } };\n')
        worker_config = json.dumps({"name": self.state["worker"],
                                    "entrypoint": "src/retire.ts",
                                    "compatibilityDate": "2026-10-21"})
        (self.worker / "cloudflare.config.ts").write_text(
            'import { defineConfig, exports } from "cf/config";\n'
            'export default defineConfig({ accountId: ' + json.dumps(self.state["account_id"])
            + ', worker: { ...' + worker_config
            + ', exports: { SpoolQuota: exports.durableObject({ state: "deleted" }) } } });\n')
        self.command(["cf", "deploy"], cwd=self.worker, log="worker-retire.log")

    def cleanup(self):
        for transport in ["smtp", "api"]:
            if self.state.get(f"{transport}_plan_id") and not self.state.get(f"{transport}_tests_passed") and not self.args.discard_unsent:
                raise RuntimeError(f"{transport} is pending; finish it or explicitly pass --discard-unsent")
        run = self.state["run"]
        if not re.fullmatch(r"afmail-live-[a-f0-9]{8}", run) or self.state["bucket"] != run or self.state["worker"] != run:
            raise RuntimeError("cleanup only accepts isolated resources created by this script")
        completed = self.state.setdefault("cleanup", [])

        def once(name, action):
            if name not in completed:
                action()
                completed.append(name)
                self.save()
                event("resource_removed", resource=name)

        def remove_rules():
            rules = self.cf("email-routing", "rules", "list-account", "--zone", self.state["domain"])
            for rule in rules:
                if any(m.get("value") in self.state["recipients"] for m in rule["matchers"]):
                    if rule["actions"] != [{"type": "worker", "value": [run]}]:
                        raise RuntimeError("a test route changed ownership; refusing to delete it")
                    self.cf("email-routing", "rules", "delete", rule["id"], "--zone", self.state["domain"], "--force")

        once("routes", remove_rules)
        if self.state.get("worker_deployed"):
            if self.state.get("quota_namespace_created"):
                once("quota_namespace", self.retire_spool)
            once("worker", lambda: self.cf("workers", "delete", run, "--force"))
        if self.state.get("sender_subdomain_id"):
            once("sender", lambda: self.cf("email-sending", "subdomains", "delete", self.state["sender_subdomain_id"],
                                            "--zone", self.state["domain"], "--force"))

        def remove_sender_dns():
            # The sending API removes its locked records but can leave the
            # generated DMARC policy. Restrict deletion to the exact DNS names
            # captured when this run created its isolated sending subdomain.
            expected = json.loads((self.root / "sender-dns.json").read_text())
            names = {r["name"] for r in expected["records"]}
            records = self.cf("dns", "records", "list", "--zone", self.state["domain"],
                              "--name-contains", self.state["sender_domain"])
            for record in records:
                if record["name"] not in names or not record["name"].endswith("." + self.state["sender_domain"]):
                    raise RuntimeError("unexpected DNS ownership; refusing to delete the record")
                self.cf("dns", "records", "delete", record["id"], "--zone", self.state["domain"], "--force")

        if self.state.get("sender_subdomain_id"):
            once("sender_dns", remove_sender_dns)

        def remove_bucket():
            for obj in self.objects(""):
                self.cf("r2", "objects", "delete", obj["key"], "--bucket-name", run, "--force")
            self.cf("r2", "buckets", "delete", run, "--force")

        if self.state.get("bucket_created"):
            once("bucket", remove_bucket)
        for kind in ["r2", "smtp"]:
            if self.state.get(f"{kind}_token_id"):
                if kind in self.state.get("deferred_token_revocations", {}) and f"{kind}_token_secret" not in self.secrets:
                    continue
                def revoke(kind=kind):
                    token_id = self.state[f"{kind}_token_id"]
                    try:
                        self.cf("user", "tokens", "delete", token_id, "--force")
                    except RuntimeError as error:
                        if "403 Forbidden" not in str(error):
                            raise
                        # A restricted bootstrap credential may create scoped
                        # children without being able to manage them later.
                        # Try self-revocation without widening any permissions.
                        if f"{kind}_token_secret" not in self.secrets:
                            raise
                        self.command(["cf", "user", "tokens", "delete", token_id, "--force"],
                                     env={**self.env, "CLOUDFLARE_API_TOKEN": self.secrets[f"{kind}_token_secret"]})
                try:
                    once(f"{kind}_token", revoke)
                except RuntimeError as error:
                    if "403 Forbidden" not in str(error):
                        raise
                    expiry = self.state[f"{kind}_expires_rfc3339"]
                    expiry_time = dt.datetime.strptime(expiry, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)
                    if expiry_time > dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=2):
                        raise RuntimeError("refusing to defer revocation for a token beyond the test's two-hour lifetime") from error
                    self.state.setdefault("deferred_token_revocations", {})[kind] = expiry
                    self.save()
                    event("token_revocation_denied", scope=kind, expires_rfc3339=expiry,
                          reason="bootstrap and self credentials lack token management permission")
        self.secrets_path.unlink(missing_ok=True)
        self.secrets.clear()
        self.verify_cleanup()
        event("cleanup_complete", evidence_dir=str(self.root),
              deferred_token_revocations=self.state.get("deferred_token_revocations", {}))

    def verify_cleanup(self):
        buckets = self.cf("r2", "buckets", "list")["buckets"]
        if any(b["name"] == self.state["bucket"] for b in buckets):
            raise RuntimeError("test bucket still exists after cleanup")
        rules = self.cf("email-routing", "rules", "list-account", "--zone", self.state["domain"])
        if any(m.get("value") in self.state["recipients"] for r in rules for m in r["matchers"]):
            raise RuntimeError("test email routes still exist after cleanup")
        senders = self.cf("email-sending", "subdomains", "list", "--zone", self.state["domain"])
        if any(s.get("name") == self.state["sender_domain"] for s in senders):
            raise RuntimeError("test sending domain still exists after cleanup")
        workers = self.cf("workers", "list")
        if any(w.get("name") == self.state["worker"] for w in workers):
            raise RuntimeError("test Worker still exists after cleanup")
        dns = self.cf("dns", "records", "list", "--zone", self.state["domain"],
                      "--name-contains", self.state["sender_domain"])
        if dns:
            raise RuntimeError("test sending DNS records still exist after cleanup")
        if self.secrets_path.exists():
            raise RuntimeError("local test credential file still exists")
        self.state["cleanup_resources_verified"] = True
        self.save()
        event("cleanup_verified", test_resources_absent=True, credential_file_deleted=True,
              remaining_email_route_count=len(rules))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["prepare-storage", "prepare-sender", "inspect", "deliver", "observe", "consume", "smtp-preview", "smtp-send", "smtp-observe", "api-preview", "api-send", "api-observe", "verify-duplicate-hint", "cleanup", "verify-cleanup"])
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--account-id")
    parser.add_argument("--domain")
    parser.add_argument("--spool-control-url", help="Override the deployed Worker HTTPS URL for quota-aware acknowledgements")
    parser.add_argument("--confirm-plan-id", help="Explicit authorization of the exact push plan; smtp-send or api-send only")
    parser.add_argument("--discard-unsent", action="store_true", help="Clean up test resources even when a sending test is pending")
    args = parser.parse_args()
    os.umask(0o077)
    try:
        test = LiveTest(args)
        getattr(test, args.mode.replace("-", "_"))()
    except (RuntimeError, KeyError, ValueError, OSError) as error:
        event("failed", message=str(error))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
