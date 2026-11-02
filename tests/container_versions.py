#!/usr/bin/env python3
"""Check all mail fixture versions, including the MinIO build toolchain."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
UPSTREAMS = {
    "GreenMail": ("https://api.github.com/repos/greenmail-mail-test/greenmail/releases/latest", "release-"),
    "Dovecot": ("https://api.github.com/repos/dovecot/core/releases/latest", ""),
    "MinIO": ("https://api.github.com/repos/minio/minio/releases/latest", ""),
    "Go": ("https://go.dev/dl/?mode=json", "go"),
}


def pin(source, pattern, label):
    values = set(re.findall(pattern, source))
    if len(values) != 1:
        raise ValueError(f"missing or inconsistent {label} pin")
    return values.pop()


def local_versions(root=ROOT):
    tests = (root / "tests/container_e2e.rs").read_text()
    greenmail = pin(tests, r'greenmail/standalone:([0-9.]+)', "GreenMail")
    for name in ("scripts/demo-greenmail-mail-batch-workflow.sh", ".github/workflows/ci.yml"):
        if name.endswith(".yml"):
            event = json.loads(subprocess.check_output(
                [os.environ.get("AFDATA_BIN", "afdata"), "get", str(root / name), "jobs"], text=True))
            source = "\n".join(step["run"] for job in event["result"]["value"].values()
                               for step in job.get("steps", []) if "run" in step)
        else:
            source = (root / name).read_text()
        if pin(source, r'greenmail/standalone:([0-9.]+)', "GreenMail") != greenmail:
            raise ValueError("GreenMail demo, CI, and e2e versions differ")
    driver = (root / "scripts/test.sh").read_text()
    go = pin(driver, r'GOTOOLCHAIN=go([0-9.]+)', "Go")
    minio = pin(driver, r'github.com/minio/minio@(RELEASE\.[0-9TZ.-]+)', "MinIO")
    expected_image = f"afmail-test-minio:{minio[8:18]}-go{go}"
    if f"AFMAIL_TEST_MINIO_IMAGE={expected_image}\n" not in driver:
        raise ValueError("MinIO image cache tag must include its source date and Go version")
    return {"GreenMail": greenmail, "Go": go, "MinIO": minio,
            "Dovecot": pin(tests, r'dovecot/dovecot:([0-9.]+)', "Dovecot")}


def check_tag(name, current, tag, prefix):
    if not isinstance(tag, str) or not tag.startswith(prefix) or tag[len(prefix):] != current:
        raise ValueError(f"{name} pin {current} differs from upstream {tag}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--latest", action="store_true")
    options = parser.parse_args()
    versions = local_versions()
    for name, current in versions.items():
        if options.latest:
            url, prefix = UPSTREAMS[name]
            request = urllib.request.Request(url, headers={"User-Agent": "afmail-pin-check"})
            with urllib.request.urlopen(request, timeout=30) as response:
                release = json.load(response)
            tag = release[0]["version"] if name == "Go" else release["tag_name"]
            check_tag(name, current, tag, prefix)
        print(f"{name}: {current}")


if __name__ == "__main__":
    main()
