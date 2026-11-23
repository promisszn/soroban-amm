#!/usr/bin/env bash
#
# Fail if a storage key in the oracle consumers is derived from the ledger
# timestamp or sequence (issue #985).
#
# A Soroban transaction's storage footprint is fixed when it is simulated,
# against the latest closed ledger. It applies in a later ledger, with a later
# timestamp and sequence, so a key computed from either at apply time is not
# the key simulation put in the footprint, and the host traps. Unit tests
# cannot catch this: `Env::default()` does not enforce footprints.
#
# Two grep-level checks per contract:
#   1. No `DataKey` variant carries an integer field. The keys that broke
#      were `Snapshot(Address, u64)` and `LiquiditySnapshot(Address, u64)`;
#      an integer in a key is how a timestamp or sequence gets in. If a
#      future key genuinely needs an integer that does not come from the
#      ledger, extend ALLOWED_INT_KEYS below deliberately.
#   2. No `DataKey::...(...)` construction mentions the ledger clock
#      (`ledger()`, `timestamp`, `sequence`, or a `*_ts` / `now` variable).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

CONTRACTS=(
  contracts/twap_consumer/src/lib.rs
  contracts/twal_consumer/src/lib.rs
)
# Variant names allowed to carry an integer field. Empty today.
ALLOWED_INT_KEYS=()

status=0
for rel in "${CONTRACTS[@]}"; do
  file="$ROOT_DIR/$rel"
  if [[ ! -f "$file" ]]; then
    echo "check_storage_keys: missing $rel" >&2
    status=1
    continue
  fi

  # 1. Integer fields in the DataKey enum (comments stripped).
  enum_body=$(awk '/^pub enum DataKey[[:space:]]*\{/{inside=1; next} inside && /^\}/{exit} inside' "$file" \
    | sed 's://.*$::')
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    variant=$(sed -E 's/^[[:space:]]*([A-Za-z0-9_]+).*/\1/' <<<"$line")
    allowed=false
    for ok in "${ALLOWED_INT_KEYS[@]:-}"; do
      [[ -n "$ok" && "$variant" == "$ok" ]] && allowed=true
    done
    if [[ "$allowed" == false ]]; then
      echo "$rel: DataKey::$variant has an integer field: ${line#"${line%%[![:space:]]*}"}" >&2
      status=1
    fi
  done < <(grep -E '\((.*[^A-Za-z0-9_])?(u8|u16|u32|u64|u128|i8|i16|i32|i64|i128|usize|isize)([^A-Za-z0-9_].*)?\)' <<<"$enum_body" || true)

  # 2. DataKey constructions whose arguments mention the ledger clock.
  hits=$(perl -0777 -ne '
    s{//[^\n]*}{}g;
    while (/DataKey::(\w+)(\((?:[^()]++|(?2))*\))/sg) {
      my ($v, $args) = ($1, substr($2, 1, -1));
      if ($args =~ /ledger\s*\(|timestamp|sequence|\b\w*_ts\b|\bnow\b/) {
        $args =~ s/\s+/ /g;
        print "DataKey::$v($args)\n";
      }
    }' "$file")
  if [[ -n "$hits" ]]; then
    while IFS= read -r hit; do
      echo "$rel: storage key built from the ledger clock: $hit" >&2
    done <<<"$hits"
    status=1
  fi
done

if [[ $status -ne 0 ]]; then
  echo "check_storage_keys: FAILED — storage keys must not depend on the ledger timestamp or sequence (see issue #985)" >&2
  exit 1
fi
echo "check_storage_keys: OK (${#CONTRACTS[@]} contracts)"
