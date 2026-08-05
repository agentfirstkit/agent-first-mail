#!/bin/bash
set -euo pipefail

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
  for dependency in axum tokio; do
    if [[ "$dependency_tree" == *"$dependency v"* ]]; then
      echo "$dependency must not be present in the no-default-features dependency tree" >&2
      return 1
    fi
  done
}

run_static() {
  cargo fmt --all --check
  run_clippy
  run_clippy --no-default-features
  run_clippy --no-default-features --features api
  run_clippy --no-default-features --features demo
  run_clippy --no-default-features --features review
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

run_render_perf() {
  cargo test --lib render_performance_synthetic -- --ignored --nocapture --test-threads=1
}

run_e2e() {
  docker info >/dev/null
  if [[ -n "$FILTER" ]]; then
    cargo test --test container_e2e "$FILTER" -- --ignored --nocapture --test-threads=1
  else
    cargo test --test container_e2e -- --ignored --nocapture --test-threads=1
  fi
}

case "$MODE" in
  static)        run_static ;;
  unit)          run_unit ;;
  minimal)       run_minimal ;;
  cli)           run_target cli_integration ;;
  request-id)    run_target request_id_field_edits ;;
  workspace)     run_target workspace_identity ;;
  config)        run_target config_contract ;;
  surface)       run_target cli_surface_contract ;;
  draft)         run_target draft_contract ;;
  push)          run_target push_contract ;;
  pull)          run_target pull_contract ;;
  notification)  run_target notification_contract ;;
  case)          run_target case_contract ;;
  contact)       run_target contact_contract ;;
  message)       run_target message_contract ;;
  notes)         run_target notes_contract ;;
  render)        run_target render_contract ;;
  render-perf)   run_render_perf ;;
  e2e)           run_e2e ;;
  greenmail)          "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" run ;;
  greenmail-prepare)  "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" prepare ;;
  greenmail-review)   "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" review ;;
  all)           run_static; run_unit; run_minimal ;;
  # What CI runs, and therefore what a release must survive: `all` plus the
  # container e2e that `all` omits because it needs Docker. A check that can
  # fail the release has to be able to fail this gate first.
  release)       run_static; run_unit; run_minimal; run_e2e ;;
  full)          run_static; run_unit; run_minimal; run_e2e; "$ROOTPATH/scripts/demo-greenmail-mail-batch-workflow.sh" run ;;
  *)
    echo "Usage: $0 [static|unit|minimal|cli [FILTER]|request-id [FILTER]|workspace [FILTER]|config [FILTER]|surface [FILTER]|draft [FILTER]|push [FILTER]|pull [FILTER]|notification [FILTER]|case [FILTER]|contact [FILTER]|message [FILTER]|notes [FILTER]|render [FILTER]|render-perf|e2e [FILTER]|greenmail|greenmail-prepare|greenmail-review|all|release|full]" >&2
    exit 2
    ;;
esac
