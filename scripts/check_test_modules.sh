#!/usr/bin/env bash
#
# Fail if a `*_test.rs` file under contracts/*/src is never compiled.
#
# Cargo only builds the files a crate reaches through `mod` declarations, so a
# `foo_test.rs` that no `mod foo_test;` names is silently skipped. A test
# module gated behind a feature nothing enables is skipped the same way. This
# check requires every `*_test.rs` under a crate's src/ to be declared with a
# `mod` line in that crate. Files under tests/ are discovered by Cargo and are
# not checked here.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
status=0

while IFS= read -r file; do
    name="$(basename "$file" .rs)"
    crate_src="${file%%/src/*}/src"
    if ! grep -rqE "^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?mod[[:space:]]+${name}[[:space:]]*[;{]" \
        --include='*.rs' "$crate_src"; then
        echo "${file#"$ROOT"/}: no \`mod ${name};\` declares it, so cargo never compiles it" >&2
        status=1
    fi
done < <(find "$ROOT"/contracts/*/src -name '*_test.rs' -type f 2>/dev/null | sort)

if [ "$status" -eq 0 ]; then
    echo "every *_test.rs file under contracts/*/src is declared as a module"
fi

# Check that no #[cfg(...)] attribute gating a `mod` declaration under
# contracts/*/src references a Cargo feature not declared in that crate's
# Cargo.toml [features] table.
while IFS= read -r cargo_toml; do
    crate_dir="$(dirname "$cargo_toml")"
    crate_src="$crate_dir/src"
    [ -d "$crate_src" ] || continue

    declared_features="$(awk '
        /^[[:space:]]*\[features\][[:space:]]*(#.*)?$/ { in_features = 1; next }
        /^[[:space:]]*\[/ { in_features = 0; next }
        in_features {
            sub(/#.*/, "")
            if (match($0, /^[[:space:]]*"?([A-Za-z0-9_-]+)"?[[:space:]]*=/)) {
                key = substr($0, RSTART, RLENGTH)
                sub(/^[[:space:]]*"?/, "", key)
                sub(/"?[[:space:]]*=$/, "", key)
                if (length(key) > 0) print key
            }
        }
    ' "$cargo_toml")"

    while IFS= read -r rs_file; do
        rel_path="${rs_file#"$ROOT"/}"
        while IFS= read -r violation; do
            [ -n "$violation" ] || continue
            echo "$violation" >&2
            status=1
        done < <(awk -v rel="$rel_path" -v feats="$declared_features" '
            BEGIN {
                n = split(feats, arr, "\n")
                for (i = 1; i <= n; i++) {
                    if (length(arr[i]) > 0) declared[arr[i]] = 1
                }
                pending_count = 0
            }
            {
                line = $0
                # Blank lines and line comments do not clear pending attributes.
                if (line ~ /^[[:space:]]*$/ || line ~ /^[[:space:]]*\/\//) {
                    next
                }
                # Attribute line: collect any feature = "..." references in #[cfg(...)].
                if (line ~ /^[[:space:]]*#\[/) {
                    if (line ~ /cfg[[:space:]]*\(/) {
                        rest = line
                        while (match(rest, /feature[[:space:]]*=[[:space:]]*"[^"]+"/)) {
                            frag = substr(rest, RSTART, RLENGTH)
                            sub(/^feature[[:space:]]*=[[:space:]]*"/, "", frag)
                            sub(/"$/, "", frag)
                            pending_count++
                            pending_feat[pending_count] = frag
                            pending_line[pending_count] = NR
                            rest = substr(rest, RSTART + RLENGTH)
                        }
                    }
                    next
                }
                # Module declaration: validate any pending feature gates.
                if (line ~ /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z0-9_]+[[:space:]]*[;{]/) {
                    mod_name = line
                    sub(/^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]+/, "", mod_name)
                    sub(/[[:space:]]*[;{].*$/, "", mod_name)
                    for (i = 1; i <= pending_count; i++) {
                        f = pending_feat[i]
                        if (!(f in declared)) {
                            printf "%s:%d: `mod %s` is gated behind undeclared Cargo feature \"%s\"\n", rel, pending_line[i], mod_name, f
                        }
                    }
                }
                pending_count = 0
            }
        ' "$rs_file")
    done < <(find "$crate_src" -name '*.rs' -type f 2>/dev/null | sort)
done < <(find "$ROOT"/contracts -maxdepth 2 -name 'Cargo.toml' -type f 2>/dev/null | sort)

if [ "$status" -eq 0 ]; then
    echo "every cfg-gated mod under contracts/*/src uses a declared Cargo feature"
fi
exit "$status"
