#!/bin/bash
set -euo pipefail
_AFDATA_BASH_SOURCE="$("${AFDATA_BIN:-afdata}" shell bash)"
# shellcheck source=/dev/stdin
source /dev/stdin <<<"$_AFDATA_BASH_SOURCE"
unset _AFDATA_BASH_SOURCE
ROOT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
afdata_run --quiet python3 "$ROOT_PATH/tests/container_versions.py" --latest
afdata_result "Mail fixture images and build toolchain are current"
