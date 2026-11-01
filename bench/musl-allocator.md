# musl global allocator (ADR-079 item 4)

Owned by WP-36.

**Question.** musl's allocator (mallocng) is markedly slower than glibc's under allocation-heavy, multi-threaded
load. snp's release profile uses the system allocator everywhere (ARCHITECTURE §12.2 T17); ADR-079 leaves the musl
artifacts' allocator to a measurement of B3/B4/B6/B12 with the system allocator and with mimalloc.

**Mechanism.** `crates/snp/src/main.rs` installs `mimalloc::MiMalloc` as the `#[global_allocator]` only under
`cfg(all(target_env = "musl", feature = "musl-mimalloc"))` (and never together with `dhat-heap`). The feature is off by
default; `deny.toml` admits mimalloc only through `wrappers = ["snp"]`.

**Measurement.** The `musl` job of `.github/workflows/bench.yml` (nightly and on dispatch) builds
`x86_64-unknown-linux-musl` twice and measures the musl scenarios with both allocators:

```sh
cargo xtask bench setup --target x86_64-unknown-linux-musl
cargo xtask bench run --target x86_64-unknown-linux-musl --allocator system   --label musl-system
cargo xtask bench run --target x86_64-unknown-linux-musl --allocator mimalloc --label musl-mimalloc
cargo xtask bench gate --compare target/bench/results/musl-system.json target/bench/results/musl-mimalloc.json
```

The records go to `bench/history/linux-x86_64-musl-{system,mimalloc}.jsonl`. The gate holds the adopted allocator
(`[musl] adopted` in `budgets.toml`) to the rule that applies to every artifact: never slower than prek at p50 on any
scenario, and the B12 RSS budgets.

**Decision rule.** Adopt mimalloc only if it is significantly faster (disjoint 95 % intervals) on B4, B6 or B12u
and costs at most 2 MiB of peak RSS on B4/B6/B12u; otherwise keep the system allocator (smaller binary, no C
dependency in the default build). Adopting it means setting `[musl] adopted = "mimalloc"` and enabling
`--features musl-mimalloc` for the musl targets of the release build (WP-37).

**Status.** Provisional: **system allocator** (`adopted = "system"`). B3/B4/B6/B12u need `snp run` (WP-32), which is
not merged yet; the measurement runs as soon as the bench workflow's `MUSL_SCENARIOS` is widened to
`B1,B3,B4,B6,B12u`, and its result is recorded here and in ADR-079.
