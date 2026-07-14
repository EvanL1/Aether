#!/usr/bin/env bash
# shellcheck disable=SC2016 # GitHub expressions and commands are asserted literally.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT_DIR
readonly AGENT_INSTRUCTIONS="$ROOT_DIR/AGENTS.md"
readonly PULL_REQUEST_TEMPLATE="$ROOT_DIR/.github/PULL_REQUEST_TEMPLATE.md"
readonly CODE_CHECK_WORKFLOW="$ROOT_DIR/.github/workflows/rust-check.yml"
readonly TOPOLOGY_SOAK_WORKFLOW="$ROOT_DIR/.github/workflows/topology-soak.yml"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_contains() {
    local file=$1
    local expected=$2

    rg --fixed-strings --quiet -- "$expected" "$file" \
        || fail "$file is missing required CI policy: $expected"
}

assert_not_contains() {
    local file=$1
    local forbidden=$2

    if rg --fixed-strings --quiet -- "$forbidden" "$file"; then
        fail "$file retains obsolete CI policy: $forbidden"
    fi
}

echo "Checking local verification is risk-proportional..."
assert_contains "$AGENT_INSTRUCTIONS" \
    'Full-workspace verification is owned by pull-request CI.'
assert_contains "$AGENT_INSTRUCTIONS" \
    'workspace suite locally by default.'
assert_contains "$AGENT_INSTRUCTIONS" \
    'CI runs; retrieve detailed logs only for failures'

echo "Checking the pull-request template asks for focused local evidence..."
assert_contains "$PULL_REQUEST_TEMPLATE" 'Focused affected check(s)'
assert_contains "$PULL_REQUEST_TEMPLATE" 'Full workspace verification is provided by PR CI.'
assert_not_contains "$PULL_REQUEST_TEMPLATE" '- [ ] Full workspace Clippy check'
assert_not_contains "$PULL_REQUEST_TEMPLATE" '- [ ] `cargo test --workspace --lib --bins`'

echo "Checking Code Check is authoritative and avoids duplicate branch runs..."
[[ "$(rg --fixed-strings --count-matches 'branches: [main, develop]' "$CODE_CHECK_WORKFLOW")" == 2 ]] \
    || fail "Code Check must run on main/develop pushes and PRs targeting main/develop"
assert_not_contains "$CODE_CHECK_WORKFLOW" 'feature/*'
assert_contains "$CODE_CHECK_WORKFLOW" 'cancel-in-progress: ${{ github.event_name == '\''pull_request'\'' }}'
assert_contains "$CODE_CHECK_WORKFLOW" './scripts/test-ci-verification-policy.sh'
assert_contains "$CODE_CHECK_WORKFLOW" 'cargo fmt --all -- --check'
assert_contains "$CODE_CHECK_WORKFLOW" './scripts/check-architecture.sh'
assert_contains "$CODE_CHECK_WORKFLOW" \
    'cargo clippy --workspace --all-targets --all-features -- -D warnings'
assert_contains "$CODE_CHECK_WORKFLOW" 'cargo nextest run --workspace --lib --bins'
ruby -ryaml -e '
    jobs = YAML.load_file(ARGV.fetch(0)).fetch("jobs")
    %w[unit-tests coverage-report config-validation].each do |job|
      abort "#{job} must run directly after quality-check" unless jobs.fetch(job).fetch("needs") == "quality-check"
    end
  ' "$CODE_CHECK_WORKFLOW" \
    || fail "independent Code Check jobs must run in parallel after Quality Check"

echo "Checking topology soak is path-scoped but remains scheduled and dispatchable..."
assert_contains "$TOPOLOGY_SOAK_WORKFLOW" 'workflow_dispatch:'
assert_contains "$TOPOLOGY_SOAK_WORKFLOW" 'schedule:'
assert_contains "$TOPOLOGY_SOAK_WORKFLOW" 'cancel-in-progress: ${{ github.event_name == '\''pull_request'\'' }}'
for required_path in \
    'crates/aether-dataplane/**' \
    'crates/aether-ports/**' \
    'extensions/shm-bridge/**' \
    'libs/aether-shm/**' \
    'services/history/**' \
    'services/io/**' \
    'services/uplink/**'; do
    assert_contains "$TOPOLOGY_SOAK_WORKFLOW" "$required_path"
done

echo "CI verification policy tests passed."
