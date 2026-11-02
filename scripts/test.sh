#!/bin/bash
# shellcheck source-path=SCRIPTDIR
set -euo pipefail
export PYTHONDONTWRITEBYTECODE=1

ROOTPATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOTPATH"

MODE="${1:-all}"
FILTER="${2:-}"

check_cli_docs() {
  local binary="$ROOTPATH/target/debug/afmail"
  local doc_path="$ROOTPATH/docs/cli.md"
  local generated existing

  cargo build --bin afmail --quiet
  # The registry renders the whole document, title and generated-by marker
  # included, so there is nothing here to wrap around it.
  generated=$("$binary" --docs)
  generated="${generated%$'\n'}"
  existing=$(<"$doc_path")
  if [ "$generated" != "$existing" ]; then
    echo "docs/cli.md is out of date; regenerate it with scripts/projects.sh docs agent-first-mail in the monorepo" >&2
    return 1
  fi
}

run_clippy() {
  cargo clippy --all-targets "$@" -- -D warnings
}

check_minimal_dependency_tree() {
  local dependency_tree dependency
  dependency_tree=$(cargo tree --no-default-features --edges normal --prefix none)
  for dependency in axum tokio ureq rusty-s3; do
    if [[ "$dependency_tree" == *"$dependency v"* ]]; then
      echo "$dependency must not be present in the no-default-features dependency tree" >&2
      return 1
    fi
  done
  dependency_tree=$(cargo tree --no-default-features --edges features --prefix none)
  if [[ "$dependency_tree" == *'lettre feature "smtp-transport"'* ]]; then
    echo "SMTP transport must not be present without the smtp feature" >&2
    return 1
  fi
}

run_static() {
  # Parsing is not linting, and this ran neither until now. The pinned version
  # lives in the script itself, mirrored from the monorepo and compared byte for
  # byte there, so this spore and its siblings cannot drift onto different rules.
  "$ROOTPATH/scripts/install-shellcheck.sh" --check "$ROOTPATH"
  python3 "$ROOTPATH/scripts/test-cloudflare-live.py" --help >/dev/null
  python3 "$ROOTPATH/tests/cloudflare_live_contract.py"
  python3 "$ROOTPATH/tests/container_versions_contract.py"
  cargo fmt --all --check
  run_clippy
  run_clippy --no-default-features
  run_clippy --no-default-features --features api
  run_clippy --no-default-features --features demo
  run_clippy --no-default-features --features review
  run_clippy --no-default-features --features cloudflare
  run_clippy --no-default-features --features files
  run_clippy --no-default-features --features smtp
  check_minimal_dependency_tree
  check_cli_docs
  python3 -m json.tool spore.core.json >/dev/null
}

run_unit() {
  # Tests are isolated (unique temp workspace per test via temp_root; mock
  # servers bind 127.0.0.1:0). Run them in parallel — the suite is I/O-wait
  # bound (fork+fsync), so parallelism overlaps the waits.
  cargo test --all-features --lib --bins --tests
}

run_minimal() {
  cargo test --no-default-features --lib --bins --tests
}

run_target() {
  local target="$1"
  if [[ -n "$FILTER" ]]; then
    cargo test --test "$target" "$FILTER"
  else
    cargo test --test "$target"
  fi
}

run_worker() (
  command -v node >/dev/null 2>&1 || { echo "Node.js is required for the Worker gate" >&2; exit 1; }
  command -v npm >/dev/null 2>&1 || { echo "npm is required for the Worker gate" >&2; exit 1; }
  worker_check_dir=$(mktemp -d "${TMPDIR:-/tmp}/afmail-worker.XXXXXX")
  trap 'rm -rf "$worker_check_dir"' EXIT
  cargo build --bin afmail --quiet
  "$ROOTPATH/target/debug/afmail" cloudflare scaffold "$worker_check_dir" >/dev/null
  cd "$worker_check_dir"
  npm ci --no-audit --no-fund
  npm run check
  npm test
  npm run dry-run:wrangler
  npm run dry-run:cf
  python3 "$ROOTPATH/tests/cloudflare_live_contract.py" --retirement-dir "$worker_check_dir"
  npm run dry-run:cf
)

run_render_perf() {
  cargo test --lib render_performance_synthetic -- --ignored --nocapture --test-threads=1
}

select_test_container_runtime() {
  # shellcheck source=container-runtime.sh
  source "$ROOTPATH/scripts/container-runtime.sh"
  af_container_resolve "${AFMAIL_TEST_CONTAINER_RUNTIME:-}"
  af_container_start >/dev/null
  export AFMAIL_TEST_CONTAINER_RUNTIME="$CONTAINER_RUNTIME"
}

