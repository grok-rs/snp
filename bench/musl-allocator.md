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

**Status.** **System allocator** (`adopted = "system"`), measured. The musl job has measured B1, B3, B4, B6 and B12u
nightly since `MUSL_SCENARIOS` was widened (ADR-091). Run 44301994701 (2026-11-18) found mimalloc significantly
faster on B4 (−5.5 %), B6 (−23.4 %) and B12u (−10.1 %), at a cost of +34.3/+21.3/+41.9 MiB of peak RSS. Most of that
cost came from mimalloc's transparent huge pages, so mimalloc is now built with `no_thp` (ADR-118). Even then it costs
+0.5/+2.4/+5.3 MiB on B4/B6/B12u (four CPUs, `musl-system` 19.1/8.2/16.5 MiB), more than the 2 MiB the rule allows on
B6 and B12u.
