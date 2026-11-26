#!/usr/bin/env bash
# run.test.sh — offline tests for the --only/--skip validation in run.sh.
#
# Covers: unknown token (non-zero exit, no side-effects), empty selection
# (non-zero exit), valid --only, valid --skip, and the invariant that
# "E2E PASS" is never printed unless at least one flow executed.
#
# No network: generate_and_fund_source and the flow functions are stubbed.
# Run: bash scripts/e2e/run.test.sh
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN="$HERE/run.sh"
FAILED=0

# ── Helpers ──────────────────────────────────────────────────────────────────

pass_test() { printf 'ok   - %s\n' "$*"; }
fail_test() { printf 'FAIL - %s\n' "$*" >&2; FAILED=1; }

expect_exit() {
  # expect_exit NAME WANT_RC [--stdout PATTERN] [--stderr PATTERN] -- CMD...
  local name="$1" want="$2"
  shift 2
  local check_stdout="" check_stderr=""
  while [[ $# -gt 0 && "$1" != "--" ]]; do
    case "$1" in
      --stdout) check_stdout="$2"; shift 2 ;;
      --stderr) check_stderr="$2"; shift 2 ;;
      *) break ;;
    esac
  done
  [[ "$1" == "--" ]] && shift

  local out err rc=0
  out=$(mktemp); err=$(mktemp)
  trap 'rm -f "$out" "$err"' RETURN
  "$@" >"$out" 2>"$err" || rc=$?
  if [[ "$rc" -ne "$want" ]]; then
    fail_test "$name: expected exit $want, got $rc"
    return
  fi
  if [[ -n "$check_stdout" ]] && ! grep -qF "$check_stdout" "$out"; then
    fail_test "$name: stdout missing $(printf '%q' "$check_stdout")"
    cat "$out" >&2
    return
  fi
  if [[ -n "$check_stderr" ]] && ! grep -qF "$check_stderr" "$err"; then
    fail_test "$name: stderr missing $(printf '%q' "$check_stderr")"
    cat "$err" >&2
    return
  fi
  pass_test "$name"
}

# ── Stub environment ──────────────────────────────────────────────────────────
# We build a minimal fake tree so run.sh can be exercised without a network:
#   - A fake `stellar` binary that records calls and exits 0.
#   - A fake ROOT_DIR whose e2e/ subdirectory contains stub flow scripts, so
#     the `source "$ROOT_DIR/e2e/<flow>.sh"` lines do not fail.
#   - Overrides for generate_and_fund_source and the deploy call, supplied via
#     a small shim script that is prepended to run.sh at test time.
#
# Error-path tests (unknown token, empty selection) exit before require_cmd and
# generate_and_fund_source, so they need no stubs at all — we just run run.sh
# directly.

STUB_DIR="$(mktemp -d)"
trap 'rm -rf "$STUB_DIR"' EXIT

# fake stellar binary — records calls and satisfies `stellar keys address`
mkdir -p "$STUB_DIR/bin"
cat >"$STUB_DIR/bin/stellar" <<'STUB'
#!/usr/bin/env bash
echo "stellar $*" >> "${STUB_CALL_LOG:-/dev/null}"
if [[ "${1:-}" == "keys" && "${2:-}" == "address" ]]; then
  printf 'GFAKEPUBLICKEY000000000000000000000000000000000000000000\n'
fi
exit 0
STUB
chmod +x "$STUB_DIR/bin/stellar"

# BASH_ENV file: bash sources this file before executing each script when
# BASH_ENV is set, which lets us inject function overrides without modifying
# run.sh. We override the functions that touch the network or the filesystem
# so that success-path tests run offline.
#
# run.sh sources each flow's .sh file (which redefines run_<flow>_flow), so we
# also override those functions here — BASH_ENV is sourced before run.sh runs
# but after the shell is set up, so the overrides survive the subsequent source.
# We use a post-source hook by overriding `source` itself to additionally apply
# the stub after every sourced file.
BASH_ENV_FILE="$STUB_DIR/bash_env.sh"

# Build the list of per-flow stub function bodies.
FLOW_STUBS=""
for _flow in v2 factory cl governance staking twap \
             token router dex_aggregator oracle_aggregator \
             cl_position_nft pol_vesting twap_consumer twal_consumer \
             incentive_campaigns; do
  FLOW_STUBS+="
run_${_flow}_flow() { printf '[stub] ${_flow} flow ran\n'; }"
done

cat >"$BASH_ENV_FILE" <<BASH_ENV
# Override functions that touch the network or require built artifacts.
generate_and_fund_source() { :; }
e2e_deploy()  { printf 'CFAKECONTRACTID00000000000000000000000000000000000000000\n'; }
e2e_upload()  { printf 'deadbeef00000000000000000000000000000000000000000000000000000000\n'; }
invoke()      { :; }
invoke_as()   { :; }
e2e_wasm()    { printf '/dev/null\n'; }
e2e_trap_errors() { :; }
# Stub all flow functions so the real definitions from sourced files are
# immediately replaced and do not make network calls.
${FLOW_STUBS}
# Override 'source' so that our stubs survive each flow file being sourced.
# After the real source runs (redefining the flow function), we reapply stubs.
_real_source() { builtin source "\$@"; }
source() {
  _real_source "\$@"
  ${FLOW_STUBS}
  generate_and_fund_source() { :; }
  e2e_deploy()  { printf 'CFAKECONTRACTID00000000000000000000000000000000000000000\n'; }
  e2e_upload()  { printf 'deadbeef00000000000000000000000000000000000000000000000000000000\n'; }
  invoke()      { :; }
  invoke_as()   { :; }
  e2e_wasm()    { printf '/dev/null\n'; }
  e2e_trap_errors() { :; }
}
BASH_ENV

