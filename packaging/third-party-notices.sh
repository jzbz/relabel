#!/usr/bin/env bash
# Write the third-party notice file that ships with a release.
#
#   packaging/third-party-notices.sh OUTPUT [CRATE_MANIFEST]
#
# OUTPUT is the file to write; keep it out of the working tree. It is only
# written once every check below has passed, so a failed run never leaves a
# plausible-looking notice file behind. CRATE_MANIFEST, when given, limits the
# notices to that crate's graph - the shipped binary's - rather than the whole
# workspace.
#
# Needs cargo-about 0.9.2, whose command-line tool has been behind a feature
# since 0.9.0:
#
#   cargo install --locked cargo-about@0.9.2 --features cli
#
# or CARGO_ABOUT set to such a binary. Needs the network too: two crates in
# about.toml have their licence files fetched from upstream.
#
# cargo-about's own --fail is not enough. It exits 0 when a pinned
# clarification no longer matches - btleplug then silently loses both its BSD
# notices - when an upstream fetch fails, and when it falls back to a generic
# licence text with no copyright line. This script fails on all three. The one
# accepted fallback is the five macOS objc2 crates, which publish no licence
# text anywhere, only a note that they are MIT.
set -euo pipefail

usage="usage: packaging/third-party-notices.sh OUTPUT [CRATE_MANIFEST]"
root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?$usage}
about=${CARGO_ABOUT:-cargo-about}
want=0.9.2

have=$("$about" --version | awk '{print $2}')
if [ "$have" != "$want" ]; then
    echo "cargo-about $want is required; found ${have:-none}" >&2
    exit 1
fi

if [ $# -ge 2 ]; then
    scope=(--manifest-path "$2")
else
    scope=(--workspace --manifest-path "$root/Cargo.toml")
fi

raw=$(mktemp)
log=$(mktemp)
draft=$(mktemp)
trap 'rm -f "$raw" "$log" "$draft"' EXIT

# -L is an option of cargo-about itself, so it comes before the subcommand;
# after it, it is an argument error. Debug level, because that is the only
# level at which a fallback to a generic licence text is logged.
status=0
"$about" -L debug generate --locked --fail "${scope[@]}" \
    -c "$root/about.toml" -o "$draft" "$root/about.hbs" 2>"$raw" || status=$?

# Each log line is a timestamp and a level coloured with escape codes, whether
# or not stderr is a terminal. Strip the codes before reading any of it.
sed 's/\x1b\[[0-9;]*m//g' "$raw" >"$log"

if [ "$status" -ne 0 ]; then
    grep -v '\[DEBUG\]' "$log" | tail -n 40 >&2
    echo "cargo-about failed; nothing was written to $out" >&2
    exit 1
fi

# Warnings and errors, less the one a single-crate run always prints for the
# clarifications that belong to other crates.
problems=$(grep -E '\[(WARN|ERROR)\]|^(warning|error):' "$log" |
    grep -v 'config found for a crate not present in the graph' || true)
# Generic texts standing in for a crate's own, less the accepted five.
fallbacks=$(grep 'falling back to canonical text' "$log" |
    grep -v -E "for crate '(block2|objc2|objc2-encode|objc2-foundation|objc2-core-bluetooth) " || true)

if [ -n "$problems$fallbacks" ]; then
    printf '%s\n' "$problems" "$fallbacks" | sed '/^$/d' >&2
    echo "the notices are incomplete; nothing was written to $out" >&2
    exit 1
fi
# mktemp makes files readable by their owner alone; this one ships.
chmod 644 "$draft"
mv "$draft" "$out"
echo "wrote $out"
