#!/usr/bin/env bash
# Pinned proof tools (ADR-095, docs/VERIFICATION.md §5; owned by WP-00). Used by
# `cargo xtask proofs install|verus|mutants|status`, which ci.yml (xtask gates), release.yml (gates) and nightly.yml
# (proofs-sweep) run (ADR-096); runnable locally.
#
#   proofs-tools.sh install   download the Verus release zip and verusfmt pinned in proofs/registry.toml, check
#                             their sha256, unpack them into the tools directory, and install Verus's rustc
#                             (`rustup toolchain install <rustc> --profile minimal`; never made the default: the
#                             `verus` launcher selects it through RUSTUP_TOOLCHAIN). An existing install is kept.
#   proofs-tools.sh path      print the directory holding `verus` and `verusfmt` (whether or not installed)
#   proofs-tools.sh status    say what is installed
#
# Tools directory: $SNP_PROOFS_TOOLS (default ~/.cache/snp-dev/tools), then verus/<version>/verus-<platform>/,
# the layout of the release zip. Only the `[verus]` table's `key = "value"` and inline-table lines are understood
# (the format of proofs/registry.toml).
set -euo pipefail

cmd="${1:-}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
registry="$here/../../proofs/registry.toml"
[ -f "$registry" ] || { echo "proofs-tools: no such file: $registry" >&2; exit 2; }

# The [verus] table, one line per key.
verus_table() {
    awk '/^\[verus\][[:space:]]*$/ { on = 1; next } /^\[/ { on = 0 } on && /^[a-z]/ { print }' "$registry"
}
# `key = "value"` of the [verus] table.
pin() {
    verus_table | sed -n "s/^$1[[:space:]]*=[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}
# The sha256 of platform $2 in line `$1 = { … }` (for verusfmt: inside its `sha256 = { … }`).
pinned_sha() {
    verus_table | sed -n "s/^$1[[:space:]]*=.*[[:space:]{]$2[[:space:]]*=[[:space:]]*\"\([0-9a-f]\{64\}\)\".*/\1/p"
}

version="$(pin version)"
rustc="$(pin rustc)"
fmt_version="$(verus_table | sed -n 's/^verusfmt[[:space:]]*=.*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p')"
[ -n "$version" ] && [ -n "$rustc" ] && [ -n "$fmt_version" ] || {
    echo "proofs-tools: cannot read version, rustc and verusfmt.version from [verus] of $registry" >&2
    exit 2
}

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) plat=x86-linux; fmt_target=x86_64-unknown-linux-gnu ;;
    Darwin-arm64) plat=arm64-macos; fmt_target=aarch64-apple-darwin ;;
    Darwin-x86_64) plat=x86-macos; fmt_target=x86_64-apple-darwin ;;
    *) echo "proofs-tools: no Verus release for $(uname -s)-$(uname -m)" >&2; exit 2 ;;
esac

tools="${SNP_PROOFS_TOOLS:-$HOME/.cache/snp-dev/tools}"
bin_dir="$tools/verus/$version/verus-$plat"

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch URL FILE SHA: download unless present with the right hash; fail on a mismatch.
fetch() {
    local url="$1" file="$2" want="$3"
    if [ -f "$file" ] && [ "$(sha256_of "$file")" = "$want" ]; then
        return 0
    fi
    curl --proto '=https' --tlsv1.2 -fsSL --retry 3 -o "$file.part" "$url"
    local got
    got="$(sha256_of "$file.part")"
    if [ "$got" != "$want" ]; then
        rm -f "$file.part"
        echo "proofs-tools: sha256 mismatch for $url: got $got, pinned $want" >&2
        exit 1
    fi
    mv "$file.part" "$file"
}

install() {
    local sha fmt_sha dl
    sha="$(pinned_sha sha256 "$plat")"
    fmt_sha="$(pinned_sha verusfmt "$fmt_target")"
    [ -n "$sha" ] || { echo "proofs-tools: no pinned sha256 for Verus on $plat in $registry (add it)" >&2; exit 1; }
    [ -n "$fmt_sha" ] || { echo "proofs-tools: no pinned sha256 for verusfmt on $fmt_target in $registry (add it)" >&2; exit 1; }
    dl="$tools/verus/_download"
    mkdir -p "$dl"

    if [ -x "$bin_dir/verus" ] && grep -q "\"version\": \"$version\"" "$bin_dir/version.json" 2>/dev/null; then
        echo "proofs-tools: Verus $version already in $bin_dir"
    else
        local zip="$dl/verus-$version-$plat.zip"
        fetch "https://github.com/verus-lang/verus/releases/download/release%2F$version/verus-$version-$plat.zip" "$zip" "$sha"
        rm -rf "$bin_dir"
        mkdir -p "$tools/verus/$version"
        unzip -q "$zip" -d "$tools/verus/$version"
        [ -x "$bin_dir/verus" ] || { echo "proofs-tools: $zip did not unpack to $bin_dir" >&2; exit 1; }
        echo "proofs-tools: Verus $version in $bin_dir"
    fi

    if [ -x "$bin_dir/verusfmt" ] && "$bin_dir/verusfmt" --version 2>/dev/null | grep -q "$fmt_version"; then
        echo "proofs-tools: verusfmt $fmt_version already in $bin_dir"
    else
        local tarball="$dl/verusfmt-$fmt_version-$fmt_target.tar.xz" unpack="$dl/verusfmt-$fmt_version"
        fetch "https://github.com/verus-lang/verusfmt/releases/download/v$fmt_version/verusfmt-$fmt_target.tar.xz" "$tarball" "$fmt_sha"
        rm -rf "$unpack"
        mkdir -p "$unpack"
        tar -xJf "$tarball" -C "$unpack"
        cp "$unpack/verusfmt-$fmt_target/verusfmt" "$bin_dir/verusfmt"
        chmod +x "$bin_dir/verusfmt"
        echo "proofs-tools: verusfmt $fmt_version in $bin_dir"
    fi

    # The archives are unpacked: drop them (they would double the CI cache).
    rm -rf "$dl"

    if rustup toolchain list | grep -q "^$rustc-"; then
        echo "proofs-tools: rustup toolchain $rustc already installed"
    else
        rustup toolchain install "$rustc" --profile minimal --no-self-update
    fi
    echo "proofs-tools: add $bin_dir to PATH to run verus directly (cargo xtask proofs finds it there)"
}

status() {
    if [ -x "$bin_dir/verus" ]; then echo "verus $version: $bin_dir"; else echo "verus $version: not installed ($bin_dir)"; fi
    if [ -x "$bin_dir/verusfmt" ]; then echo "verusfmt $fmt_version: $bin_dir"; else echo "verusfmt $fmt_version: not installed"; fi
    if rustup toolchain list 2>/dev/null | grep -q "^$rustc-"; then echo "rustc $rustc: installed"; else echo "rustc $rustc: not installed"; fi
}

case "$cmd" in
    install) install ;;
    path) echo "$bin_dir" ;;
    status) status ;;
    *) echo "usage: proofs-tools.sh install|path|status" >&2; exit 2 ;;
esac
