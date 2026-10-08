#!/usr/bin/env bash
# Runs one shard of the cargo-fuzz targets (ARCHITECTURE §13.5; owned by WP-00), after `cargo +nightly fuzz build`.
#
#   fuzz-run.sh SHARD SHARDS SECONDS
#
# Target i runs when i % SHARDS == SHARD, for SECONDS each, with the §13.5 limits: no hang > 5 s, <= 512 MB RSS and
# 256 KiB stacks. libFuzzer runs every input on the main thread, so its stack is capped with `ulimit -s 256`; threads
# a target spawns get 256 KiB through RUST_MIN_STACK. Both apply to the fuzz binary only: rustc honours
# RUST_MIN_STACK for its own main thread and overflows with 256 KiB, so the build step never sets it.
set -euo pipefail

shard="${1:?shard}"
shards="${2:?shards}"
seconds="${3:?seconds}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

host="$(rustc +nightly -vV | sed -n 's/^host: //p')"
bin="$root/fuzz/target/$host/release"
i=0
for t in $(cargo +nightly fuzz list); do
    if [ $((i % shards)) -eq "$shard" ]; then
        [ -x "$bin/$t" ] || { echo "fuzz-run: $bin/$t not built (run cargo +nightly fuzz build)" >&2; exit 2; }
        echo "::group::$t"
        mkdir -p "fuzz/corpus/$t" "fuzz/artifacts/$t"
        (
            ulimit -s 256
            RUST_MIN_STACK=262144 "$bin/$t" -max_total_time="$seconds" -timeout=5 -rss_limit_mb=512 \
                -artifact_prefix="fuzz/artifacts/$t/" "fuzz/corpus/$t"
        )
        echo "::endgroup::"
    fi
    i=$((i + 1))
done
