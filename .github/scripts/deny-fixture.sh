#!/usr/bin/env bash
# deny.toml regression fixture (WBS WP-00 acceptance; ARCHITECTURE §1.3 "Banned"; owned by WP-00).
#
#   deny-fixture.sh
#
# Copies the workspace (tracked files, working-tree contents) into a temporary directory next to links to the
# sibling user crates, plants one banned dependency at a time in crates/snp-pycompat/Cargo.toml (`regex`, which is
# allowed only behind snp-pyre; `which`; `tokio`) and expects `cargo deny check bans` to fail naming that crate as
# banned. Exits non-zero when a plant is accepted, so a weakened deny.toml fails CI. Needs network access for the
# crates.io index (the plants are resolved into the copy's Cargo.lock only).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mkdir "$tmp/base"
(cd "$root" && git ls-files -z | tar --null -T - -cf -) | tar -xf - -C "$tmp/base"
for sibling in cfgv-rs file-identify; do
    ln -s "$(cd "$root/../$sibling" && pwd)" "$tmp/$sibling"
done

failed=0
for plant in 'regex = { workspace = true }' 'which = "7"' 'tokio = { version = "1", default-features = false }'; do
    crate="${plant%% *}"
    ws="$tmp/snp"
    rm -rf "$ws"
    cp -R "$tmp/base" "$ws"
    printf '\n[dependencies]\n%s\n' "$plant" >> "$ws/crates/snp-pycompat/Cargo.toml"
    set +e
    out="$(cd "$ws" && cargo deny --log-level error check bans 2>&1)"
    status=$?
    set -e
    if [ "$status" -ne 0 ] && grep -q "crate '$crate = [^']*' is explicitly banned" <<<"$out"; then
        echo "deny-fixture: ok: $crate in snp-pycompat is rejected"
    else
        echo "deny-fixture: FAILED: $crate in snp-pycompat was not rejected as banned (exit $status)" >&2
        printf '%s\n' "$out" >&2
        failed=1
    fi
done
exit "$failed"
