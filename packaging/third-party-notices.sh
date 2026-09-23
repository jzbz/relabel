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
# Two licence texts are appended by this script rather than by cargo-about: the
# embedded fonts' OFL, because fonts are not crates, and Slint's royalty-free
# licence, because a LicenseRef has no canonical text and cargo-about carries
# none. Both are checked before they are written.
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
# Slint's royalty-free licence is not an SPDX identifier, so cargo-about
# resolves the crates to it and then has no text to print. That text is
# appended below from packaging/licenses, so the warning is expected.
problems=$(grep -E '\[(WARN|ERROR)\]|^(warning|error):' "$log" |
    grep -v 'config found for a crate not present in the graph' |
    grep -v 'LicenseRef-Slint-Royalty-free-2.0 has no license file' || true)
# Crates that publish no licence text at all, so the canonical text is the only
# thing there is to print. Each was checked by looking inside the published
# crate: nothing named LICENSE, LICENCE or COPYING. Anything that does ship its
# text belongs in about.toml's clarify section instead, where the copyright
# line survives.
no_text='block2|objc2|objc2-encode|objc2-foundation|objc2-core-bluetooth'
no_text="$no_text|objc-sys|objc2-app-kit|objc2-cloud-kit|objc2-core-data"
no_text="$no_text|objc2-core-foundation|objc2-core-graphics|objc2-core-image"
no_text="$no_text|objc2-core-text|objc2-core-video|objc2-quartz-core|dispatch"
no_text="$no_text|dispatch2"
no_text="$no_text|accesskit|accesskit_atspi_common|accesskit_consumer"
no_text="$no_text|accesskit_macos|accesskit_unix|accesskit_windows|accesskit_winit"
no_text="$no_text|lyon_algorithms|lyon_extra|lyon_geom|lyon_path"
no_text="$no_text|taffy|clipboard-win|integer-sqrt"

# Generic texts standing in for a crate's own, less the ones named above.
fallbacks=$(grep 'falling back to canonical text' "$log" |
    grep -v -E "for crate '($no_text) " || true)

if [ -n "$problems$fallbacks" ]; then
    printf '%s\n' "$problems" "$fallbacks" | sed '/^$/d' >&2
    echo "the notices are incomplete; nothing was written to $out" >&2
    exit 1
fi
# Slint's licence, which cargo-about cannot carry: a LicenseRef has no
# canonical text, and a clarification does not give one to a licence the tool
# has never heard of. The text lives in the repository rather than being read
# out of a cargo cache at release time, and is checked here against the copy
# every Slint crate ships, so it cannot quietly go stale.
slint_text="$root/packaging/licenses/LicenseRef-Slint-Royalty-free-2.0.md"
slint_sha=5167f5056e850419106ab6265efbdca7cba4d99c849d1445ca0bbf6a1e2315fe
if [ "$(sha256sum "$slint_text" | cut -d' ' -f1)" != "$slint_sha" ]; then
    echo "$slint_text is not the text Slint 1.17 ships; nothing was written to $out" >&2
    exit 1
fi
for src in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/slint-1.17.*/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md; do
    [ -f "$src" ] || continue
    if ! cmp -s "$src" "$slint_text"; then
        echo "$slint_text differs from $src; nothing was written to $out" >&2
        exit 1
    fi
done

{
    printf '\n%s\n' "================================================================================"
    echo "Slint Royalty-free Desktop, Mobile, and Web Applications License 2.0"
    echo "         (LicenseRef-Slint-Royalty-free-2.0)"
    echo "================================================================================"
    echo "Used by:"
    for c in slint slint-macros i-slint-common i-slint-compiler i-slint-core \
        i-slint-core-macros i-slint-backend-selector i-slint-backend-winit \
        i-slint-renderer-software; do
        echo "  - $c 1.17.1 <https://github.com/slint-ui/slint>"
    done
    echo "    Authors: SixtyFPS GmbH <info@slint.dev>"
    echo
    cat "$slint_text"
} >>"$draft"

# The embedded fonts are not crates, so cargo-about does not know them.
{
    printf '\n%s\n' "================================================================================"
    echo "SIL Open Font License 1.1 (OFL-1.1)"
    echo "================================================================================"
    echo "Used by the fonts embedded in relabel-render:"
    echo "  - Noto Sans Condensed and Noto Sans Condensed SemiBold 2.015"
    echo "  - Noto Sans Mono Condensed 2.014"
    echo
    cat "$root/crates/relabel-render/fonts/OFL.txt"
} >>"$draft"

# mktemp makes files readable by their owner alone; this one ships.
chmod 644 "$draft"
mv "$draft" "$out"
echo "wrote $out"