prepare_minio_image() (
  # Upstream's final release directs container users to build from source:
  # https://github.com/minio/minio/releases/tag/RELEASE.2025-10-15T17-29-55Z
  # Keep the real S3 gate reproducible without a removed public MinIO image.
  local runtime="$AFMAIL_TEST_CONTAINER_RUNTIME"
  if [[ "$runtime" == "apple" ]]; then runtime=container; fi
  if "$runtime" image inspect "$AFMAIL_TEST_MINIO_IMAGE" >/dev/null 2>&1; then return; fi
  command -v go >/dev/null 2>&1 || { echo "Go is required to build the MinIO e2e image" >&2; exit 1; }
  local source_dir build_dir minio_arch
  case "$(uname -m)" in
    arm64|aarch64) minio_arch=arm64 ;;
    x86_64|amd64) minio_arch=amd64 ;;
    *) echo "Unsupported MinIO e2e architecture" >&2; exit 1 ;;
  esac
  build_dir=$(mktemp -d "${TMPDIR:-/tmp}/afmail-minio.XXXXXX")
  trap 'rm -rf "$build_dir"' EXIT
  source_dir=$(GOTOOLCHAIN=go1.27.3 go mod download -json github.com/minio/minio@RELEASE.2025-10-15T17-29-55Z | afdata value - Dir)
  cd "$source_dir"
  CGO_ENABLED=0 GOOS=linux GOARCH="$minio_arch" GOTOOLCHAIN=go1.27.3 go build -mod=readonly -o "$build_dir/minio" .
  "$runtime" build -t "$AFMAIL_TEST_MINIO_IMAGE" -f "$ROOTPATH/tests/fixtures/minio/Dockerfile" "$build_dir"
)

run_e2e() {
  select_test_container_runtime
  export AFMAIL_TEST_MINIO_IMAGE=afmail-test-minio:2025-10-15-go1.27.3
  if [[ -z "$FILTER" || "$FILTER" == *minio* ]]; then prepare_minio_image; fi
  if [[ -n "$FILTER" ]]; then
    cargo test --test container_e2e "$FILTER" -- --ignored --nocapture --test-threads=1
  else
    cargo test --test container_e2e -- --ignored --nocapture --test-threads=1
  fi
}

case "$MODE" in
  check-target)
    if [[ -z "$FILTER" ]]; then
      echo "check-target requires a Rust target triple" >&2
      exit 2
    fi
    cargo check --all-targets --target "$FILTER"
    ;;
  runtime)
    cargo fmt --all --check
    cargo clippy --all-targets -- -D warnings
    cargo test --test container_e2e runtime_selection
    ;;
  worker)        run_worker ;;
  static)        run_static ;;
  unit)          run_unit ;;
  lib)           cargo test --all-features --lib "${FILTER:-}" ;;
  minimal)       run_minimal ;;
  cli)           run_target cli_integration ;;
  request-id)    run_target request_id_field_edits ;;
  workspace)     run_target workspace_identity ;;
  config)        run_target config_contract ;;
  surface)       run_target cli_surface_contract ;;
  draft)         run_target draft_contract ;;
  push)          run_target push_contract ;;
  pull)          run_target pull_contract ;;
  consumer)      run_target cloudflare_consumer_contract ;;
  files)         run_target file_channel_contract ;;
  notification)  run_target notification_contract ;;
  case)          run_target case_contract ;;
  contact)       run_target contact_contract ;;
  message)       run_target message_contract ;;
  notes)         run_target notes_contract ;;
  render)        run_target render_contract ;;
  file-descriptors) run_target file_descriptor_contract ;;
  render-perf)   run_render_perf ;;
  e2e)           run_e2e ;;
  greenmail)          "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" run ;;
  greenmail-prepare)  "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" prepare ;;
  greenmail-review)   "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" review ;;
  all)           run_static; run_unit; run_minimal; run_worker ;;
  # What CI runs, and therefore what a release must survive: `all` plus the
  # container e2e that `all` omits because it needs a container runtime. A check that can
  # fail the release has to be able to fail this gate first.
  release)       run_static; run_unit; run_minimal; run_worker; run_e2e ;;
  full)          run_static; run_unit; run_minimal; run_worker; run_e2e; "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" run ;;
  *)
    echo "Usage: $0 [static|unit|runtime|check-target TARGET|lib [FILTER]|minimal|worker|cli [FILTER]|request-id [FILTER]|workspace [FILTER]|config [FILTER]|surface [FILTER]|draft [FILTER]|push [FILTER]|pull [FILTER]|consumer [FILTER]|files [FILTER]|notification [FILTER]|case [FILTER]|contact [FILTER]|message [FILTER]|notes [FILTER]|render [FILTER]|file-descriptors [FILTER]|render-perf|e2e [FILTER]|greenmail|greenmail-prepare|greenmail-review|all|release|full]" >&2
    exit 2
    ;;
esac
