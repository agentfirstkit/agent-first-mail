#!/usr/bin/env python3
"""A stale fixture, mixed toolchain, or unusable upstream must fail the gate."""
from pathlib import Path
import shutil
import tempfile
import unittest

import container_versions as versions


class ContainerVersions(unittest.TestCase):
    def test_local_fixture_versions_are_consistent(self):
        self.assertEqual(set(versions.local_versions()), {"GreenMail", "Dovecot", "MinIO", "Go"})

    def test_a_partial_greenmail_or_toolchain_upgrade_is_refused(self):
        for name, old, new in (
            ("scripts/demo-greenmail-mail-batch-workflow.sh", "greenmail/standalone:", "greenmail/standalone:99."),
            ("scripts/test.sh", "GOTOOLCHAIN=go", "GOTOOLCHAIN=go99."),
        ):
            with self.subTest(file=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                for source in ("tests/container_e2e.rs", "scripts/test.sh",
                               "scripts/demo-greenmail-mail-batch-workflow.sh", ".github/workflows/ci.yml"):
                    target = root / source
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(versions.ROOT / source, target)
                target = root / name
                target.write_text(target.read_text().replace(old, new, 1))
                with self.assertRaises(ValueError):
                    versions.local_versions(root)

    def test_missing_or_new_upstream_tags_are_not_current(self):
        for tag in (None, "release-99.0.0", "2.1.15"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                versions.check_tag("GreenMail", "2.1.15", tag, "release-")


if __name__ == "__main__":
    unittest.main()
