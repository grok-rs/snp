# Benchmarks and budgets

Owned by WP-36. This directory holds the budgets of the performance and memory gates (ARCHITECTURE §12, research
10 §0.2 and §7.11, ADR-043, ADR-071, ADR-079), the pinned prek lock, and the JSON-lines history the gate compares
against.

| File | What |
|---|---|
| `budgets.toml` | every gated number: the hard/soft budgets per OS of ARCHITECTURE §12.1, the regression rule, the never-slower-than tools, the syscall ceiling, the allocation budget, the micro thresholds of the W1/W2 packages and the adopted musl allocator |
| `prek-requirements.{in,txt}` | prek 0.5.5, hash-locked (`uv pip compile --universal --generate-hashes`); installed wheels-only by `bench setup` |
| `tools-requirements.{in,txt}` | the bench helpers, hash-locked: uv 0.12.19 (linked as `target/bench/tools/path/uv`, the head of the hermetic PATH, so prek and snp never download a uv) and pip (downloads the wheelhouse) |
| `wheelhouse-requirements.{in,txt}` | the Python packages B6/B7 install (pre-commit-hooks' `ruamel.yaml` and the `setuptools` build backend of its `setup.py`), hash-locked; `bench setup` downloads them into `target/bench/tools/wheels`, and every measured install resolves from there with no index |
| `history/<platform>-<variant>.jsonl` | one run record per line, appended by `cargo xtask bench gate --record` (CI keeps it in the Actions cache and uploads it as an artifact; git-ignored, so a local `--record` leaves the tree clean) |
| `musl-allocator.md` | the ADR-079 musl allocator measurement and decision |

## Running

```sh
cargo xtask oracle install          # pre-commit 4.6.2 on CPython 3.12.3 (target/oracle/pc462-py312)
cargo xtask bench setup             # hyperfine 1.19.0, prek 0.5.5, fixtures, snp (dist), warm stores
cargo xtask bench run               # target/bench/results/current.json
cargo xtask bench gate              # budgets + regression against the last history record
start=$(date +%s) && cargo bench -p snp --bench pyre_kinds ... -- --noplot && cargo xtask bench gate --micro --since "$start"
```

`setup`, `run` and `gate` take `--help`. Useful options: `--scenarios B1,B3`, `--tools snp,prek`, `--snp PATH` (an
existing binary instead of the dist build), `--target x86_64-unknown-linux-musl --allocator mimalloc`, `--syscalls`
(`strace -f -c`), `--dhat` (B3's total allocation), `gate --baseline FILE`, `gate --comment FILE`,
`gate --compare A.json B.json`. Everything is written under `target/bench/`; `--mirror-from` lets `setup` clone the
pre-commit-hooks mirror from a local repository.

**Cost on a shared machine.** `setup` and `run` build snp with the fat-LTO `dist` profile, `setup` compiles
hyperfine (`cargo install`), and `run --dhat` makes a second complete release build of the workspace in
`target/bench/dhat-target` (a separate feature set: nothing is shared with the other builds, several GB and the
longest build of all). Every one of these cargo invocations takes `--jobs N` (default: `CARGO_BUILD_JOBS`, else every
core); pass it when other work runs on the machine. Only `setup` uses the network (hyperfine, prek, the helper
tools, the wheelhouse and the pre-commit-hooks mirror).

## Scenarios (ARCHITECTURE §12.1)

All fixtures are generated deterministically (fixed identity and dates, an LCG for the file corpora): the same trees
and commit SHAs on every machine. Every tool runs in a hermetic environment (own `HOME`, `GIT_CONFIG_GLOBAL`, no
system git config, C.UTF-8, `PATH=/usr/local/bin:/usr/bin:/bin`) with its own store (`SNP_HOME`, `PRE_COMMIT_HOME`,
`PREK_HOME`). No measurement touches the network: every clone of pre-commit-hooks is redirected to a local bare
mirror (`url.<mirror>.insteadOf`), every Python install resolves from the local wheelhouse only (`PIP_NO_INDEX`,
`PIP_FIND_LINKS`, `UV_NO_INDEX`, `UV_FIND_LINKS`, `UV_OFFLINE`, `UV_PYTHON_DOWNLOADS=never`,
`VIRTUALENV_NO_PERIODIC_UPDATE`), and the pinned uv is on the PATH. So B7 (store removed before every run) measures
installation from local files, never network latency.

`GIT_CEILING_DIRECTORIES=target/bench` keeps git from discovering any repository above the bench directory, and
before a scenario is primed or measured its fixture must resolve (`git rev-parse --show-toplevel`) to exactly its own
directory. A fixture whose `.git` is missing or broken is therefore an error. Without this, git would fall through to
the enclosing snp checkout, and B9's 56 `git commit`s would land on its checked-out branch and run its hooks.

Before a scenario is measured, `setup` runs it once per tool. A tool that cannot run it (for example a snp command
that is not implemented yet) is recorded as unavailable, and the gate fails that scenario for snp. `setup` marks every
scenario it prepares as unavailable before it rebuilds anything and clears the mark only after the priming
succeeded, so an interrupted setup leaves nothing for `run` to measure.

| ID | Fixture | Command | Measured |
|---|---|---|---|
| B1 | `noop10` | `--version` | time; nightly: syscalls (`strace -f -c`, ceiling 64) |
| B2 | `noop10`: 10 local `true` hooks, `always_run`, `pass_filenames: false` (prek's corpus) | `run noop-01` | time |
| B3 | `noop10` | `run` | time, RSS; nightly: syscalls (`strace -f -c` over the whole tree, ceiling 4230) and dhat total allocation (≤ 2 MB = 1.907 MiB) |
| B4 | `files50k`: 50 000 tracked files, one `true` hook with filenames | `run --all-files` | time, RSS (B12 ≤ 30 MB = 28.61 MiB) |
| B5 | `files50k` with `types`/`types_or`/`exclude_types` | `run --all-files --config types.yaml` | time (≤ 1.3× B4) |
| B6 | `pch960`: 960 clean files, 13 pre-commit-hooks hooks at v6.0.0 (prek's medium corpus) | `run --all-files` | time, RSS (B12 ≤ 25 MB = 23.84 MiB) |
| B7 | `pch960`, store removed before every run (`--prepare`) | `run --all-files` | time |
| B8 | `stash10m`: a 10 MiB unstaged rewrite of a tracked file + one staged change | `run` | time, RSS (≤ B3 + 2 MB) |
| B9 | `noop10` with the tool's own installed hook | `git commit --allow-empty` | time (≤ B3 + 3 ms) |
| B10 | `validate1000`: 1 000 local hooks | `validate-config big.yaml` | time |
| B11 | `gc500`: 500 hook repositories used by 200 configurations, all installed | `gc` | time |
| B12u | `unicode20`: 20 000 non-ASCII paths, 20 hooks with Unicode `files` patterns, `SNP_JOBS=32` | `run --all-files` | time, RSS (≤ 30 MB = 28.61 MiB; the ADR-079 regex-cache case) |

Timing: `hyperfine -N -w 5 -r 50` (the slow scenarios use fewer runs, listed in `xtask/src/tasks/bench.rs`). RSS:
the median of 10 runs under `/usr/bin/time -v` (`-l` on macOS); it is the peak resident set of the largest process
the tool waited for, so a git child larger than the tool itself is what gets reported (B8).

## The gate

* **Regression**: snp's median more than 5 % above the baseline's **and** the two 95 % confidence intervals of the
  medians disjoint (distribution-free order-statistic intervals, ranks `n/2 ∓ 1.96·√n/2`) fails; a > 5 % change with
  overlapping intervals is a warning. Pull requests and pushes to main use the base commit (the PR base, or the commit
  before the push) measured on the same runner as the baseline, because records from different runner VMs differ by
  more than 5 % for identical binaries; the nightly uses the last history record of the same platform and variant.
* **Drift** (the nightly, and a push without a previous commit, which compare against the history): the same rule against the record with the
  median snp time among the last 10 history records (at least 3 needed). Without it, a series of slowdowns each just
  under 5 % of the previous record would pass forever, since every passing run becomes the next baseline. The gate reads the records and
  criterion's results from cargo's target directory (`CARGO_TARGET_DIR` respected), like `run` writes them, and uses
  hyperfine 1.19.0 only (another hyperfine on PATH is ignored).
* **Budgets**: every `[[budget]]` of the running OS; `hard = true` fails, a soft breach is a warning.
* **Nothing passes silently**: a hard budget whose scenario is not in the record fails (a soft one warns), unless the
  scenario is listed in `--allow-missing` (the scenarios a workflow knowingly does not run yet, `BENCH_PENDING` in
  `.github/workflows/bench.yml`), which turns it into a warning. A syscall or allocation budget is checked only when
  the run asked for that metric (`--syscalls`, `--dhat`, recorded in the record's `extras`); asked for but without a
  value, it fails. A missing baseline is a warning (the regression check is skipped), and so is a baseline without
  the scenario.
* **Never slower**: snp's p50 above pre-commit's or prek's on any scenario fails (musl: the non-adopted allocator is
  reported only). A competitor that failed or was not measured on a scenario snp measured also fails, because the
  rule could not be checked, unless the scenario is listed in `--allow-missing-reference` (which makes it a warning
  naming the reason). The tools of `never_slower_than` are validated when the budgets are read, so a misspelt name
  is an error, not a comparison that never happens.
* **Micro** (`gate --micro`): criterion's median of each `[[micro]]` benchmark against its exact threshold. A missing
  result fails. `--since <unix-secs>` is required (CI passes the time `cargo bench` started): a result older than
  that is stale and counts as missing, so a renamed or removed benchmark cannot pass on old numbers. `--any-age`
  accepts results of any age. These are the exact targets whose release-mode `perf_*` tests (the `perf-smoke` CI job) only catch a
  3× regression: argparse hook-impl parse < 5 µs (WP-12), tier-1 path searches ≥ 1,000,000/s (WP-13),
  classification of 50 k files with 10 filters ≤ 40 ms (WP-22), alias bomb and 10,000-deep nesting rejected in
  < 100 ms (WP-14).
  A threshold with `reference_only = true` (the WP-22 classification limit) is hard only with `--reference`, on
  the reference machine; on the shared CI runners its breach is a warning (ADR-086 amendment of ADR-043).

## Spawn budgets

`crates/snp/tests/it/budgets.rs` runs a `--features testing` build with `SNP_TEST_SPAWN_LOG` and asserts the
documented upper bounds per scenario shape (B1 0 spawns, B2 ≤ 8, B3 ≤ 18, B4 one partition, B6 ≤ 13 and no non-git
process); see the table in that file. They run in every `cargo nextest run --workspace --features snp/testing`.

## Criterion micro-benches (`crates/snp/benches`)

`pyre_kinds` (fast kinds vs the tier-1/tier-2 engines on 100 k paths, the typical tier-1 set, 32 threads × 20 Unicode
patterns), `shuffle_partition` (MT19937 shuffle + xargs partition of 50 k names), `tagging_tiers` (name/stat/content
tiers, prefetch, classification), `yaml_manifest_load` (the pre-commit-hooks v6.0.0 manifest, the B3 config, the
alias bomb, deep nesting), `argparse_hook_impl` (the hook-impl argv in both brands), `len_cjk`.

## ADR-047: speculative config load

The speculative load runs inside `snp run` (WP-32). Its effect is measured as an A/B of two snp builds on the same
runner — the normal build and one with the speculation disabled — with
`cargo xtask bench run --snp <build> --tools snp --scenarios B3,B9 --label <name>` for each and
`cargo xtask bench gate --compare A.json B.json`, which prints the median change per scenario and whether the 95 %
intervals are disjoint. ADR-047 keeps the speculation only if B3/B9 show a significant win. Until `snp run` exists
there is nothing to measure (see the WP-36 report).
