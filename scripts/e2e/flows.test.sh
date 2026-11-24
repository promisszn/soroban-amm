#!/usr/bin/env bash
# flows.test.sh — offline guard for scripts/e2e/run.sh.
#
# A flow named in SHARED_FLOWS/SELF_CONTAINED_FLOWS is advertised by --help and
# appears in the final summary. If it is never sourced, or never passed to
# run_flow_isolated, it reports "skipped" forever while the suite still exits 0
# — the suite cannot catch a regression in it, and a reader is told the flow is
# covered (issue #1345). This test fails when the declared flows and the run
# wiring disagree.
set -Eeuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# An explicit path lets the guard test itself be exercised against a deliberately
# broken copy of run.sh; CI and normal use check the real orchestrator.
RUN="${1:-$HERE/run.sh}"
FAILED=0

if [[ ! -f "$RUN" ]]; then
  echo "FAIL: missing $RUN" >&2
  exit 1
fi

# array_entries VAR — print each name declared in `VAR=(a b c)` on its own line.
array_entries() {
  local var="$1" line
  line=$(grep -E "^${var}=\(" "$RUN" | head -n 1)
  if [[ -z "$line" ]]; then
    echo "FAIL: $RUN declares no $var array" >&2
    return 1
  fi
  line="${line#*=\(}"
  line="${line%\)}"
  # Word splitting is intended: the array body is a space-separated list.
  # shellcheck disable=SC2086
  printf '%s\n' $line
}

flows="$(array_entries SHARED_FLOWS; array_entries SELF_CONTAINED_FLOWS)" || exit 1
if [[ -z "$flows" ]]; then
  echo "FAIL: $RUN declares no flows" >&2
  exit 1
fi

help_output="$(bash "$RUN" --help)"

count=0
while IFS= read -r flow; do
  [[ -z "$flow" ]] && continue
  count=$((count + 1))

  if ! grep -qF "e2e/${flow}.sh\"" "$RUN"; then
    echo "FAIL: flow '$flow' is declared in run.sh but e2e/${flow}.sh is never sourced" >&2
    FAILED=1
  fi
  if ! grep -Eq "^run_flow_isolated[[:space:]]+${flow}[[:space:]]+" "$RUN"; then
    echo "FAIL: flow '$flow' is declared in run.sh but run_flow_isolated never runs it" >&2
    FAILED=1
  fi
  if [[ "$help_output" != *"$flow"* ]]; then
    echo "FAIL: flow '$flow' is declared in run.sh but missing from --help" >&2
    FAILED=1
  fi
done <<<"$flows"

if [[ "$FAILED" -ne 0 ]]; then
  echo "FAIL: scripts/e2e/run.sh advertises flows it does not run" >&2
  exit 1
fi

echo "ok   - run.sh sources and runs all $count declared flows"
