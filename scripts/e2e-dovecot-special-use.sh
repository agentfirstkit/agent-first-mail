#!/usr/bin/env bash
set -euo pipefail

# Runs the Dovecot RFC 6154 special-use container e2e test through the fixed
# runtime-aware gate.
SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
exec "$CRATE_DIR/scripts/test.sh" \
  e2e \
  container_dovecot_special_use_fixture_batch_e2e
