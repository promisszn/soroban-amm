#!/bin/sh
# docker_fetch_deps.sh — run `cargo fetch` from manifests alone.
#
# The Dockerfile copies only Cargo.toml/Cargo.lock files before this step so
# the dependency layer stays cached across source-only edits. Cargo refuses to
# load a manifest whose targets have no source file ("can't find library"),
# so create an empty placeholder for each missing target — `src/lib.rs` plus
# any explicit `path = "….rs"` — fetch, then delete the placeholders again so
# none of them can outlive this layer and shadow the real tree.
set -eu

members="$(sed -n '/^members = \[/,/^\]/p' Cargo.toml | grep -o '"[^"]*"' | tr -d '"')"

stubs=""
for m in $members; do
  paths="$(grep -o 'path *= *"[^"]*\.rs"' "$m/Cargo.toml" | sed 's/.*"\(.*\)"/\1/' || true)"
  for rel in src/lib.rs $paths; do
    f="$m/$rel"
    [ -e "$f" ] && continue
    mkdir -p "$(dirname "$f")"
    : >"$f"
    stubs="$stubs $f"
  done
done

cargo fetch

# shellcheck disable=SC2086 # word-splitting the list is intended
rm -f $stubs
