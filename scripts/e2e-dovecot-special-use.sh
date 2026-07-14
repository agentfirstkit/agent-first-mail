#!/usr/bin/env bash
set -euo pipefail

# Runs the Dovecot RFC 6154 special-use container e2e test.
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
cd "$CRATE_DIR"

exec cargo test \
  docker_dovecot_special_use_fixture_batch_e2e \
  -- --ignored --nocapture