# fake DEPLOY_ENV file so `source "$DEPLOY_ENV"` is a no-op
FAKE_DEPLOY_ENV="$STUB_DIR/deploy.env"
touch "$FAKE_DEPLOY_ENV"

# A wrapper that runs run.sh with the stub environment active.
# Tests that need to exercise the full run (past the validation block) use this.
# Success-path tests only use self-contained flows (token, router, …) so that
# needs_shared_deploy() returns false and scripts/deploy.sh is never invoked.
stub_run() {
  env \
    PATH="$STUB_DIR/bin:$PATH" \
    BASH_ENV="$BASH_ENV_FILE" \
    DEPLOY_ENV="$FAKE_DEPLOY_ENV" \
    SOURCE_ACCOUNT="e2e-test-stub" \
    STUB_CALL_LOG="$STUB_DIR/stellar_calls.log" \
    bash "$RUN" "$@"
}

# ── Error-path tests (exit before require_cmd / generate_and_fund_source) ────
# These run run.sh directly without stubs because they never reach line 80+.
# We still redirect stderr to capture the error message.

ALL_FLOWS_STR="v2 factory cl governance staking twap token router dex_aggregator oracle_aggregator cl_position_nft pol_vesting twap_consumer twal_consumer incentive_campaigns"

# 1. Unknown token in --only → exit 2, stderr contains available flow names,
#    and generate_and_fund_source is never called (verified by the absence of
#    any stellar call log entries).
CALL_LOG="$(mktemp)"
rc=0
out=$(STUB_CALL_LOG="$CALL_LOG" PATH="$STUB_DIR/bin:$PATH" bash "$RUN" --only definitely_not_a_flow 2>&1) || rc=$?
if [[ "$rc" -ne 2 ]]; then
  fail_test "unknown --only token: expected exit 2, got $rc"
elif ! grep -qF "v2" <<<"$out"; then
  fail_test "unknown --only token: stderr should list available flows"
elif [[ -s "$CALL_LOG" ]]; then
  fail_test "unknown --only token: generate_and_fund_source was called (stellar binary was invoked)"
else
  pass_test "unknown --only token exits 2, prints flows, calls no network"
fi
rm -f "$CALL_LOG"

# 2. Unknown token mixed with valid token in --only → still exit 2
expect_exit "unknown token mixed with valid in --only" 2 \
  bash "$RUN" --only "v2,definitely_not_a_flow"

# 3. Unknown token in --skip → exit 2
expect_exit "unknown --skip token exits 2" 2 \
  bash "$RUN" --skip "definitely_not_a_flow"

# 4. Empty selection: skip every declared flow → exit non-zero
expect_exit "skipping all flows exits non-zero" 2 \
  bash "$RUN" --skip "v2,factory,cl,governance,staking,twap,token,router,dex_aggregator,oracle_aggregator,cl_position_nft,pol_vesting,twap_consumer,twal_consumer,incentive_campaigns"

# 5. --help lists available flow names
expect_exit "--help exits 0" 0 \
  --stdout "v2" \
  --stdout "token" \
  -- bash "$RUN" --help

# ── Valid-selection tests (need stubs to avoid network) ───────────────────────

# 6. Valid --only: a single known flow name is accepted and runs that flow
out=$(stub_run --only token 2>&1) || { fail_test "valid --only token: unexpected non-zero exit"; }
if grep -qF "[stub] token flow ran" <<<"$out"; then
  pass_test "valid --only token: selected flow ran"
else
  fail_test "valid --only token: flow did not run (output: $out)"
fi
# Confirm an unselected flow did not run
if grep -qF "[stub] v2 flow ran" <<<"$out"; then
  fail_test "valid --only token: unselected flow (v2) should not have run"
else
  pass_test "valid --only token: unselected flow (v2) did not run"
fi

# 7. Valid --skip: skip all shared flows and one self-contained flow; confirm
#    the skipped self-contained flow did not run and another did.
#    Skipping all shared flows means needs_shared_deploy() returns false, so
#    deploy.sh is never invoked.
SKIP_SHARED="v2,factory,cl,governance,staking,twap"
out=$(stub_run --skip "${SKIP_SHARED},token" 2>&1) || { fail_test "valid --skip token: unexpected non-zero exit"; }
if grep -qF "[stub] token flow ran" <<<"$out"; then
  fail_test "valid --skip token: skipped flow should not have run"
else
  pass_test "valid --skip token: skipped flow did not run"
fi
if grep -qF "[stub] router flow ran" <<<"$out"; then
  pass_test "valid --skip token: non-skipped flow (router) ran as expected"
else
  fail_test "valid --skip token: non-skipped flow (router) did not run (output: $out)"
fi

# 8. E2E PASS invariant: a valid run that executes at least one flow must print
#    "E2E PASS"; it must NOT print "E2E PASS" if no flow ran (which is already
#    prevented by the empty-selection guard, but we confirm the positive case).
out=$(stub_run --only token 2>&1) || { fail_test "E2E PASS invariant: unexpected non-zero exit"; }
if grep -qF "E2E PASS" <<<"$out"; then
  pass_test "E2E PASS printed after at least one flow executed"
else
  fail_test "E2E PASS not printed despite successful flow run (output: $out)"
fi

# ── Result ────────────────────────────────────────────────────────────────────
exit "$FAILED"
