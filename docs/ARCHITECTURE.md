# snp — Architecture (authoritative)

> **snp** ("You Shell Not Pass") is a Rust drop-in replacement for **pre-commit 4.6.2** with optional,
> lefthook-inspired extensions. Repository `github.com/grok-rs/snp`, MIT, author Serhii Kaliuzhnyi.
>
> This document is the **contract** the parallel implementers code against. §1.4 lists every cross-crate public
> API as real Rust signatures; WP-00 turns them into compiling stubs plus a `tests/api.rs` per crate that references
> every public item. Changing a signature after WP-00 needs an ADR in `docs/DECISIONS.md` and the agreement of every
> consuming package owner.
>
> Companion documents: `docs/DECISIONS.md` (numbered ADRs: every contradiction and open question resolved),
> `docs/COMPATIBILITY.md` (policy, branding, the complete DIVERGENCES register, extension-key reference),
> `docs/WBS.md` (work packages in waves).
>
> Research references are written `NN §x` = `docs/research/NN-*.md` section x. `01 Addendum` is the normative regex
> addendum at the end of `01-config-schema.md` (§A.0–§A.11); it overrides every earlier regex statement in any
> document. Where this document and a research document disagree, this document wins; where this document is silent,
> the research document is normative.
>
> Design lineage: the skeleton (invariants, contracts-first crate layout, seams, journaled stash, fault-injection
> matrices) comes from the *robustness* proposal; the byte-parity layer (Python two-layer stdout, brand_map, keys
> markers, Windows CRLF/VT parity, `clean` parity) from the *compatibility* proposal; the performance layer
> (DiffOracle, tiered lazy tags, regex fast kinds, git fan-out, budgets) from the *performance* proposal — with every
> fatal flaw the three judges found corrected (ADR-001).

---

## 0. Overview

### 0.1 Fixed product decisions (owner; never re-opened)

| ID | Decision |
|---|---|
| FD-1 | Binary `snp`. In-place replacement for pre-commit: same `.pre-commit-config.yaml` / `.pre-commit-hooks.yaml`, CLI subcommands/flags, output layout, exit codes, env vars. Branding follows `argv[0]`: through a `pre-commit` name the user-facing text is byte-identical to pre-commit 4.6.2 (product name included); as `snp` the layout is identical but the product name is `snp`. |
| FD-2 | Own cache store: `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp` (Windows `%USERPROFILE%\.cache\snp`, the platform equivalent of pre-commit's own default, 14 P-65). Never read or write pre-commit's store; no SQLite; hash-keyed directories with marker files written last. |
| FD-3 | User crates as path deps with version requirements: `cfgv = { path = "../cfgv-rs", version = "0.3" }`, `file-identify = { path = "../file-identify", version = "0.5" }` (branch `snp-support`; the real APIs are bound in §1.5). |
| FD-4 | `rust-toolchain.toml` channel `1.99.0`; `rust-version = "1.98"`; edition 2024; resolver 3. |
| FD-5 | `unsafe_code = "forbid"` in every snp crate; no panics on any input; `panic = "unwind"` + top-level guard (exit 3 + crash log) so the stash guard always restores; SIGINT/SIGTERM/SIGHUP-safe restore; fail closed. |
| FD-6 | Behave exactly like pre-commit 4.6.2 except where pre-commit crashes, loses user data or has security bugs; every deliberate divergence is in the DIVERGENCES register (`docs/COMPATIBILITY.md` §5). |
| FD-7 | Maximal performance and memory efficiency: `--version` ≤ 3 ms, no-op run of 10 hooks ≤ 25 ms (Linux; per-OS budgets in §12). |
| FD-8 | v1 scope: all 16 pre-commit commands, all 21 languages, native pygrep/fail/meta, native pre-commit-hooks fast path ON by default (opt-out flag/env/config), lefthook MUST/SHOULD features as optional extension keys, extra snp commands. Multi-project workspace mode: seams in v1, implementation in v1.1 (§1.6). |
| FD-9 | Repo `github.com/grok-rs/snp`, MIT, author "Serhii Kaliuzhnyi"; workspace at `/home/dromix/projects/snp`. |

### 0.2 Invariants

Each invariant has an owner, a mechanism and a test family; weakening one needs an ADR.

| # | Invariant | Mechanism (owner crate) | Verified by |
|---|---|---|---|
| S1 | No `unsafe` in snp code | `unsafe_code = "forbid"` workspace lint; platform needs via safe APIs of `rustix`, `signal-hook`, `ctrlc`, `winapi-util`, `process-wrap`, std `CommandExt` | `xtask check-unsafe`, CI |
| S2 | No panic reachable from input | clippy `unwrap_used/expect_used/panic/indexing_slicing/string_slice/todo/unimplemented/unreachable/arithmetic_side_effects/unchecked_time_subtraction = deny`, `disallowed-macros` (`assert*!`) and `disallowed-methods` (panicking std APIs, thread spawns, §1.3) outside tests; thread creation through `Builder::spawn_scoped` (an OS refusal is a `Failure`, not a panic); duration env vars parsed with `Duration::try_from_secs_f64` and an upper bound; bounded recursion (§11.4); `overflow-checks` only as a backstop (ADR-066) | fuzz targets (256 KiB stacks, 512 MB RSS) |
| S3 | The user's unstaged work survives any single failure | durable patch re-verified immediately before the stash checkout, journal + pointer file in the git dir, worktree lock, restore on every exit path, restore-critical git commands in their own process group and never signalled (`Role::Critical`), descendants of cancelled hooks killed before the restore (Linux subreaper, Windows job; macOS residual risk documented in D-DATA-02), next-run recovery notice (snp-stash, §4) | fault-injection FI-01…FI-30, kill-9, signal, disk-full matrices (§13.6) |
| S4 | Fail closed | no error path exits 0 unless every selected hook ran and passed; EPIPE → 120; malformed pre-push → 3; missing binary in hook script → 1 | `fail_closed_*` property tests over injected faults |
| S5 | Parity by default, divergence by register | every difference from 4.6.2 has an ID in COMPATIBILITY §5 and a differential scenario asserting it still diverges | difftest fails on unlisted diffs *and* on listed diffs that no longer reproduce |
| S6 | No process-global mutable state | immutable `Env` snapshot threaded explicitly; never `set_var`/`remove_var`/`set_current_dir` (clippy `disallowed-methods`) | lint; tests run in parallel |
| S7 | Bounded resources | one global `JobPool` for hook processes and native partitions, sized `min(target_concurrency, available_parallelism)` unless `-j`/`SNP_JOBS`, clamped to the fd budget; separate `Limiter`s for network and installs (compile-heavy installs exclusive); one global output byte budget with spill to unnamed files (§3.4, §3.6) | `pool_never_exceeds_permits`, low-`ulimit -n` test, RSS gate B12 |
| S8 | Untrusted input never reaches a shell, a path join or the store layout unvalidated | argv-only spawning, env-dir name sanitising, archive validation, containment checks | SEC-01…SEC-14 suite |
| S9 | Network bytes are verified before use | TLS + sha256 (embedded or TLS-fetched manifests), verify-then-extract (snp-net) | checksum-mismatch tests |
| S10 | Crash-consistent on-disk state | markers written last; temp + fsync + rename; never write through symlinks (snp-os) | kill-9 matrix |
| S11 | Deterministic output | config-order head-of-line printer; `IndexMap`; clippy `iter_over_hash_type = deny` | `--jobs 1` vs `--jobs 32` byte equality |
| S12 | Every crate testable in isolation | seams: `GitBackend` (+ `ScriptedBackend`), `CmdRunner`, `Resolve`, `Preflight`, `Fetch`, `Clock`, `ToolDirs`, `ToolProbe` | unit suites with zero real processes |
| S13 | Exact optimisations only | every optimisation that skips work (DiffOracle, baseline elision, lazy tags, fast path, P17, speculative config load, regex fast kinds) has a soundness argument and a randomized/differential test against the unoptimised path; any doubt falls back to the exact path | §13.4 property tests, `SNP_DIFF_MODE=git`, `SNP_VERIFY_BASELINE=1` in CI |
| S14 | Poison-proof safety paths | every `Mutex`/`RwLock` on a safety path is taken through `snp_core::sync::lock` (recovers `PoisonError::into_inner`) | panic-injection test inside a worker holding the printer lock (stash restored, exit 3, banner printed) |
| S15 | fd hygiene under concurrent spawning | Linux: every fd snp creates is `O_CLOEXEC` atomically (std pipes, pty master `openpt(CLOEXEC)`, pty slave `open(O_RDWR\|O_NOCTTY\|O_CLOEXEC)`); macOS/BSD: a process-wide spawn lock serialises fd creation and `spawn()`; hook output is read by dedicated reader threads, so a descendant holding a pipe delays only its own hook and never blocks cancellation (§3.6) | N-partition test (pipe and pty mode) where one child sleeps: the others finish promptly; cancellation with a daemonised descendant holding stdout |

### 0.3 Emulated Python profiles (per subsystem; never one global "Python version")

| Subsystem | Profile | Source |
|---|---|---|
| `files`/`exclude`/pygrep regex grammar (accept/reject) | CPython **3.12** `re/_parser.py` | 01 Addendum §A.3 |
| Regex Unicode data (`\w \d \s`, case folding, `\N{}`) | `regex-syntax` 0.8 = **UCD 16.0**; no second table set | 01 Addendum §A.3, §A.5 |
| argparse help/usage/error texts (pre-commit CLI, `pygrep.py`, `hazmat.py`, pre-commit-hooks console scripts) | CPython **≥ 3.13.14** (`argparse` texts identical to 3.14.8) | 12 §A.2 |
| `repr()`, `str.isprintable`, East-Asian width (`_len_cjk`) | **UCD 15.1** (CPython 3.13) | 15 §A3, 07 Q13 |
| `str.split/strip/splitlines`, `shlex`, `os.path`, codecs | CPython 3.12 vectors + 3.13 delta file (no behavioural change) | 15 §A1 |
| pre-commit-hooks native emulation (json messages, recursion thresholds, ruamel/tomllib/expat messages) | CPython **3.13** | 09 §10 Q2 |
| identify tag tables | identify **2.6.20** (`file_identify::IDENTIFY_VERSION`) | 08 §4 |

Constants live in `snp_core::profile`. The differential harness knows which oracle produced each byte stream
(`pc462-py312` = pre-commit 4.6.2 on CPython 3.12.3, used for runtime behaviour and regex; `pc462-py313` = 4.6.2 on
CPython ≥ 3.13.14, used for argparse texts) and normalises the documented 3.12→3.13 argparse deltas (12 §A.4.1).

### 0.4 Decision summary (details in DECISIONS.md)

| Topic | Decision | ADR |
|---|---|---|
| Base | Robust's contracts-first skeleton + compat's parity layer + perf's performance layer, all judge-identified flaws fixed | 001 |
| Runtime | No async runtime: `std::thread::scope`, one global re-entrant `JobPool`, `Limiter`s for net/installs, blocking `ureq`, one signal thread with origin-aware forwarding (§10.4) | 004, 065, 070 |
| CLI | Own argparse-compatible engine (`snp-argparse`) driven by static tables (`snp-cli`); clap only in `xtask` for completions | 010 |
| YAML | `libyaml-safer` events + own PyYAML composer/resolver/constructor/representer; error texts target Tier A | 002 |
| Regex | Own CPython-3.12 parser + lowering; tier 1 `regex` 1.13 (linear), tier 2 `fancy-regex` 0.19.2 (`backtrack_limit` 1e8) behind a mandatory relaxed prefilter; fast kinds bypass engines | 003, 020 |
| Store | `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp` (Win `%USERPROFILE%\.cache\snp`); versioned buckets; blake3 keys; marker-last; shared/exclusive global lock; `keys-v1` message markers | 011, 024 |
| Stash | pre-commit's exact algorithm + durable patch re-verified before checkout, journal + git-dir pointer, worktree lock (nested-run token valid only while the lock is held), restore commands in their own process group and never signalled, SIGTERM/SIGHUP restore after killing hook descendants, `git apply --3way` hints on stderr, lock-based recovery notice | 016, 026, 065, 067 |
| Modification detection | exact `git diff` digest semantics; Unix DiffOracle (stat snapshot over **all** diff inputs + racy hashing); Windows always real `git diff`; baseline elided on `diff-index` rc 0 | 027 |
| Output | Printer reproduces Python's two-layer stdout (block-buffered `print()` when piped), CRLF for print/argparse text on Windows, `winapi-util` VT on stderr | 028, 029 |
| Fast path | default `auto`; Tier A (known tag, no clone) / Tier B (blob-verified clone); delegation decided before installs; marker-driven `Initializing`/`Installing` lines keep cold transcripts byte-identical | 031, 032 |
| Languages | All 21; Python via pinned uv (pip fallback); verified toolchain downloads; env dirs sanitised/hashed; per-deps-variant prefixes cloned locally (`--local`) | 009, 025, 033, 034 |
| Docker | image tag `pre-commit-<md5(repo-key hex)>` (never the `src` basename) | 035 |
| Kill switch | `SNP=0` skips snp's hooks only; the chained `.legacy` hook (git-lfs) always runs | 038 |
| Safety | forbid unsafe; panic-free lints incl. arithmetic and panicking std APIs; `catch_unwind` → exit 3; panic hook only records and cancels; restore in `RunState::drop` only as panic fallback; single-exit `run()`; poison-proof locks; fd hygiene | 015, 037, 040, 066 |

---
## 1. Workspace and crate layout

### 1.1 Repository tree

```
snp/
├─ Cargo.toml              virtual workspace; members = ["crates/*", "xtask"]; resolver = "3";
│                          [workspace.package] version = "2.0.0" (crates.io `snp` 1.x exists, 11 §A.1), edition = "2024",
│                          rust-version = "1.98", license = "MIT", authors = ["Serhii Kaliuzhnyi"],
│                          repository = "https://github.com/grok-rs/snp"; [workspace.lints]; COMPLETE [workspace.dependencies]
├─ Cargo.lock              committed; regenerated mechanically on merge (`cargo update -w`), never hand-merged
├─ rust-toolchain.toml     channel = "1.99.0", components = ["rustfmt", "clippy", "llvm-tools-preview"], profile = "minimal"
├─ rustfmt.toml clippy.toml deny.toml typos.toml .cargo/config.toml .config/nextest.toml .gitignore LICENSE README.md
├─ deps.lock               pinned commits of the sibling user crates (cfgv-rs, file-identify) for CI and releases (ADR-061)
├─ .github/workflows/      ci.yml (WP-00), nightly.yml (WP-00 skeleton; jobs call xtask tasks), platform.yml + release.yml (WP-37),
│                          bench.yml (WP-36), difftest.yml (WP-35)
├─ crates/
│  ├─ snp-core/            vocabulary, Env/EnvPatch, CancelToken, Failure envelope, Printer (Python stdout model), sync, spawnlock
│  ├─ snp-pycompat/        Python stdlib semantics (15 §A6) + precommit composites, MT19937, EAW, float repr
│  ├─ snp-argparse/        argparse-compatible parser + HelpFormatter + textwrap port
│  ├─ snp-cli/             pre-commit + snp ParserSpec tables (verbatim help), typed `Cli` façade
│  ├─ snp-pyre/            Python `re` front end (CPython 3.12), lowering, two engines, fast kinds, cache
│  ├─ snp-yaml/            PyYAML SafeLoader/SafeDumper compatibility over libyaml-safer
│  ├─ snp-os/              durable fs ops, file locks, terminal/colour, home/realpath, fs-type probes
│  ├─ snp-proc/            spawning (pipe/pty), child registry, signals, JobPool/Limiter, xargs, exe resolution, hook_cmd, timeouts
│  ├─ snp-probe/           (dev bin) cross-platform probe hook used by tests (10 §7.3.5)
│  ├─ snp-net/             verified downloads, safe archive extraction, toolchain catalogue
│  ├─ snp-lang/            Language trait, registry, shared helpers, CmdRunner seam, the 21 backends
│  ├─ snp-pyparsers/       CPython json, ruamel-compatible YAML checker, tomllib, expat well-formedness, difflib, INI
│  ├─ snp-hooks-data/      generated pre-commit-hooks tag profiles, manifests, blob table, builtin manifest
│  ├─ snp-git/             every git invocation, GitBackend seam (+ ScriptedBackend), pre-push/MERGE_MSG parsers, owned repos
│  ├─ snp-testkit/         (dev) hermetic TestEnv, git fixture builder, oracle locator, insta filters
│  ├─ snp-config/          schemas (cfgv 0.3), typed model, merge, extensions, overlay, migrate-config, rev rewrite, schema
│  ├─ snp-store/           store location/layout/keys/markers/locks, config tracking, gc/clean core
│  ├─ snp-files/           FileTable (arena, lazy tiered tags), classifier, hook filters
│  ├─ snp-stash/           staged-files stash guard (data-safety core) + DiffOracle
│  ├─ snp-hooks/           NativeHook trait, pygrep, fail, hazmat (in-hook + CLI), pre-commit-hooks ports, `repo: builtin`
│  ├─ snp-repo/            hook resolution, clones, fast-path decision, env install orchestration (`Resolve`)
│  ├─ snp-run/             run pipeline, scheduler, renderer, hook-impl, meta hooks, try-repo, install-hooks, list
│  ├─ snp-cmd/             install/uninstall/init-templatedir, autoupdate, gc/clean, validate-*, migrate/sample-config,
│  │                       doctor, self, cache, config dump/schema
│  ├─ snp-migrate/         `snp migrate lefthook|builtin|upstream` converters
│  ├─ snp-difftest/        (tooling, not shipped) differential harness: scenarios, normaliser, brand_map, register, shrinker
│  └─ snp/                 the binary (+ `lib.rs` with `entry()`): brand, `__hazmat`, parse, dispatch, envelope, crash log, guard
├─ xtask/                  dispatcher; one file per task under xtask/src/tasks/ (each owned by the WP that needs it)
├─ fuzz/                   cargo-fuzz crate (not a workspace member); targets fuzz_targets/<crate>_<name>.rs
├─ tests/differential/     scenarios/** + brand_map.yaml (WP-35); divergences/<ID>.yaml (owned per ID, WBS)
├─ tests/catalog/index.tsv every catalog row of 10 §5, 13 X.4 and 11 §A.20 with its namespaced ID and owner (ADR-062)
├─ bench/                  hyperfine scenario definitions + baselines (WP-36)
└─ docs/                   ARCHITECTURE.md, DECISIONS.md, COMPATIBILITY.md, WBS.md, research/, divergences/<ID>.md, user/
```

26 crates. Sizing rule: each crate is 1–10 kLOC with one owner per wave. Every compile-heavy third-party subtree lives
in exactly one crate (`regex`/`fancy-regex` → snp-pyre, `ureq`/`rustls`/`tar`/`zip` →
snp-net) so they compile in parallel and downstream edits never rebuild them.

### 1.2 Dependency DAG and implementation waves

Internal edges (every crate except `snp-pycompat` and `snp-probe` (a standalone dev binary) — `xtask` included — also depends on `snp-core`; `snp-core` depends on `snp-pycompat`):

```
snp-pycompat   ← snp-core
snp-argparse   (pycompat)                       snp-cli       (argparse)
snp-pyre       (pycompat)                       snp-yaml      (pycompat; cfgv)
snp-os         (pycompat)                       snp-proc      (pycompat, os; file-identify)
snp-net        (os)                             snp-lang      (pycompat, os, proc, net)
snp-pyparsers  (pycompat)                       snp-hooks-data()
snp-git        (pycompat, os, proc)             snp-testkit   (proc, git)          snp-probe ()
snp-config     (pycompat, yaml, pyre; cfgv, file-identify)
snp-store      (pycompat, os)                   snp-files     (pycompat, pyre, os, proc; file-identify)
snp-stash      (pycompat, os, proc, git)
snp-hooks      (pycompat, pyre, argparse, cli, proc, os, git, pyparsers, hooks-data; file-identify, cfgv)
snp-repo       (config, store, git, lang, hooks-data, files, proc, os, pyre, net)
snp-run        (repo, stash, hooks, files, config, store, git, lang, proc, os, pyre, yaml, cli, pycompat, net)
snp-cmd        (config, store, git, stash, yaml, os, proc, cli, pycompat, hooks-data, pyre)
snp-migrate    (config, yaml, pyre, hooks-data; file-identify)
snp            (run, cmd, migrate, repo, hooks, cli, store, proc, os, net, git, pyre, lang)
snp-difftest   (testkit, yaml)
xtask          (cli, hooks, argparse)           — build-time tool, never shipped; may not depend on snp or snp-difftest
```

**Dev-dependencies** are checked by `check-dag` too: a crate may dev-depend on any of its own ancestors above, and on
`snp-testkit` unless it is one of snp-testkit's ancestors (`snp-core`, `snp-pycompat`, `snp-os`, `snp-proc`, `snp-git`
keep self-contained test helpers: no dev-cycles). `crates/snp` may dev-depend on every crate (integration tests, the
WP-36 criterion benches over snp-pycompat, snp-yaml, snp-files, snp-argparse, snp-pyre). Test binaries locate
`snp`/`snp-probe` through `snp_testkit::Bins` (built by `cargo build --workspace --bins` before the test run;
`CARGO_BIN_EXE_*` exists only inside the owning package).

Implementation waves (WBS.md). All signatures exist as compiling stubs after wave 0, so a package may *compile*
against any crate; it may *rely on the behaviour* only of packages from earlier waves.

| Wave | Crates implemented |
|---|---|
| W0 | all stubs; `snp-core` (complete); `snp-pycompat` (prototype adapted to the §1.4.2 signatures, regex-free, incl. the `precommit` composites); working baselines (exact ports, replaced or completed in W1 without signature change) of `snp_proc::spawn::{capture, stream, inherit}`, `resolve::{find_executable, normexe, normalize_cmd}`, `xargs::{cpu_count, target_concurrency, platform_max_length, partition}`, `snp_os::lock::FileLock` and `snp_os::fs::{write_new_durable, atomic_replace}`, so W1 packages (snp-git, snp-lang framework) can rely on them; `snp` entry/dispatch skeleton; xtask (`check-dag`, `check-unsafe`, `lint-guards`, `oracle`) |
| W1 | snp-pycompat (complete), snp-argparse + snp-cli, snp-pyre, snp-yaml, snp-os + snp-proc + snp-probe, snp-net + snp-lang framework/env-less backends, snp-pyparsers + snp-hooks-data, snp-git + snp-testkit |
| W2 | snp-config, snp-store + snp-files, snp-stash, snp-lang group 1 (python, node, golang, rust, ruby), snp-lang group 2 (docker, conda, coursier, dart, dotnet, haskell, julia, lua, perl, r, swift), snp-hooks A (framework, pygrep, fail, hazmat, builtin, P0), snp-hooks B (P1/P2/caveat hooks) |
| W3 | snp-repo, snp-run, snp-cmd + snp binary, snp-migrate, snp-difftest + corpus, performance gates, platform CI/release/docs |

`xtask check-dag` (CI) enforces the edge list above (normal, build and dev dependencies; no edge may be added without an
ADR) and that no crate depends on `snp` or `snp-difftest`. `snp-run` uses `snp-repo` only through the `Resolve` trait and plain data, so
its tests run with a fake resolver before snp-repo lands. Critical path: pycompat → pyre/yaml → config → repo → run → snp.

### 1.3 Workspace-wide rules

* **Lints** (root `Cargo.toml`, 07 §4.2 with these deltas): `[workspace.lints.rust] unsafe_code = "forbid"`,
  `missing_debug_implementations = "warn"`, `unreachable_pub = "warn"`, `unused_qualifications = "warn"`,
  `non_ascii_idents = "deny"`, `rust_2018_idioms = "warn"`; `[workspace.lints.clippy]` `all`/`pedantic`/`nursery`/
  `cargo` = warn (noise allowed per 07 §4.2) and **deny**: `unwrap_used, expect_used, panic, todo, unimplemented,
  unreachable, indexing_slicing, string_slice, arithmetic_side_effects, unchecked_time_subtraction, dbg_macro,
  mem_forget, exit, print_stdout, print_stderr, iter_over_hash_type, allow_attributes, allow_attributes_without_reason,
  get_unwrap, unwrap_in_result, panic_in_result_fn, disallowed_methods, disallowed_macros`. Arithmetic uses
  `checked_*`/`saturating_*`/`wrapping_*`; a proven-bounded site may carry `#[expect(clippy::arithmetic_side_effects,
  reason = "<bound>")]`. `clippy.toml`: `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`,
  `allow-panic-in-tests = true`, `allow-indexing-slicing-in-tests = true`; `disallowed-macros`: `assert`, `assert_eq`,
  `assert_ne` (non-test code returns errors; `debug_assert*` expands to these macros, so a non-test `debug_assert*`
  carries `#[expect(clippy::disallowed_macros, reason = "<invariant>")]`, ADR-081); `disallowed-methods`: `std::env::set_var`,
  `std::env::remove_var`, `std::env::set_current_dir`, `std::env::home_dir`, `std::env::var`, `std::env::var_os`,
  `std::env::vars_os` (only `snp_core::env::Env::capture_process` may read the process environment, via an
  `#[expect]`), `std::fs::canonicalize` (use `snp_os::realpath`), `std::process::exit` (allowed only in
  `crates/snp/src/main.rs`, via `#[expect(clippy::disallowed_methods, reason = "…")]` — clippy's `exit` lint never
  fires inside `main`, ADR-081), and the panicking std APIs (ADR-066):
  `Duration::{from_secs_f64, from_secs_f32, mul_f64, mul_f32, div_f64, div_f32}` (use `try_from_secs_f64` + an upper
  bound), slice `split_at`/`split_at_mut`/`copy_from_slice`/`clone_from_slice`/`swap`/`chunks`/`chunks_exact`/
  `rchunks`/`windows`/`repeat`, `Iterator::step_by`, `Vec::{remove, insert, drain, split_off, swap_remove,
  with_capacity}`, `String::{insert, insert_str, remove, drain, split_off, truncate, with_capacity}`, `str::repeat`,
  `RefCell::{borrow, borrow_mut}`, integer `pow`/`abs`/`div_euclid`/`rem_euclid`, `std::thread::spawn` and
  `std::thread::Scope::spawn` (use `std::thread::Builder::spawn_scoped`/`spawn` and map the `io::Error` to a
  `Failure`; every scoped handle is joined explicitly, so `thread::scope` never panics). A site whose precondition is
  proven carries `#[expect(clippy::disallowed_methods, reason = "<proof>")]`. WP-00 verifies that every listed path
  resolves (clippy warns on an unresolvable path; `-Dwarnings` makes it an error). CI builds with
  `RUSTFLAGS=-Dwarnings`: `cargo clippy --workspace --lib --bins` with the full set, and `cargo clippy --workspace
  --tests --benches --examples -- -A clippy::disallowed_macros -A clippy::arithmetic_side_effects` for test code.
* **W0 stubs.** Each stub lives in a module file that has exactly one owning package (WBS), and that file starts with
  the inner attribute `#![expect(clippy::todo, reason = "W0 stub, removed by WP-xx")]` (never a crate-level attribute
  in a `lib.rs` shared by several owners). When the owner removes the file's last `todo!()` the expectation becomes
  unfulfilled and CI fails until the owner deletes the attribute — stubs cannot survive a package's completion. A
  function returning `impl Iterator` (RPIT) is stubbed as `core::iter::from_fn(|| todo!())` (a bare `todo!()` does not
  compile: ``E0277: `!` is not an iterator``); every other stub body is `todo!()`.
* **Profiles**: `release`: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, **`panic = "unwind"`** (load-bearing,
  FD-5), `overflow-checks = true`, `debug = "line-tables-only"`, `strip = "debuginfo"`; `dist` inherits release +
  `strip = "symbols"`; `profiling` = release + `lto = "thin"` + `debug = "full"`; `perf` = release + `lto = "thin"` +
  `codegen-units = 16` (the `perf-smoke` CI job's test binaries only, ADR-081). Dev profile: `opt-level = 1` for
  `regex-automata`, `fancy-regex`, `libyaml-safer`, `blake3`. System allocator on gnu, macOS and Windows (mimalloc
  measured: +RSS, no gain, 07 §3.15). musl: decided by measurement (07 D10: musl's allocator is slow under
  contention): WP-36 benches the musl build with the system allocator and with mimalloc behind
  `#[cfg(target_env = "musl")] #[global_allocator]` in `crates/snp` on B3/B4/B6/B12, and mimalloc ships for musl
  targets iff it wins (ADR-079).
* **Dependencies** (complete `[workspace.dependencies]`, exact versions from 07 §6 unless noted; WP-00 owns the root
  manifest and nobody edits it later — a package needing a new external dependency files an ADR and the integration
  lead adds it): `cfgv` (path `../cfgv-rs`, `version = "0.3"`, `default-features = false`), `file-identify` (path
  `../file-identify`, `version = "0.5"`, `default-features = false`) — the sibling checkouts are pinned by the
  committed `deps.lock` (commit SHAs) that every workflow checks out (ADR-061), `libyaml-safer 0.3.0`, `regex 1.13.1`,
  `regex-syntax 0.8.11`, `fancy-regex 0.19.2`, `unicode-ident 1`, `unicode_names2 =4.0.0` (feature `py-re-names` of
  snp-pyre, default on), `memchr 2.8.3`, `bstr 1.13.1`, `indexmap 2.14.2`, `foldhash 0.2.0`, `smallvec 1.16.2`,
  `blake3 1.8.7`, `sha2 0.11.0`, `md-5 0.11.0`, `serde 1.0.229` + `serde_json 1.0.151` (state files and JSON I/O only),
  `toml 1.1.6` (snp-migrate; tests of snp-pyparsers), `thiserror 2.0.21`, `tracing 0.1.44` + `tracing-subscriber
  0.3.23`, `ureq 3.4.2` (rustls + platform-verifier + gzip), `flate2 1.1.10`, `tar 0.4.46`, `zip 8.6.0`,
  `liblzma 0.4.8` (feature `xz`, Unix node tarballs only), `tempfile 3.27.0`, `same-file 1.0.6`, `globset 0.4.20`,
  `roxmltree 0.21` (dotnet nuspec), `rustix 1.1.5` (unix: `fs process pty termios thread stdio event`), `signal-hook
  0.3` (unix; `SignalsInfo<WithOrigin>`), `ctrlc 3.5` (windows, feature `termination`), `winapi-util 0.1.11` (windows:
  console VT), `process-wrap 10.0.1` (windows only: `std`, `job-object`), `mimalloc` (musl targets of `crates/snp`
  only, iff ADR-079's measurement adopts it); dev: `insta 1.49`, `proptest 1.11`, `rstest 0.27`, `criterion 0.8.2`,
  `pretty_assertions 1.4.1`, `dhat 0.3` (bench feature); build-time only (`xtask`): `clap 4.6.7` (default
  features off: its `color` feature pulls the banned `anstyle-query`; ADR-081), `clap_complete 4.6.11`,
  `clap_mangen 0.3.3`, `syn 2` (lint-guards), `anyhow 1`.
* **Banned** (`deny.toml`): `tokio`, `async-std`, `rayon`, `clap` outside `xtask`, `which`, `shlex`, `shell-words`,
  `serde_yaml*`, `serde_yml`, `yaml-rust*`, `rusqlite`, `openssl*`, `atty`, `mimalloc` (except with `wrappers =
  ["snp"]` for the musl allocator), `regex` outside `snp-pyre` (wrappers; `snp-pycompat` is regex-free), `memmap2`,
  `anstyle-query`
  (Windows colour goes through `winapi-util`, ADR-029), `unicode-width` (EAW comes from pycompat tables), `nix`.
  `sources`: crates.io + the two user path crates only. Licenses allow-list: MIT, Apache-2.0, BSD-2/3, ISC,
  Unicode-3.0, Zlib, MPL-2.0 (file-level).
* **Output discipline**: no crate prints. Only `snp_core::output::Printer` writes stdout/stderr; lower crates return
  data or receive `&Printer`/`&dyn WarnSink`/`&dyn DiagSink`. Children write to inherited fds only where pre-commit
  does (legacy hooks, show-diff, hazmat children).
* **Messages**: every user-visible string lives in a `messages.rs` of the emitting crate, takes `Brand` when it names
  the tool, and has golden tests for both brands (compat = 4.6.2 bytes; snp = the `brand_map` substitution for
  non-argparse texts and the SNP_SPEC goldens rendered with prog `snp` for argparse texts, §9.6, ADR-058).
* **API documentation**: every public item names the Python/pre-commit source it reproduces (`run.py:142-235`).
* **Contract tests**: every crate has `tests/api.rs` (WP-00) that references every public signature with explicit
  types; signature drift breaks compilation. `Send + Sync` static assertions for every type documented as such.
* **Conventions for §1.4**: all public enums are `#[non_exhaustive]` unless marked `// exhaustive`; types are
  `Send + Sync` unless noted (`snp_core::spawnlock::SpawnGuard` is `!Send`, and `snp_pycompat::ospath::HomeLookup`
  is `!Send + !Sync` (borrowed `&dyn Fn` lookups that live for one `expanduser` call); both are excluded from the
  `api.rs` Send assertions); `Env`, `Printer`, `CancelToken`, `Git` are cheap to clone (`Arc` inside); `PyRegex` is
  not `Clone` — compiled patterns are shared as `Arc<PyRegex>` (from `RegexCache::get`; ADR-081). Every public type
  implements `Debug` (`missing_debug_implementations` + `-Dwarnings`): listings omit `#[derive(Debug)]` on plain
  structs for brevity, WP-00 adds it; manual impls are noted (`Printer`, `pygrep::Compiled`). Every seam trait has
  `std::fmt::Debug` as a supertrait (`WarnSink`, `DiagSink`, `Clock`, `CmdRunner`, `ToolDirs`, `Fetch`, `Resolve`,
  `Preflight`, `GitBackend`, `DoctorCheck`), so context structs holding `&dyn Trait` derive `Debug`.
  Bodies, private items and private fields (`/* … */`) are the implementer's choice. `impl Trait for T {}` in a listing
  means "T implements Trait" (bodies elided); `pub const X: T;` / `pub static X: T;` mean the item exists with that type.

### 1.4 Crate APIs (normative)

#### 1.4.1 `snp-core` (implemented in W0 by WP-00)

```rust
#![forbid(unsafe_code)]
pub mod stage {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] #[repr(u8)]
    pub enum HookType { CommitMsg, PostCheckout, PostCommit, PostMerge, PostRewrite,
                        PreCommit, PreMergeCommit, PrePush, PreRebase, PrepareCommitMsg }      // exhaustive
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum ArgCount { Exact(u8), Range(u8, u8) }                                             // exhaustive
    impl HookType {
        pub const ALL: [HookType; 10];                      // HOOK_TYPES source order (01 §1.2)
        pub const fn as_str(self) -> &'static str;
        pub fn parse(s: &str) -> Option<Self>;              // exact; no legacy aliases (01 §1.3)
        pub const fn arg_count(self) -> ArgCount;           // hook_impl._check_args_length (02 §16.2)
        pub const fn stage(self) -> Stage;
    }
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub enum Stage { Hook(HookType), Manual }                                                   // exhaustive
    impl Stage {
        pub const ALL: [Stage; 11];                         // STAGES source order, manual last
        pub const fn as_str(self) -> &'static str;
        pub fn parse(s: &str) -> Option<Self>;              // canonical names only
        pub fn transform_legacy(s: &str) -> &str;           // commit→pre-commit, push→pre-push, merge-commit→pre-merge-commit
        pub fn parse_with_legacy(s: &str) -> Option<(Self, bool /* was legacy */)>;
    }
    #[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Debug)]
    pub struct StageSet(/* u16 */);
    impl StageSet {
        pub const ALL: Self; pub const EMPTY: Self;
        pub const fn with(self, s: Stage) -> Self; pub const fn contains(self, s: Stage) -> bool;
        pub const fn is_empty(self) -> bool; pub fn iter(self) -> impl Iterator<Item = Stage>;
    }
}
pub mod lang {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)] #[repr(u8)]
    pub enum LanguageId { Conda, Coursier, Dart, Docker, DockerImage, Dotnet, Fail, Golang, Haskell, Julia, Lua,
                          Node, Perl, Pygrep, Python, R, Ruby, Rust, Swift, Unsupported, UnsupportedScript } // exhaustive
    impl LanguageId {
        pub const ALL: [Self; 21];                          // == pre-commit language_names (sorted, 01 §1.4)
        pub const fn as_str(self) -> &'static str;
        pub fn from_config(s: &str) -> Option<Self>;        // + aliases system→Unsupported, script→UnsupportedScript
        pub const fn env_dir_base(self) -> Option<&'static str>; // "py_env", "node_env", …; None = env-less
    }
}
pub mod brand {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub enum Brand { PreCommit, Snp }                                                           // exhaustive
    impl Brand {
        /// basename(argv[0]) minus a trailing ".exe" (ASCII-ci, Windows only) == "pre-commit" (exact on Unix,
        /// ASCII-case-insensitive on Windows) ⇒ PreCommit. Never `current_exe()` (Linux resolves symlinks). No env override.
        pub fn detect(arg0: Option<&std::ffi::OsStr>) -> Self;
        pub const fn prog(self) -> &'static str;            // "pre-commit" | "snp"
        pub fn version_line(self) -> String;                // "pre-commit 4.6.2" | "snp 2.0.0" (ADR-023)
        pub const fn crash_log_name(self) -> &'static str;  // "pre-commit.log" | "snp.log" (ADR-013)
    }
}
pub mod profile {
    pub const SNP_VERSION: &str = env!("CARGO_PKG_VERSION");
    pub const PRE_COMMIT: &str = "4.6.2";                   // minimum_pre_commit_version comparisons, compat --version
    pub const ARGPARSE_PY: &str = "3.13.14";
    pub const RE_GRAMMAR_PY: &str = "3.12";
    pub const RE_UNICODE: &str = "16.0.0";
    pub const TEXT_UNICODE: &str = "15.1.0";
    pub const PCH_PY: &str = "3.13";
    pub const IDENTIFY: &str = "2.6.20";
    pub const BUG_URL: &str = "https://github.com/grok-rs/snp/issues";   // snp-only texts (panic hint, crash log, snp brand)
    pub const LOCAL_TEMPLATE_VERSION: u32 = 1;              // in local repo keys (snp-store); re-exported by snp-lang
}
pub mod env {
    use std::{borrow::Cow, ffi::{OsStr, OsString}, path::PathBuf};
    /// Immutable environment snapshot. Windows: keys compare ASCII-case-insensitively, first spelling kept.
    #[derive(Clone, Debug)] pub struct Env(/* Arc<EnvInner> */);
    impl Env {
        /// ONLY called by `snp::entry`; drops `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE` (main.py:39-42).
        pub fn capture_process() -> Env;
        pub fn from_pairs<K: Into<OsString>, V: Into<OsString>>(pairs: impl IntoIterator<Item = (K, V)>) -> Env;
        pub fn get(&self, key: &str) -> Option<&OsStr>;
        pub fn get_nonempty(&self, key: &str) -> Option<&OsStr>;   // Python `os.environ.get(k) or …`
        pub fn is_present(&self, key: &str) -> bool;               // presence semantics (PRE_COMMIT_NO_CONCURRENCY, TRAVIS)
        pub fn is_truthy_flag(&self, key: &str) -> bool;           // non-empty and not "0"/"false" (CI, SNP_* flags)
        pub fn patched(&self, patch: &EnvPatch) -> Env;            // envcontext: every Var reads the PRE-patch snapshot
        pub fn without(&self, keys: &[&str]) -> Env;
        pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)>;
        pub fn python_environ_size(&self) -> usize;                // 8*n + Σ(len k + len v + 2) (xargs.py:41-46)
        pub fn apply_to(&self, cmd: &mut std::process::Command);   // env_clear() + envs()
    }
    #[derive(Clone, Debug)] pub enum Part { Lit(OsString), Var { name: Cow<'static, str>, default: Cow<'static, str> } }
    #[derive(Clone, Debug)] pub enum EnvOp { Set(OsString), Unset, Template(Box<[Part]>) }
    #[derive(Clone, Debug, Default)] pub struct EnvPatch(/* Vec<(Cow<'static, str>, EnvOp)> */);
    impl EnvPatch {
        pub fn set(self, k: impl Into<Cow<'static, str>>, v: impl Into<OsString>) -> Self;
        pub fn unset(self, k: impl Into<Cow<'static, str>>) -> Self;
        /// `(d1, PATHSEP, …, Var(key))`; omits the trailing separator when `key` is unset or empty (D-SEC-07).
        pub fn prepend_path(self, key: &'static str, dirs: impl IntoIterator<Item = PathBuf>) -> Self;
        pub fn template(self, k: impl Into<Cow<'static, str>>, parts: Vec<Part>) -> Self;
        pub fn extend(self, other: EnvPatch) -> Self;
        pub fn is_empty(&self) -> bool;
    }
}
pub mod cancel {
    /// `fail_fast` is a scheduler decision, never a cancellation (ADR-065); `Panic` is set by the panic hook.
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum CancelReason { Interrupt, Terminate, Hangup, StdoutClosed, Panic }
    #[derive(Clone, Debug)] pub struct CancelToken(/* Arc<Inner: AtomicU8 reason + Mutex<()> + Condvar> */);
    impl CancelToken {
        pub fn new() -> Self;
        /// First reason wins (atomic compare-exchange); waiters are woken with a `try_lock` + notify, so `cancel` never
        /// blocks and is safe from the signal thread and from the panic hook.
        pub fn cancel(&self, r: CancelReason);
        pub fn reason(&self) -> Option<CancelReason>;
        pub fn is_cancelled(&self) -> bool;
        pub fn wait_timeout(&self, d: std::time::Duration) -> bool;   // true if cancelled
        pub fn child(&self) -> CancelToken;                 // cancelled with parent; own cancels don't propagate up
    }
    impl Default for CancelToken {}
}
pub mod failure {
    use std::{borrow::Cow, ffi::{OsStr, OsString}};
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ExcKind { Fatal, Unexpected }      // exhaustive
    /// A Python-style exception as pre-commit's error_handler prints it (13 X.1).
    #[derive(Debug, Clone)] pub struct PyExc {
        pub ty: Cow<'static, str>,          // unqualified class: CalledProcessError, FileNotFoundError, FatalError, …
        pub text: Vec<u8>,                  // bytes(exc) / str(exc).encode(); may span lines
        pub hint: Option<String>,           // K+ recovery line → stderr after line 2 (13 X.3, ADR-045)
        pub chain: Vec<String>,             // crash-log only: source chain
        pub backtrace: Option<String>,      // crash-log only: PanicException
    }
    impl PyExc {
        pub fn new(ty: impl Into<Cow<'static, str>>, text: impl Into<Vec<u8>>) -> Self;
        pub fn with_hint(self, hint: impl Into<String>) -> Self;
        pub fn panic(payload: &str, location: Option<&str>, backtrace: Option<String>) -> Self;  // ty "PanicException"
    }
    #[derive(Debug)] pub enum Failure {
        /// line 1 "An error has occurred: {ty}: {text}" (Fatal, exit 1) / "An unexpected error has occurred: …" (exit 3)
        Exc { kind: ExcKind, exc: PyExc },
        Interrupted(crate::cancel::CancelReason),   // Interrupt: banner + exit 130; Terminate/Hangup: no banner, die by signal
        StdoutClosed,                               // 120, silent (X13-EH-03)
        SystemExit { code: i32, stderr: Vec<u8> },  // `raise SystemExit(msg)`: stderr (text mode), exit code
        Usage { stderr: String },                   // argparse: stderr, exit 2
        Exit(i32),                                  // silent exit with code (hazmat child status, hook-impl legacy|run)
    }
    impl Failure {
        pub fn fatal(ty: &'static str, text: impl Into<Vec<u8>>) -> Self;
        pub fn unexpected(ty: impl Into<Cow<'static, str>>, text: impl Into<Vec<u8>>) -> Self;
        pub fn from_io(e: &std::io::Error, f1: Option<&OsStr>, f2: Option<&OsStr>) -> Self;  // OSError family, exit 3
        pub fn exit_code(&self) -> i32;             // 1 / 3 / 130 / 143 / 129 / 120 / code / 2 / n
        pub fn writes_crash_log(&self) -> bool;     // Exc only
    }
    pub type SnpResult<T> = Result<T, Failure>;
    pub mod pyerr {
        /// Python OSError subclass name for an io::Error (FileNotFoundError, PermissionError, IsADirectoryError, …).
        pub fn os_error_class(e: &std::io::Error) -> &'static str;
        /// str(OSError): "[Errno 2] No such file or directory: 'x'" / Windows "[WinError N] …" (13 X.2.1).
        pub fn os_error_text(e: &std::io::Error, f1: Option<&std::ffi::OsStr>, f2: Option<&std::ffi::OsStr>) -> String;
        /// pre_commit.util.CalledProcessError bytes (format A, 13 X.2.2).
        pub fn called_process_a(argv: &[std::ffi::OsString], rc: i64, stdout: Option<&[u8]>, stderr: Option<&[u8]>) -> Vec<u8>;
        /// subprocess.CalledProcessError str (format B; hook_impl rev-list calls).
        pub fn called_process_b(argv: &[std::ffi::OsString], rc: i64) -> String;
    }
}
pub mod output {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ColorSetting { Auto, Always, Never }  // exhaustive
    impl ColorSetting { pub fn parse(v: &std::ffi::OsStr) -> Option<Self>; }   // exact, case-sensitive
    #[derive(Copy, Clone, Debug)] pub enum Color { Red, Green, Yellow, Turquoise, Subtle }    // bytes of 02 §4
    #[derive(Copy, Clone, Debug)] pub enum Level { Info, Warning, Error }
    #[derive(Clone, Debug)] pub struct PrinterOpts {
        pub brand: crate::brand::Brand, pub color: bool, pub stdout_is_tty: bool,
        pub windows_text_mode: bool,                // CRLF for py_print/py_eprint (true on Windows)
        pub cancel: crate::cancel::CancelToken,
    }
    /// The only writer of user-facing stdout/stderr. Never panics, never returns an error: EPIPE latches `closed`
    /// and cancels the token with StdoutClosed. Poison-proof (S14). Reproduces CPython's two stdout layers (§3.9).
    /// `Debug` is implemented manually (the boxed writers are not `Debug`). The fault point `PrinterWrite` (feature
    /// `fault-injection`) fires while the state lock is held (S14 test).
    pub struct Printer { /* Mutex<State{ stdout, stderr, pending_text: String, closed }> + opts */ }
    impl Printer {
        pub fn new(stdout: Box<dyn std::io::Write + Send>, stderr: Box<dyn std::io::Write + Send>, o: PrinterOpts) -> Self;
        pub fn stdio(o: PrinterOpts) -> Self;
        pub fn brand(&self) -> crate::brand::Brand;  pub fn color(&self) -> bool;
        pub fn write(&self, bytes: &[u8]);              // output.write: (flush pending text) sys.stdout.buffer.write + flush
        pub fn write_line(&self, bytes: &[u8]);         // output.write_line(_b)
        pub fn log(&self, level: Level, msg: &str);     // logging handler: "[WARNING] msg" (+ colours, incl. "[INFO]\x1b[m")
        pub fn py_print(&self, s: &str);                // print(): text layer (8192-byte pending buffer when piped)
        pub fn py_eprint(&self, s: &str);               // print(file=sys.stderr) / SystemExit / argparse to stderr
        pub fn write_stderr(&self, bytes: &[u8]);       // raw stderr bytes
        pub fn diag(&self, s: &str);                    // snp-only diagnostics → stderr, never stdout
        pub fn paint<'a>(&self, text: &'a str, c: Color) -> std::borrow::Cow<'a, str>;
        pub fn flush(&self);                            // before handing stdout to an inheriting child
        pub fn finish(&self);                           // interpreter-exit flush order (pending text last)
        pub fn is_closed(&self) -> bool;
    }
    pub trait WarnSink: Send + Sync + std::fmt::Debug { fn warn(&self, msg: &str); }   // `[WARNING] msg` lines on stdout, in order
    pub trait DiagSink: Send + Sync + std::fmt::Debug { fn diag(&self, msg: &str); }   // stderr diagnostics
    impl WarnSink for Printer {}  impl DiagSink for Printer {}
    #[derive(Debug, Default)] pub struct CollectWarnings(/* Mutex<Vec<String>> */);   // tests + bare replay (meta hooks)
    impl CollectWarnings { pub fn new() -> Self; pub fn take(&self) -> Vec<String>; }
    impl WarnSink for CollectWarnings {}  impl DiagSink for CollectWarnings {}
}
pub mod path {
    /// Repo-relative, '/'-separated path as raw bytes (Unix may be non-UTF-8). Rejects NUL, absolute, empty.
    #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)] pub struct RepoPath(/* Box<[u8]> */);
    impl RepoPath { pub fn new(b: &[u8]) -> Option<Self>; pub fn as_bytes(&self) -> &[u8];
        pub fn to_os(&self) -> std::borrow::Cow<'_, std::ffi::OsStr>; pub fn lossy(&self) -> std::borrow::Cow<'_, str>;
        pub fn is_contained(&self) -> bool; }       // no '..' component (native writes, `root:` extension)
}
pub mod clock {
    pub trait Clock: Send + Sync + std::fmt::Debug { fn now(&self) -> std::time::Instant; fn unix_secs(&self) -> u64; }
    #[derive(Debug, Default)] pub struct SystemClock; impl Clock for SystemClock {}
}
pub mod sync {   // S14: every safety-path lock goes through these (recover PoisonError::into_inner)
    pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T>;
    pub fn read<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockReadGuard<'_, T>;
    pub fn write<T>(l: &std::sync::RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T>;
}
pub mod spawnlock {  // S15 (ADR-037): no-op on Linux and Windows; a process-wide Mutex on macOS/other Unix
    /// `!Send` (a `MutexGuard`): held only within one thread; excluded from the `api.rs` Send assertions.
    #[derive(Debug)] pub struct SpawnGuard(/* Option<MutexGuard<'static, ()>> */);
    /// Held across every `Command::spawn` and every non-atomic-CLOEXEC fd creation (pipe, pty master and slave).
    pub fn guard() -> SpawnGuard;
}
/// Test-only fault injection (ADR-066). Always compiled; `hit` is an inlined no-op unless the crate feature
/// `fault-injection` is on (enabled only by `crates/snp`'s `testing` feature for FI/panic tests, never in releases).
pub mod fault {
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub enum Point { PrinterWrite, WorkerPartition, StashState(u8), RestoreCommand(u8), InstallCommit, StoreMaterialize }
    /// `SNP_FAULT=<point>[@n]` (n-th hit, default 1); read once by `snp::entry` from the captured Env.
    pub fn arm_from_env(env: &crate::env::Env);
    /// Panics with payload `snp fault injection: <point>` on the armed hit; otherwise does nothing.
    pub fn hit(p: Point);
}
```

#### 1.4.2 `snp-pycompat` (W0 vendors the prototype adapted to these signatures; WP-11 completes)

This listing is the frozen production contract (ADR-056); it supersedes the pseudo-signatures of 15 §A6 and the
simpler prototype signatures (`docs/research/pycompat-proto`), which W0 adapts (thin wrappers where needed; the regex-
free `rev_line_match_manual` becomes `rev_line_match`). The crate has **no dependencies** besides std (no `regex`: it
sits under every crate, and `regex` lives only in snp-pyre); the Unicode tables are generated by
`xtask/oracle/gen_pycompat_tables.py` under CPython 3.13 (UCD 15.1, ADR-020) into `src/generated/`. No public
function returns `impl Trait`: iterators are named types implementing `Iterator` (+ `DoubleEndedIterator`,
`FusedIterator`, `Clone` where cheap). Every error type implements `Display` = Python's `str(exc)`, `std::error::Error`,
and `py_type()` = the Python class for crash lines. Every function names the Python expression it reproduces.

```rust
//! crates/snp-pycompat — CPython 3.12 stdlib semantics (+ 3.13 delta), verified by committed vectors (15 §A1–§A7).
use std::{borrow::Cow, ffi::{OsStr, OsString}, io, path::{Path, PathBuf}};

/// Python `ValueError` texts of `os.path` (both flavours).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError { NoPathSpecified, EmptySequence, MixAbsRel, DifferentDrives,
                     DifferentMounts { path_drive: String, start_drive: String } }
impl PathError { pub const fn py_type(&self) -> &'static str; }             // "ValueError"
impl std::fmt::Display for PathError {}  impl std::error::Error for PathError {}

pub mod chars {
    pub const UNICODE_VERSION: &str = "15.1.0";             // generated table version (CPython 3.13)
    pub const fn is_space(c: char) -> bool;                 // PY-WS: str.isspace (incl. U+001C–U+001F)
    pub const fn is_linebreak(c: char) -> bool;             // PY-LB (str.splitlines boundaries; caller merges "\r\n")
    pub const fn is_bytes_space(b: u8) -> bool;             // B-WS: bytes.isspace
    pub fn is_printable(c: char) -> bool;                   // str.isprintable (binary search, generated table)
    pub fn decimal_value(c: char) -> Option<u32>;           // unicodedata.decimal
    pub const PY_STR_SPACE_CLASS: &str;                     // regex class body for Python `\s` (data for snp-pyre)
    /// One Python character: a scalar value or a lone surrogate (surrogateescape byte U+DC80..U+DCFF, Windows WTF-16).
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub enum PyChar { Scalar(char), Surrogate(u16) }  // exhaustive
}
pub mod text {
    /// `str.splitlines()` / `splitlines(True)`: zero-copy, Unicode line boundaries.
    #[derive(Clone, Debug)] pub struct SplitLines<'a> { /* … */ }
    impl<'a> Iterator for SplitLines<'a> { type Item = &'a str; }  impl DoubleEndedIterator for SplitLines<'_> {}
    impl std::iter::FusedIterator for SplitLines<'_> {}
    /// `bytes.splitlines()` / `splitlines(True)` (\n, \r, \r\n only).
    #[derive(Clone, Debug)] pub struct BytesSplitLines<'a> { /* … */ }
    impl<'a> Iterator for BytesSplitLines<'a> { type Item = &'a [u8]; }  impl std::iter::FusedIterator for BytesSplitLines<'_> {}
    /// `str.split(maxsplit=n)` (whitespace split).
    #[derive(Clone, Debug)] pub struct SplitWs<'a> { /* … */ }
    impl<'a> Iterator for SplitWs<'a> { type Item = &'a str; }  impl std::iter::FusedIterator for SplitWs<'_> {}
    pub fn splitlines(s: &str) -> SplitLines<'_>;
    pub fn splitlines_keepends(s: &str) -> SplitLines<'_>;
    pub fn bytes_splitlines(b: &[u8]) -> BytesSplitLines<'_>;
    pub fn bytes_splitlines_keepends(b: &[u8]) -> BytesSplitLines<'_>;
    pub fn split_whitespace(s: &str, maxsplit: Option<usize>) -> SplitWs<'_>;
    pub fn rsplit_whitespace(s: &str, maxsplit: Option<usize>) -> Vec<&str>;          // str.rsplit(maxsplit=n), left-to-right
    pub fn bytes_rsplit_whitespace(b: &[u8], maxsplit: Option<usize>) -> Vec<&[u8]>;  // bytes.rsplit (ASCII whitespace)
    /// `a, b, c, d = s.rsplit(maxsplit=3)` (pre-push, no allocation). Display = "not enough values to unpack (expected 4, got N)".
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub struct UnpackError { pub expected: usize, pub got: usize }
    impl UnpackError { pub const fn py_type(&self) -> &'static str; }                  // "ValueError"
    impl std::fmt::Display for UnpackError {}  impl std::error::Error for UnpackError {}
    pub fn rsplit4(s: &str) -> Result<[&str; 4], UnpackError>;
    pub fn bytes_rsplit4(b: &[u8]) -> Result<[&[u8]; 4], UnpackError>;               // D-CRASH-11 bytes path
    pub fn strip(s: &str) -> &str;  pub fn lstrip(s: &str) -> &str;  pub fn rstrip(s: &str) -> &str;
    pub fn strip_chars<'a>(s: &'a str, set: &str) -> &'a str;   pub fn lstrip_chars<'a>(s: &'a str, set: &str) -> &'a str;
    pub fn rstrip_chars<'a>(s: &'a str, set: &str) -> &'a str;
    pub fn bytes_strip(b: &[u8]) -> &[u8];  pub fn bytes_lstrip(b: &[u8]) -> &[u8];  pub fn bytes_rstrip(b: &[u8]) -> &[u8];
    pub fn bytes_strip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8];
    pub fn bytes_lstrip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8];
    pub fn bytes_rstrip_set<'a>(b: &'a [u8], set: &[u8]) -> &'a [u8];
    pub fn strip_fsbytes(b: &[u8]) -> &[u8];   // os.fsdecode(b).strip(): UTF-8-aware PY-WS, undecodable bytes are non-space
    pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, &'a str);
}
pub mod codec {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Utf8Reason { InvalidStartByte, InvalidContinuationByte, UnexpectedEndOfData }
    /// CPython's `UnicodeDecodeError` for codec 'utf-8' (maximal invalid subpart).
    #[derive(Clone, Debug, PartialEq, Eq)] pub struct Utf8DecodeError { pub start: usize, pub end: usize, pub first_byte: u8, pub reason: Utf8Reason }
    impl Utf8DecodeError { pub const fn py_type(&self) -> &'static str; }             // "UnicodeDecodeError"
    impl std::fmt::Display for Utf8DecodeError {}  impl std::error::Error for Utf8DecodeError {}
    /// "'utf-8' codec can't encode character '\udcff' in position N: surrogates not allowed".
    #[derive(Clone, Debug, PartialEq, Eq)] pub struct Utf8EncodeError { pub position: usize, pub surrogate: u16 }
    impl Utf8EncodeError { pub const fn py_type(&self) -> &'static str; }             // "UnicodeEncodeError"
    impl std::fmt::Display for Utf8EncodeError {}  impl std::error::Error for Utf8EncodeError {}
    pub fn decode_utf8(b: &[u8]) -> Result<&str, Utf8DecodeError>;                   // bytes.decode(); BOM kept
    pub fn decode_utf8_backslashreplace(b: &[u8]) -> Cow<'_, str>;
    /// `os.fsdecode(b)` as Python characters (surrogateescape view; POSIX).
    #[derive(Clone, Debug)] pub struct FsDecodeChars<'a> { /* … */ }
    impl Iterator for FsDecodeChars<'_> { type Item = crate::chars::PyChar; }
    pub fn fsdecode_chars(b: &[u8]) -> FsDecodeChars<'_>;
}
pub mod repr {
    pub fn repr_str(s: &str) -> String;                          // repr(str), pre-sized len + 2
    pub fn write_repr_str(out: &mut String, s: &str);
    pub fn repr_fsbytes(b: &[u8]) -> String;                     // repr(os.fsdecode(b)) (POSIX)
    pub fn repr_os(s: &OsStr) -> String;                         // Unix: repr_fsbytes; Windows: WTF-16, lone surrogates \udXXX
    pub fn repr_str_tuple<S: AsRef<str>>(xs: &[S]) -> String;    // repr(tuple of str), incl. the 1-tuple comma
    pub fn repr_str_list<S: AsRef<str>>(xs: &[S]) -> String;
    #[derive(Clone, Debug, PartialEq, Eq)] pub enum PyInt { Small(i128), Decimal(Box<str>) }   // exhaustive
    /// Value shapes pre-commit formats with repr()/str() (cfgv's own values are rendered by cfgv itself).
    #[derive(Clone, Debug, PartialEq)] pub enum PyValue { None, Bool(bool), Int(PyInt), Float(f64), Str(String),
        List(Vec<PyValue>), Tuple(Vec<PyValue>), Dict(Vec<(PyValue, PyValue)>) }
    pub fn repr(v: &PyValue) -> String;  pub fn write_repr(out: &mut String, v: &PyValue);
    pub use crate::float::repr as repr_float;
    pub fn fstring_eq(name: &str, v: &str) -> String;            // f'{name=}' with a str value
    pub fn oserror_str(errno: i32, strerror: &str, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String;  // [Errno N] s: 'f1' -> 'f2'
    pub fn oserror_from_io(e: &io::Error, f1: Option<&OsStr>, f2: Option<&OsStr>) -> String;          // [Errno N] / [WinError N]
    pub fn stdlib_called_process_error_str(rc: i64, cmd_str: &str) -> String;  // subprocess.CalledProcessError.__str__
    pub const fn signal_name(sig: u32) -> Option<&'static str>;  // "SIGKILL" … for "died with <Signals.SIGKILL: 9>"
    pub fn int_literal_error(literal: &str) -> String;          // "invalid literal for int() with base 10: %.200R"
}
pub mod int {
    #[derive(Clone, Debug, PartialEq, Eq)] pub struct IntParseError { pub literal: String }
    impl IntParseError { pub const fn py_type(&self) -> &'static str; }              // "ValueError"
    impl std::fmt::Display for IntParseError {}  impl std::error::Error for IntParseError {}
    pub fn parse_int(s: &str) -> Result<i128, IntParseError>;                       // int(s), base 10, Nd digits, '_'
    pub fn parse_version(s: &str) -> Result<Vec<i128>, IntParseError>;             // clientlib.parse_version
}
pub mod shlex {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ShlexError { NoClosingQuotation, NoEscapedCharacter }  // exhaustive
    impl ShlexError { pub const fn py_type(&self) -> &'static str; }                 // "ValueError"
    impl std::fmt::Display for ShlexError {}  impl std::error::Error for ShlexError {}
    pub fn quote(s: &str) -> Cow<'_, str>;
    pub fn quote_bytes(b: &[u8]) -> Cow<'_, [u8]>;               // shlex.quote over fsdecode'd bytes (hook scripts, Unix)
    pub fn join<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> String;
    pub fn split(s: &str) -> Result<Vec<String>, ShlexError>;    // posix=True, whitespace_split=True, commenters=''
}
pub mod posixpath {   // CPython 3.12 posixpath on raw bytes; compiled on every OS; lexical unless stated
    pub fn normpath(p: &[u8]) -> Vec<u8>;
    pub fn join(a: &[u8], rest: &[&[u8]]) -> Vec<u8>;
    pub fn split(p: &[u8]) -> (&[u8], &[u8]);
    pub fn dirname(p: &[u8]) -> &[u8];  pub fn basename(p: &[u8]) -> &[u8];
    pub fn splitext(p: &[u8]) -> (&[u8], &[u8]);
    pub fn isabs(p: &[u8]) -> bool;
    pub fn abspath(p: &[u8], cwd: &[u8]) -> Vec<u8>;            // explicit cwd: snp never chdirs
    pub fn relpath(p: &[u8], start: Option<&[u8]>, cwd: &[u8]) -> Result<Vec<u8>, crate::PathError>;
    pub fn commonpath(ps: &[&[u8]]) -> Result<Vec<u8>, crate::PathError>;
    /// `pw_current`: home of the real uid (used only when HOME is unset); `pw_named`: `~user`.
    pub fn expanduser<'a>(p: &'a [u8], home_env: Option<&[u8]>, pw_current: &dyn Fn() -> Option<Vec<u8>>,
                          pw_named: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> Cow<'a, [u8]>;
    #[cfg(unix)] pub fn realpath(p: &[u8], cwd: &[u8]) -> io::Result<Vec<u8>>;   // non-strict _joinrealpath (15 §A4.9)
}
pub mod ntpath {      // CPython 3.12 ntpath, generic over code units (u8 for tests/UTF-8, u16 for OsStr::encode_wide); every OS
    /// A path code unit: `u8` or `u16`.
    pub trait Unit: Copy + Eq + Ord + std::hash::Hash + std::fmt::Debug + Send + Sync + 'static {
        fn from_ascii(b: u8) -> Self;
        fn to_ascii(self) -> Option<u8>;                         // Some for ASCII units
    }
    impl Unit for u8 {}  impl Unit for u16 {}
    pub fn splitroot<U: Unit>(p: &[U]) -> (&[U], &[U], &[U]);
    pub fn splitdrive<U: Unit>(p: &[U]) -> (&[U], &[U]);
    pub fn isabs_312<U: Unit>(p: &[U]) -> bool;  pub fn isabs_313<U: Unit>(p: &[U]) -> bool;
    pub fn normpath<U: Unit>(p: &[U]) -> Vec<U>;
    pub fn join<U: Unit>(a: &[U], rest: &[&[U]]) -> Vec<U>;
    pub fn split<U: Unit>(p: &[U]) -> (Vec<U>, &[U]);           // head is drive + root + stripped dirname
    pub fn dirname<U: Unit>(p: &[U]) -> Vec<U>;  pub fn basename<U: Unit>(p: &[U]) -> &[U];
    pub fn splitext<U: Unit>(p: &[U]) -> (&[U], &[U]);
    pub fn normcase<U: Unit>(p: &[U]) -> Vec<U>;               // ASCII lower + '/'→'\\' (non-ASCII drift accepted, 15 Q5)
    pub fn commonpath<U: Unit>(ps: &[&[U]]) -> Result<Vec<U>, crate::PathError>;
    pub fn abspath_lexical<U: Unit>(p: &[U], cwd: &[U]) -> Vec<U>;   // normpath(join(cwd, p)); ntpath.abspath fallback
    pub fn relpath<U: Unit>(p: &[U], start: Option<&[U]>, cwd: &[U]) -> Result<Vec<U>, crate::PathError>;
    /// USERPROFILE, else HOMEDRIVE + HOMEPATH (3.12 rules); `~user` replaces the last component of the own home.
    pub fn expanduser<'a, U: Unit>(p: &'a [U], env: &dyn Fn(&str) -> Option<Vec<U>>) -> Cow<'a, [U]>;
    #[cfg(windows)] pub fn realpath(p: &OsStr, cwd: &Path) -> io::Result<PathBuf>;   // 15 §A4.10 algorithm
}
/// What every caller uses: posixpath on Unix, ntpath<u16> on Windows, over `OsStr`/`Path` with explicit cwd.
pub mod ospath {
    /// Home lookup injected by snp-os (env snapshot; passwd file for `pw_*` on Unix, unused on Windows).
    /// `!Send + !Sync` (§1.3): built on the calling thread for one `expanduser` call; manual `Debug`.
    pub struct HomeLookup<'a> { pub env: &'a dyn Fn(&str) -> Option<OsString>, pub pw_current: &'a dyn Fn() -> Option<OsString>,
                                pub pw_named: &'a dyn Fn(&OsStr) -> Option<OsString> }
    pub fn normpath(p: &OsStr) -> OsString;
    pub fn join(a: &OsStr, rest: &[&OsStr]) -> OsString;
    pub fn split(p: &OsStr) -> (OsString, OsString);
    pub fn dirname(p: &OsStr) -> OsString;  pub fn basename(p: &OsStr) -> OsString;
    pub fn isabs(p: &OsStr) -> bool;                             // 3.12 semantics
    pub fn abspath(p: &OsStr, cwd: &Path) -> PathBuf;            // Windows: drive-relative paths of another drive via std::path::absolute
    pub fn relpath(p: &OsStr, start: Option<&OsStr>, cwd: &Path) -> Result<OsString, crate::PathError>;
    pub fn commonpath(ps: &[&OsStr]) -> Result<OsString, crate::PathError>;
    pub fn normcase(p: &OsStr) -> OsString;                      // identity on Unix
    pub fn expanduser<'a>(p: &'a OsStr, home: &HomeLookup<'_>) -> Cow<'a, OsStr>;
    pub fn realpath(p: &OsStr, cwd: &Path) -> io::Result<PathBuf>;   // non-strict os.path.realpath
}
pub mod textio {
    pub fn universal_newlines(s: &str) -> Cow<'_, str>;          // open(newline=None).read(): \r\n and \r → \n
    pub fn windows_text_write(s: &str) -> Cow<'_, str>;          // text-mode write on Windows: \n → \r\n
    pub fn textwrap_indent(text: &str, prefix: &str) -> String;  // textwrap.indent, default predicate
}
pub mod precommit {   // composites, each verified by vectors; consumers call these, never re-implement them
    /// git.zsplit: `-z` output with leading/trailing NULs stripped; interior empty records kept; no allocation.
    #[derive(Clone, Debug)] pub struct ZSplit<'a> { /* … */ }
    impl<'a> Iterator for ZSplit<'a> { type Item = &'a [u8]; }  impl std::iter::FusedIterator for ZSplit<'_> {}
    pub fn zsplit(b: &[u8]) -> ZSplit<'_>;
    /// run._get_skips: {s.strip() for s in SKIP.split(',') if s.strip()} via strip_fsbytes; first-seen order, deduplicated.
    pub fn get_skips(skip: Option<&OsStr>) -> Vec<OsString>;
    /// hook_impl._pre_push_ns line (names as in pre-commit).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub struct PrePushLine<'a> {
        pub local_branch: &'a str, pub local_sha: &'a str, pub remote_branch: &'a str, pub remote_sha: &'a str }
    /// Lazy: a malformed line yields Err only when reached (the caller stops at the first namespace, as pre-commit).
    #[derive(Clone, Debug)] pub struct PrePushLines<'a> { /* … */ }
    impl<'a> Iterator for PrePushLines<'a> { type Item = Result<PrePushLine<'a>, crate::text::UnpackError>; }
    #[derive(Clone, Debug)] pub struct PrePushLinesBytes<'a> { /* … */ }
    impl<'a> Iterator for PrePushLinesBytes<'a> { type Item = Result<[&'a [u8]; 4], crate::text::UnpackError>; }
    pub fn pre_push_lines(stdin: &str) -> PrePushLines<'_>;                  // caller decoded strictly first
    pub fn pre_push_lines_bytes(stdin: &[u8]) -> PrePushLinesBytes<'_>;     // non-UTF-8 stdin (D-CRASH-11)
    /// git._parse_merge_msg_for_conflicts on bytes: lines starting with b'\t' or b'#\t', lstrip(b'#'), bytes strip;
    /// never decodes (pre-commit's per-line strict decode crash is D-CRASH-12).
    pub fn merge_msg_conflicts(merge_msg: &[u8]) -> Vec<&[u8]>;
    pub fn rev_line_match(line: &str) -> Option<[&str; 6]>;     // REV_LINE_RE, regex-free normative form (15 §A4.6)
    pub fn rev_line_rewrite(line: &str, new_rev: &str, frozen: Option<&str>) -> Option<String>;
    pub fn migrate_header_split(contents: &str) -> (&str, &str);
    /// lang_base.exe_exists after find_executable returned `found` and expanduser('~') returned `home` (os.path flavour by cfg).
    pub fn exe_exists_pure(found: &OsStr, home: &OsStr) -> bool;
}
pub mod random {      // CPython random.Random(1542676187).shuffle — bit exact (02 §11.7)
    #[derive(Clone, Debug)] pub struct Mt19937 { /* state */ }
    impl Mt19937 { pub fn init_by_array(key: &[u32]) -> Self; pub fn genrand_u32(&mut self) -> u32;
                   pub fn randbelow(&mut self, n: u64) -> u64; }
    pub fn precommit_shuffled_indices(n: usize) -> Vec<u32>;   // fresh RNG per call (lang_base._shuffled)
}
pub mod eaw { pub fn len_cjk(s: &str) -> usize; }              // east_asian_width W/F = 2 else 1 (UCD 15.1)
pub mod float {
    pub fn repr(x: f64) -> String;                             // float.__repr__
    pub fn format_duration(secs: f64) -> String;               // f'{round(x, 2) or 0}' (0.125→"0.12", 0.0→"0")
}
```

#### 1.4.3 `snp-argparse` (W1, WP-12)

```rust
#[derive(Copy, Clone, Debug)] pub enum Nargs { Zero, One, Optional, ZeroOrMore, OneOrMore, Remainder, Parser }
#[derive(Copy, Clone, Debug)] pub enum Kind { Store, StoreTrue, StoreFalse, StoreConst(&'static str), Append,
                                              Help, Version, SubParsers(&'static [SubCommand]) }
#[derive(Copy, Clone, Debug)] pub struct SubCommand { pub name: &'static str, pub help: Option<&'static str>,
                                                      pub listed: bool, pub spec: &'static ParserSpec }
#[derive(Copy, Clone, Debug)] pub enum Conv { Identity, UseColor, Stage, HookType, Int,
                                              Custom(fn(&std::ffi::OsStr) -> Result<std::ffi::OsString, String>, &'static str) }
#[derive(Copy, Clone, Debug)] pub enum DefaultValue { None, Str(&'static str), Env { var: &'static str, fallback: &'static str },
                                                      False, True, Int(i64), EmptyList, Const(&'static str) }
#[derive(Debug)] pub struct ActionSpec { pub opts: &'static [&'static str], pub dest: &'static str, pub nargs: Nargs,
    pub kind: Kind, pub conv: Conv, pub choices: Option<&'static [&'static str]>, pub default: DefaultValue,
    pub required: bool, pub help: Option<&'static str>, pub metavar: Option<&'static str>,
    pub mutex_group: Option<u8>, pub hidden: bool }
#[derive(Debug)] pub struct ParserSpec { pub prog_suffix: &'static str, pub description: Option<&'static str>,
    pub epilog: Option<&'static str>, pub actions: &'static [ActionSpec], pub allow_abbrev: bool, pub add_help: bool }
#[derive(Debug)] pub struct ParseCfg<'a> { pub prog: &'a str, pub env: &'a snp_core::env::Env, pub columns: usize }
#[derive(Debug)] pub enum Outcome { Parsed(Namespace), Help(String), Version(String), Usage { stderr: String } }
#[derive(Debug, Clone)] pub enum Value { Unset, Bool(bool), Str(std::ffi::OsString), List(Vec<std::ffi::OsString>), Int(i64) }
#[derive(Debug, Clone)] pub struct Namespace { /* dest → Value in declaration order; subcommand path */ }
impl Namespace {
    pub fn command(&self) -> Option<(&'static str, &Namespace)>;
    pub fn flag(&self, dest: &str) -> bool;            pub fn value(&self, dest: &str) -> Option<&std::ffi::OsStr>;
    pub fn list(&self, dest: &str) -> &[std::ffi::OsString]; pub fn int(&self, dest: &str) -> Option<i64>;
    pub fn was_given(&self, dest: &str) -> bool;
}
/// argparse 3.13.14 `_parse_known_args` + `parse_args` (12 §A.8.2): unknown args deferred to the top level, ambiguity
/// at consumption, env-string default converted at the end of the owning subparser only when the option was absent.
pub fn parse(spec: &'static ParserSpec, argv: &[std::ffi::OsString], cfg: &ParseCfg<'_>) -> Outcome;
pub fn format_help(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String;
pub fn format_usage(spec: &'static ParserSpec, path: &[&str], cfg: &ParseCfg<'_>) -> String;
/// COLUMNS (Python int()), else the stdout terminal width, else 80 (the formatter subtracts 2).
pub fn columns(env: &snp_core::env::Env, stdout_tty_cols: Option<usize>) -> usize;
/// Pure; every unique-prefix → option mapping of `base` must be unchanged in `extended` (12 §A.17 item 6).
pub fn prefix_stable(base: &'static ParserSpec, extended: &'static ParserSpec) -> Result<(), String>;
/// JSON description of a spec (prog included) from which `xtask cli-goldens` builds a real CPython argparse parser to
/// cross-check the snp-brand goldens (ADR-058).
pub fn export_json(spec: &'static ParserSpec, prog: &str) -> String;
pub mod textwrap {    // CPython textwrap as used by argparse.HelpFormatter
    pub fn wrap(text: &str, width: usize) -> Vec<String>;
    pub fn fill(text: &str, width: usize, initial_indent: &str, subsequent_indent: &str) -> String;
}
```

#### 1.4.4 `snp-cli` (W1, WP-12)

```rust
use std::ffi::OsString; use snp_core::{brand::Brand, env::Env, output::ColorSetting, stage::{HookType, Stage}};
/// argv WITHOUT argv[0]; `[]` ⇒ `["run"]`; `help [cmd]` re-parses `[cmd, --help]` (12 §A.5). Never touches disk.
pub fn parse(brand: Brand, argv: &[OsString], env: &Env, stdout_tty_cols: Option<usize>) -> CliOutcome;
#[derive(Debug)] pub enum CliOutcome { Cli(Cli), Stdout(String /* help | version */), Usage(String /* stderr, exit 2 */) }
#[derive(Debug)] pub enum Cli {
    Run(RunArgs), TryRepo(TryRepoArgs), HookImpl(HookImplArgs), Install(InstallArgs), Uninstall(UninstallArgs),
    InstallHooks(InstallHooksArgs), InitTemplatedir(InitTemplatedirArgs), Autoupdate(AutoupdateArgs), Clean(ColorArgs),
    Gc(ColorArgs), MigrateConfig(ConfigArgs), SampleConfig(ColorArgs), ValidateConfig(ValidateArgs),
    ValidateManifest(ValidateArgs), Hazmat(HazmatArgs),
    // snp brand only (absent from the compat brand's choices, help and parser; ADR-023):
    List(ListArgs), Config(ConfigCmd), Cache(CacheArgs), Migrate(MigrateArgs), Doctor(DoctorArgs),
    SelfCmd(SelfArgs), Completions(Shell),
}
#[derive(Clone, Debug)] pub struct ColorArgs { pub color: ColorSetting }
#[derive(Clone, Debug)] pub struct ConfigArgs { pub color: ColorSetting, pub config: OsString }
/// One field per argparse dest of 12 §A.8.1; `Some("")` ref options normalised to None (Python truthiness).
#[derive(Clone, Debug)] pub struct RunArgs {
    pub color: ColorSetting, pub config: OsString, pub hooks: Vec<OsString> /* compat: ≤ 1 */, pub verbose: bool,
    pub all_files: bool, pub files: Vec<OsString>, pub show_diff_on_failure: bool, pub fail_fast: bool,
    pub hook_stage: Stage, pub remote_branch: Option<OsString>, pub local_branch: Option<OsString>,
    pub from_ref: Option<OsString>, pub to_ref: Option<OsString>, pub pre_rebase_upstream: Option<OsString>,
    pub pre_rebase_branch: Option<OsString>, pub commit_msg_filename: Option<OsString>,
    pub prepare_commit_message_source: Option<OsString>, pub commit_object_name: Option<OsString>,
    pub remote_name: Option<OsString>, pub remote_url: Option<OsString>, pub checkout_type: Option<OsString>,
    pub is_squash_merge: Option<OsString>, pub rewrite_command: Option<OsString>, pub ext: RunExt,
}
#[derive(Clone, Debug, Default)] pub struct RunExt {    // snp brand flags (§9.3); env equivalents applied by snp-run
    pub jobs: Option<std::num::NonZeroUsize>, pub groups: Vec<OsString> /* OR */, pub require_groups: Vec<OsString> /* AND */,
    pub no_groups: Vec<OsString> /* exclusion wins; `@ungrouped` allowed in all three */,
    pub no_fast_path: bool, pub stdin_files: Option<StdinFormat>, pub summary: bool, pub no_local_config: bool,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum StdinFormat { Nul, Lines }
#[derive(Clone, Debug)] pub struct TryRepoArgs { pub repo: OsString, pub rev: Option<OsString>, pub run: RunArgs }
#[derive(Clone, Debug)] pub struct HookImplArgs { pub color: ColorSetting, pub config: OsString,
    pub hook_type: OsString /* free string */, pub hook_dir: Option<OsString>, pub skip_on_missing_config: bool,
    pub rest: Vec<OsString> /* raw REMAINDER, `--` kept */ }
#[derive(Clone, Debug)] pub struct InstallArgs { pub color: ColorSetting, pub config: OsString, pub overwrite: bool,
    pub install_hooks: bool, pub hook_types: Vec<HookType>, pub allow_missing_config: bool,
    pub git_config_hooks: bool /* snp brand `--git-config-hooks` (ADR-057) */ }
#[derive(Clone, Debug)] pub struct UninstallArgs { pub color: ColorSetting, pub config: OsString,
    pub hook_types: Vec<HookType>, pub git_config_hooks: bool }
#[derive(Clone, Debug)] pub struct InstallHooksArgs { pub color: ColorSetting, pub config: OsString, pub with_fallback: bool }
#[derive(Clone, Debug)] pub struct InitTemplatedirArgs { pub color: ColorSetting, pub config: OsString,
    pub directory: OsString, pub no_allow_missing_config: bool, pub hook_types: Vec<HookType> }
#[derive(Clone, Debug)] pub struct AutoupdateArgs { pub color: ColorSetting, pub config: OsString, pub bleeding_edge: bool,
    pub freeze: bool, pub repos: Vec<OsString>, pub jobs: i64 }
#[derive(Clone, Debug)] pub struct ValidateArgs { pub color: ColorSetting, pub filenames: Vec<OsString>,
    pub strict: bool /* snp brand */, pub portable: bool /* snp brand */ }
#[derive(Clone, Debug)] pub struct HazmatArgs { pub color: ColorSetting, pub tool: HazmatTool }
#[derive(Clone, Debug)] pub enum HazmatTool { Cd { subdir: OsString, cmd: Vec<OsString> },
                                              IgnoreExitCode { cmd: Vec<OsString> }, N1 { cmd: Vec<OsString> } }
#[derive(Clone, Debug)] pub struct ListArgs { pub config: OsString, pub json: bool, pub stage: Option<Stage>, pub groups: Vec<OsString>,
    pub require_groups: Vec<OsString>, pub no_groups: Vec<OsString> }
#[derive(Clone, Debug)] pub enum ConfigCmd { Dump { config: OsString, json: bool, no_local_config: bool }, Schema }
#[derive(Clone, Debug)] pub struct CacheArgs { pub action: CacheAction }
#[derive(Copy, Clone, Debug)] pub enum CacheAction { Dir, Size, Gc, Clean }
#[derive(Clone, Debug)] pub enum MigrateArgs {
    Lefthook { root: Option<OsString>, config: Option<OsString>, output: Option<OsString>, portable: bool, local: bool,
               strict: bool, install: bool, force: bool },
    Builtin { config: OsString }, Upstream { config: OsString } }
#[derive(Clone, Debug)] pub struct DoctorArgs { pub fix: bool, pub fast_path: bool, pub stash: bool, pub config: OsString }
#[derive(Clone, Debug)] pub enum SelfArgs { AliasInstall { dir: Option<OsString>, force: bool }, AliasUninstall, AliasStatus, Check }
#[derive(Copy, Clone, Debug)] pub enum Shell { Bash, Zsh, Fish, PowerShell }
pub static PRE_COMMIT_SPEC: &snp_argparse::ParserSpec;     // compat brand tree (12 §A.8.1, verbatim help strings)
pub static SNP_SPEC: &snp_argparse::ParserSpec;            // snp brand tree = compat ⊕ extensions (prefix-stable)
pub static COMPLETIONS: &[(Shell, &str)];                  // generated by `xtask completions` from SNP_SPEC
```

#### 1.4.5 `snp-pyre` (W1, WP-13) — CPython-3.12 `re`, two engines (01 Addendum §A.6)

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub enum Mode { Str, Bytes }        // Str: files/exclude/--pattern; Bytes: pygrep
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub enum Haystack { Path, Line, Blob } // only matters for `$` (§A.6.5)
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub struct Options {
    pub mode: Mode, pub haystack: Haystack, pub ignore_case: bool /* pygrep -i */, pub multiline_dotall: bool /* --multiline */ }
impl Options { pub const CONFIG: Options; /* Str, Path, false, false */ }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum PyReError {
    Re { msg: Box<str>, pos: Option<usize> },   // == Python re.error; Display = str(e) incl. " (line L, column C)"
    Overflow,                                   // "the repetition number is too large"
    Value(Box<str>),                            // "ASCII and UNICODE flags are incompatible", …
    Recursion,                                  // nesting ≥ 485 groups (pre-commit accepts ≤ 484)
    Unsupported(Box<str>),                      // valid Python, beyond snp's engines (§A.6.13 D3/D5/D6)
}
impl PyReError {
    pub fn py_type(&self) -> &'static str;      // "re.error" | "OverflowError" | "ValueError" | "RecursionError"
    pub fn py_last_line(&self) -> String;       // "re.error: missing ), unterminated subpattern at position 0" (pygrep, §A.4.5)
}
impl std::fmt::Display for PyReError {}
#[derive(Debug, Clone, PartialEq, Eq)] pub struct PyWarning { pub msg: Box<str> }  // "Possible nested set at position 1"
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum MatchKind { All, EmptyOrNewline, Prefix, Suffix, Contains, Tier1, Tier2 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MatchError { Backtrack }      // fancy backtrack_limit (1e8) exceeded
/// Parse + post-parse checks only; never builds an engine (validate-config, config load).
pub fn validate(src: &str, mode: Mode) -> Result<Vec<PyWarning>, PyReError>;
/// For cfgv 0.3 `CheckFn::check_regex_with` (bool callback, ADR-003): `validate(src, Mode::Str).is_ok()`. Every
/// rejection — including Overflow/Value/Recursion — becomes cfgv's "is not a valid python regex" leaf (D-CRASH-04);
/// `check_regex_try_with` (which could reproduce the exit-3 crash) is deliberately unused (§1.5).
pub fn cfgv_is_valid(src: &str) -> bool;
#[derive(Debug)] pub struct PyRegex { /* src, opts, kind, fast: Option<regex>, exact: OnceLock<fancy>, relaxed, cond */ }
impl PyRegex {
    pub fn compile(src: &str, opts: Options) -> Result<PyRegex, PyReError>;   // engines built lazily on first match
    pub fn source(&self) -> &str;
    pub fn kind(&self) -> MatchKind;
    pub fn warnings(&self) -> &[PyWarning];
    pub fn search(&self, h: &str) -> Result<bool, MatchError>;               // re.search(...) is not None
    pub fn match_prefix(&self, h: &str) -> Result<bool, MatchError>;         // re.match  → \A(?:P)
    pub fn fullmatch(&self, h: &str) -> Result<bool, MatchError>;            // re.fullmatch → \A(?:P)\z
    pub fn search_bytes(&self, h: &[u8]) -> Result<Option<(usize, usize)>, MatchError>; // pygrep needs the span
}
#[derive(Debug, Default)] pub struct RegexCache { /* Mutex<IndexMap<(Box<str>, Options), Arc<PyRegex>>> */ }
impl RegexCache {
    pub fn new() -> Self;
    pub fn get(&self, src: &str, opts: Options) -> Result<std::sync::Arc<PyRegex>, PyReError>;
}
```

#### 1.4.6 `snp-yaml` (W1, WP-14) — PyYAML compatibility

```rust
#[derive(Copy, Clone, Debug)] pub struct Limits { pub max_depth: u32 /* 512 */, pub max_constructed_nodes: u32 /* 1_000_000 */,
                                                   pub max_bytes: usize /* 16 MiB */ }
impl Default for Limits {}
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Schema { PyYaml11Safe, Yaml12Core /* lefthook reader only */ }
#[derive(Debug, Clone)] pub struct YamlError { /* context, context_mark, problem, problem_mark, note | reader position */ }
impl std::fmt::Display for YamlError {}   // == str(MarkedYAMLError) of the C loader, marks in "<unicode string>"
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub struct Mark { pub index: usize, pub line: usize, pub column: usize }
/// cfgv `open(…, encoding='utf-8').read()` parity: strict UTF-8 (CPython error text) + universal newlines.
pub fn decode_text(bytes: &[u8]) -> Result<std::borrow::Cow<'_, str>, snp_pycompat::codec::Utf8DecodeError>;
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)] pub struct NodeId(pub u32);
#[derive(Debug, Clone)] pub enum NodeKind { Scalar(Box<str>), Seq(Vec<NodeId>), Map(Vec<(NodeId, NodeId)>) }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ScalarStyle { Plain, Single, Double, Literal, Folded, NotScalar }
#[derive(Debug, Clone)] pub struct Node { pub kind: NodeKind, pub tag: Box<str>, pub style: ScalarStyle,
                                          pub span: std::ops::Range<usize>, pub start: Mark, pub anchor: Option<Box<str>> }
/// Arena composer: an alias is the *same* NodeId as its anchor (never a copy) — migrate-config dedupes by id.
#[derive(Debug, Clone)] pub struct Document { pub nodes: Vec<Node>, pub root: Option<NodeId> }
pub fn compose(text: &str, schema: Schema, lim: &Limits) -> Result<Document, YamlError>;
/// SafeConstructor + implicit resolver → cfgv::Value (date, datetime, big ints, bytes, set, omap/pairs as tuples,
/// merge keys, duplicate keys last-wins at the first position with Python key equality 1 == 1.0 == True).
pub fn construct(doc: &Document, schema: Schema, lim: &Limits) -> Result<cfgv::Value, YamlError>;
pub fn load(text: &str, lim: &Limits) -> Result<cfgv::Value, YamlError>;       // PyYaml11Safe, one document
pub mod emit {
    /// yaml.dump(o, Dumper=CSafeDumper, default_flow_style=False, indent=4, sort_keys=False) — byte exact.
    pub fn dump(v: &cfgv::Value) -> String;
    #[derive(Copy, Clone, Debug)] pub enum Quote { Plain, Single, Double }
    /// autoupdate.py:146-149: yaml_dump({'rev': rev}, default_style=q).split(':', 1)[1].strip()
    pub fn scalar_for_rev(rev: &str, q: Quote) -> String;
}
pub fn resolve_plain(s: &str, schema: Schema) -> ImplicitTag;   // bool/int/float/null/timestamp/merge/value/str
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ImplicitTag { Null, Bool, Int, Float, Timestamp, Merge, Value, Str }
```

#### 1.4.7 `snp-os` (W1, WP-15) — durable filesystem, locks, terminal, home

```rust
use std::{ffi::{OsStr, OsString}, fs::File, io, path::{Path, PathBuf}};
use snp_core::{cancel::CancelToken, env::Env, output::ColorSetting};
pub mod fs {
    /// Windows (ADR-068): every rename-based primitive here (`rename_with_retry`, `atomic_replace`, `retire_dir`, and
    /// the store's materialise/marker renames, which call `rename_with_retry`) retries on ERROR_ACCESS_DENIED,
    /// ERROR_SHARING_VIOLATION and ERROR_LOCK_VIOLATION (antivirus, indexer, 11 §A.19.2, 06 §5.6) with 50·2^n ms
    /// backoff, at most 5 retries (≈ 1.5 s), then returns the last error. Unix: a single attempt.
    pub fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()>;
    /// O_EXCL create (0600) + write_all + sync_all + fsync(parent). Fails if `path` exists.
    pub fn write_new_durable(path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Streams `src` into a new O_EXCL durable file; returns bytes written. Used for stash patches (never in memory).
    pub fn copy_into_new_durable(path: &Path, src: &mut dyn io::Read) -> io::Result<u64>;
    /// temp file in the same dir + fsync + rename over `path` (+ fsync dir). Replaces a symlink instead of following it.
    pub fn atomic_replace(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()>;
    /// In-place rewrite preserving inode/mode/hardlinks (native fixers; 09 §9.8): open without O_CREAT, write, set_len.
    pub fn rewrite_in_place(path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// rmtree parity (chmod + retry on EACCES/EPERM). Unix: fd-relative walk from an `O_DIRECTORY|O_NOFOLLOW` handle
    /// of `root` (rustix `openat`, `unlinkat`, `fchmodat` on the dirfd), so a directory swapped for a symlink
    /// mid-deletion is never followed (CVE-2022-21658 class); Windows: `std::fs::remove_dir_all` (handle-relative since
    /// 1.58.1) + read-only attribute clearing. Refuses unless `path` is strictly inside `root`. Missing path → Ok.
    pub fn remove_tree_within(path: &Path, root: &Path) -> io::Result<()>;
    /// Unnamed temporary file in `dir`: O_TMPFILE (Linux), else O_EXCL 0600 create + immediate unlink, Windows
    /// FILE_FLAG_DELETE_ON_CLOSE. Nothing survives a crash (output spill files, ADR-077).
    pub fn unnamed_temp(dir: &Path) -> io::Result<File>;
    /// rename(path → trash/<rand>) then remove; gc/clean never leave half-deleted dirs at live paths.
    pub fn retire_dir(path: &Path, trash: &Path) -> io::Result<()>;
    pub fn read_capped(path: &Path, cap: u64) -> io::Result<Vec<u8>>;
    pub fn create_truncate_nofollow(path: &Path) -> io::Result<File>;          // crash log
    /// mkdir -p, created components 0700 (Unix). Refuses (PermissionDenied) only a world-writable non-sticky final dir;
    /// returns `Ownership::Foreign { root_over_user }` when owned by another uid (ADR-044). The store refuses a Foreign
    /// store when `root_over_user` (euid 0, owner non-root) unless `SNP_ALLOW_FOREIGN_STORE=1`, and otherwise warns
    /// once on stderr (ADR-077, D-SEC-13, D-DIAG-05).
    pub fn ensure_private_dir(path: &Path) -> io::Result<Ownership>;
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Ownership { Own, Foreign { root_over_user: bool } }
}
pub mod lock {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum LockMode { Shared, Exclusive }
    #[derive(Debug)] pub struct FileLock { /* std::fs::File; released on drop */ }
    impl FileLock {
        pub fn try_acquire(path: &Path, mode: LockMode) -> io::Result<Option<FileLock>>;
        /// try first; on contention call `on_wait()` once, then wait, polling `try_lock` every 50 ms so `cancel`
        /// is observed; Err(Interrupted) when cancelled; Err(TimedOut) after `timeout` (None = forever).
        pub fn acquire(path: &Path, mode: LockMode, on_wait: &mut dyn FnMut(), cancel: &CancelToken,
                       timeout: Option<std::time::Duration>) -> io::Result<FileLock>;
        pub fn file(&self) -> &File;
    }
}
pub mod term {
    /// pre-commit's use_color(): Always/Never as given; Auto = stderr is a terminal ∧ TERM != "dumb" (Unix);
    /// Windows: winapi_util::console::Console::stderr()?.set_virtual_terminal_processing(true).is_ok() (color.py parity).
    /// NO_COLOR / FORCE_COLOR are ignored (parity, ADR-029).
    pub fn decide_color(s: ColorSetting, env: &Env) -> bool;
    pub fn stdout_tty_columns() -> Option<usize>;
    pub fn stdout_is_tty() -> bool;
    pub fn stdout_is_windows_console() -> bool;         // lossy-UTF-8 hook output only for a real console (07 §7.5)
    /// Windows: re-run SetConsoleMode(ENABLE_VIRTUAL_TERMINAL_PROCESSING) on the stderr console when the colour decision
    /// was `true` (children with inherited stdio can reset it, 14 P-52); no decision change, no byte change. Elsewhere a no-op.
    pub fn ensure_vt();
}
pub mod home {
    pub fn expanduser(p: &OsStr, env: &Env) -> OsString;  // Python semantics incl. HOME="" and Windows USERPROFILE (15 §A4.9-10)
    pub fn default_cache_root(env: &Env) -> Option<PathBuf>;  // XDG_CACHE_HOME (non-empty) else expanduser("~/.cache")
}
pub mod realpath { pub fn realpath_nonstrict(p: &Path, cwd: &Path) -> io::Result<PathBuf>; } // posix _joinrealpath / nt rules
pub mod fsinfo {
    /// DiffOracle gate (ADR-078): an allow-list of filesystems with kernel-maintained ctime and ns mtime — Linux statfs
    /// f_type ∈ {ext2/3/4, xfs, btrfs, tmpfs, zfs, f2fs, bcachefs}; macOS f_fstypename ∈ {apfs, hfs}. Everything else
    /// (NFS, SMB, 9p, every FUSE incl. sshfs/rclone/virtiofs, overlayfs, vfat, exfat, ntfs-3g, unknown) → false.
    pub fn stat_oracle_safe(path: &Path) -> io::Result<bool>;
}
pub fn same_file(a: &Path, b: &Path) -> io::Result<bool>;
```

#### 1.4.8 `snp-proc` (W1, WP-15; W0 ships working baselines of `spawn::{capture, stream, inherit}`, `resolve::{find_executable, normexe, normalize_cmd}` and `xargs::{cpu_count, target_concurrency, platform_max_length, partition}`)

```rust
use std::{ffi::{OsStr, OsString}, io, num::NonZeroUsize, path::{Path, PathBuf}, sync::Arc};
use snp_core::{cancel::CancelToken, env::Env, failure::Failure};
pub mod spawn {
    #[derive(Clone, Debug)] pub enum Stdin { EmptyPipe, Null, Bytes(Arc<[u8]>), Inherit }
    /// Inherit: child stays in snp's process group (terminal ^C reaches it, parity).
    /// Isolated: Unix `process_group(0)`, Windows `CREATE_NEW_PROCESS_GROUP` — hooks with a `timeout` and every
    /// `Role::Critical` command (ADR-026). Both keep std's posix_spawn path (no `pre_exec`).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Group { Inherit, Isolated }
    /// What cancellation may do to the child (ADR-065; normative table in §10.4).
    /// Hook: hooks and hook partitions (Windows: placed in a per-run Job Object without KILL_ON_JOB_CLOSE).
    /// Tool: snp's own git/installer/probe/clone commands.
    /// Critical: restore-critical git (stash checkout, applies, rollback checkout, ITA rm/re-add, stage_fixed add);
    /// requires Group::Isolated; never signalled or killed by anything in this crate and ignores `cancel`; env gets
    /// GIT_TERMINAL_PROMPT=0, Windows adds CREATE_NO_WINDOW (no console event reaches it); reaped with
    /// waitid(WEXITED|WSTOPPED|WNOWAIT): a SIGTTIN/SIGTTOU stop SIGKILLs its group and the call fails (`stopped_by_tty`).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Role { Hook, Tool, Critical }
    #[derive(Clone, Debug)] pub struct Spec<'a> {
        pub argv: &'a [OsString],           // argv[0] is the RESOLVED program (normalize_cmd already applied)
        pub env: &'a Env, pub cwd: &'a Path, pub stdin: Stdin, pub group: Group, pub role: Role,
        pub deadline: Option<std::time::Instant>,
        /// Never prevents a spawn (the scheduler and the pool are the only gate for new work). For Hook/Tool children a
        /// cancelled token starts the §10.4 escalation and bounds the output wait (kill → 500 ms drain → partial output);
        /// a child started on an already-cancelled token is escalated at once. Critical children ignore it.
        pub cancel: &'a CancelToken,
    }
    /// Python returncode semantics: Unix signal N → -N; Windows: the u32 exit code as i64 (14 A.1 #4).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub struct PyStatus(pub i64);
    /// One global in-memory budget for captured output across all running children (default 64 MiB, ADR-036);
    /// beyond it a capture spills to an unnamed file (`snp_os::fs::unnamed_temp`) in `spill_dir` (store `spill/`, 0700,
    /// or the OS temp dir).
    #[derive(Debug)] pub struct OutputBudget { /* AtomicUsize remaining, spill_dir */ }
    impl OutputBudget { pub fn new(bytes: usize, spill_dir: PathBuf) -> Arc<Self>; }
    #[derive(Debug)] pub enum Output { Memory(Vec<u8>), Spilled { file: std::fs::File, len: u64 } }
    impl Output { pub fn write_to(&self, w: &mut dyn io::Write) -> io::Result<()>; pub fn is_blank(&self) -> bool; }
    #[derive(Debug)] pub struct Captured { pub status: PyStatus, pub stdout: Vec<u8>, pub stderr: Vec<u8>,
                                           pub stopped_by_tty: bool /* Critical only */ }
    #[derive(Debug)] pub struct Merged { pub status: PyStatus, pub output: Output, pub timed_out: bool, pub interrupted: bool }
    #[derive(Debug)] pub struct SpawnError { pub program: OsString, pub io: io::Error }
    impl SpawnError { pub fn python_text(&self) -> String; }   // "[Errno 8] Exec format error: './x.sh'" / "[WinError N] …"
    /// Output reading (ADR-065): every pipe/pty read end is owned by a dedicated, non-scoped reader thread
    /// (`Builder::spawn`; an OS refusal is a SpawnError) that sends chunks over a bounded channel; the caller waits
    /// with `recv_timeout` ticks that observe `deadline` and `cancel`. Normal runs read to EOF (parity: a descendant
    /// holding the pipe delays the hook exactly as in pre-commit); after a deadline or cancellation the wait ends after
    /// the kill escalation plus a 500 ms drain, the partial output is returned and the reader thread is abandoned (it
    /// ends at EOF or process exit). No `poll` on any platform (macOS ptys do not support it).
    /// Separate pipes, read concurrently. snp's own tools (git, installers, probes) and Critical commands.
    pub fn capture(spec: &Spec<'_>) -> Result<Captured, SpawnError>;
    /// One pipe for stdout+stderr (std::io::pipe, Command dropped before reading), or a POSIX pty when `pty`: master
    /// `openpt(RDWR|NOCTTY|CLOEXEC)`, slave `open(ptsname, O_RDWR|O_NOCTTY|O_CLOEXEC)` (both under the spawn lock on
    /// macOS), OPOST|ONLCR cleared, stdin /dev/null, 0×0 size, slave dropped before reading, EIO = EOF. pty failure →
    /// pipe (X13-EXE-06). Hooks only.
    pub fn merged(spec: &Spec<'_>, pty: bool, budget: &OutputBudget) -> Result<Merged, SpawnError>;
    /// stdout streamed into `sink` (diff hashing, stash patch), stderr captured. Returns status and stderr.
    pub fn stream(spec: &Spec<'_>, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError>;
    pub fn inherit(spec: &Spec<'_>) -> Result<PyStatus, SpawnError>;   // legacy hooks, show-diff, hazmat children
}
pub mod pool {
    /// N (S7, ADR-065): `explicit` (`-j`/`SNP_JOBS`) when given, else min(target, available_parallelism()) — cgroup-aware,
    /// and invisible in output because partition contents always use pre-commit's `jobs`; then clamped to the fd budget
    /// max(1, (RLIMIT_NOFILE soft − 64) / 8) on Unix. The rlimit itself is never raised (children would inherit it).
    pub fn pool_size(explicit: Option<NonZeroUsize>, target: NonZeroUsize) -> NonZeroUsize;
    /// The global bound on concurrent child processes / native partitions (S7). Fair FIFO handout.
    /// Re-entrancy (ADR-070): permits are held only by process partitions and native partitions. A thread that already
    /// holds a permit (thread-local flag) runs nested `map_ordered` work inline and sequentially without acquiring
    /// (debug builds assert that a permit holder never blocks in `acquire`); bucket-level hook threads hold no permit.
    /// So `PRE_COMMIT_NO_CONCURRENCY=1` (N = 1) cannot deadlock a meta hook that classifies files.
    #[derive(Debug)] pub struct JobPool { /* Mutex<usize> + Condvar + CancelToken */ }
    #[derive(Debug)] pub struct Permit<'a>(/* &'a JobPool */);   // releases on drop
    impl JobPool {
        pub fn new(permits: NonZeroUsize, cancel: CancelToken) -> Arc<Self>;
        pub fn permits(&self) -> NonZeroUsize;
        pub fn acquire(&self) -> Option<Permit<'_>>;          // None once cancelled (stop starting work)
        /// Runs `f(0..n)` on ≤ min(limit, n) scoped threads (`Builder::spawn_scoped`, every handle joined), each call
        /// holding a permit; results in index order. n == 1, or a caller that holds a permit, runs inline. A panicking
        /// call becomes Err(Failure::Exc(PanicException …)); a refused thread spawn becomes Err(Failure::from_io).
        pub fn map_ordered<T: Send, F: Fn(usize) -> Result<T, Failure> + Sync>(&self, limit: NonZeroUsize, n: usize, f: F)
            -> Vec<Result<T, Failure>>;
        pub fn holds_permit() -> bool;                        // the calling thread's flag (for nested callers)
    }
    /// Independent counting semaphores: NetLimiter (8 concurrent clones/downloads), InstallLimiter (min(N, 4); 1 when the
    /// cgroup memory limit is below 4 GiB). Compile-heavy installs (rust, golang, haskell, swift, dotnet, dart, conda)
    /// take every permit (`acquire_all`), so they never run beside another install (ADR-065).
    #[derive(Debug)] pub struct Limiter { /* Mutex<usize> + Condvar */ }
    impl Limiter { pub fn new(n: NonZeroUsize) -> Arc<Self>; pub fn permits(&self) -> NonZeroUsize;
                   pub fn acquire(&self, c: &CancelToken) -> Option<LimiterPermit<'_>>;
                   pub fn acquire_all(&self, c: &CancelToken) -> Option<LimiterPermit<'_>>; }
    #[derive(Debug)] pub struct LimiterPermit<'a>(/* &'a Limiter, count */);
}
pub mod signals {
    /// Unix: one signal-hook iterator thread over `SignalsInfo<WithOrigin>` for SIGINT/SIGTERM/SIGHUP (caught, never
    /// SIG_IGN, so children get default dispositions). Windows: ctrlc with `termination` (CTRL_C/CTRL_BREAK →
    /// Interrupt; CTRL_CLOSE/LOGOFF/SHUTDOWN: Windows ends the process when the handler returns, so these are
    /// journal-recovered events, §4.3). On the first signal: cancel(token, reason) and `registry::forward` per the §10.4
    /// table; after `SNP_KILL_GRACE` (2 s; parsed with `try_from_secs_f64`, ≤ 60 s) `registry::escalate`. While a
    /// `RestoreShield` is alive, further signals only print "snp: restoring stashed changes, please wait" (stderr) once.
    /// `subreaper` (run, hook-impl, try-repo on Linux): PR_SET_CHILD_SUBREAPER via rustix, so orphaned hook descendants
    /// reparent to snp and `registry::quiesce` can find them on the cancel path.
    pub fn install(cancel: CancelToken, subreaper: bool) -> io::Result<SignalGuard>;
    #[derive(Debug)] pub struct SignalGuard { /* thread handle */ }
    impl SignalGuard {
        pub fn shield(&self) -> RestoreShield;
        /// After restore: re-raise the received SIGTERM/SIGHUP with the default disposition
        /// (signal_hook::low_level::emulate_default_handler) so the parent sees status 143/129 (ADR-026).
        pub fn die_by_signal_if_terminated(&self) -> Option<i32>;
    }
    #[derive(Debug)] pub struct RestoreShield { /* clears on drop */ }
}
/// Every child spawned by `spawn::*` is registered until reaped (waitid(WNOWAIT) on Unix → no pid reuse; Windows keeps
/// the handle). Critical children are registered for reaping only: nothing here ever signals them (ADR-065).
pub mod registry {
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Sig { Int, Term, Hup }                  // exhaustive
    /// Kernel = tty-generated (^C, hangup: si_code SI_KERNEL / si_pid 0, already delivered to the foreground group);
    /// Sent = kill(2)/sigqueue (SI_USER/SI_QUEUE); Unknown = origin unavailable (treated as Sent for TERM/HUP, Kernel for INT).
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Origin { Kernel, Sent, Unknown }
    pub fn forward(sig: Sig, origin: Origin);   // first phase of §10.4: Inherit hooks/tools only when Sent; Isolated hooks by -pgid always
    pub fn escalate();                          // after the grace: SIGKILL Hook/Tool children (Isolated by -pgid), Windows job terminate
    /// Cancel path only, before the restore: wait ≤ `bound` for Hook/Tool children to exit, then (Linux, subreaper)
    /// SIGKILL and reap every remaining non-Critical descendant found under /proc/self/task/*/children; Windows: the run's
    /// hook Job Object is terminated; macOS: Isolated groups by -pgid only (residual risk, D-DATA-02).
    pub fn quiesce(bound: std::time::Duration);
}
/// Test instrumentation (feature `testing`, enabled only through `crates/snp`'s `testing` feature): with
/// `SNP_TEST_SPAWN_LOG=<file>` every spawn appends `<role> <group> <argv0 basename>\n` (spawn-budget tests, WP-32/WP-36).
#[cfg(feature = "testing")] pub mod spawn_log { pub fn enable_from_env(env: &Env); pub fn read(path: &Path) -> Vec<String>; }
pub mod resolve {
    pub fn find_executable(name: &OsStr, env: &Env, cwd: &Path) -> Option<PathBuf>;   // parse_shebang.find_executable
    #[derive(Debug)] pub enum ExeError { NotFound(OsString), IsDirectory(OsString), NotExecutable(OsString), Io(io::Error) }
    impl ExeError { pub fn message(&self) -> String; }   // "Executable `x` not found" (no trailing \n)
    pub fn normexe(orig: &OsStr, env: &Env, cwd: &Path) -> Result<PathBuf, ExeError>;
    /// normexe → shebang (file_identify::parse_shebang_from_file_with, empty for non-UTF-8 = identify parity) →
    /// normexe again; Windows-only basename fallback for absolute shebang interpreters (D-PLAT-02).
    pub fn normalize_cmd(cmd: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<OsString>, ExeError>;
    pub fn self_exe() -> io::Result<PathBuf>;            // current_exe → absolute, not canonicalised (03 H.8.1)
}
pub mod xargs {
    pub fn cpu_count() -> NonZeroUsize;                  // py_cpu_count(): sched_getaffinity on Linux (14 A.4.3)
    /// PRE_COMMIT_NO_CONCURRENCY present → 1; TRAVIS present → 2; else `cpu`; 1 when require_serial.
    pub fn target_concurrency(env: &Env, require_serial: bool, cpu: NonZeroUsize) -> NonZeroUsize;
    pub fn platform_max_length(startup_env_size: usize) -> usize;   // POSIX clamp(ARG_MAX-2048-env, 4096, 131072); win 30720
    pub fn batch_max_length(cmd_exe: &Path) -> usize;    // windows .bat/.cmd budget (8192 - len - 4 - 1024)
    pub fn cmdlen(s: &OsStr) -> usize;                   // bytes (unix) / UTF-16 units (windows)
    #[derive(Debug)] pub struct ArgumentTooLong(pub OsString);
    /// Exact port of xargs.partition: index ranges into `files`; ≥ 1 partition (zero files → [0..0]).
    pub fn partition(cmd: &[OsString], files: &[&OsStr], jobs: NonZeroUsize, max_len: usize)
        -> Result<Vec<std::ops::Range<usize>>, ArgumentTooLong>;
    pub fn aggregate(codes: impl IntoIterator<Item = super::spawn::PyStatus>) -> super::spawn::PyStatus; // max |rc|, first wins
}
pub mod hookcmd {
    #[derive(Debug)] pub enum HookCmdError { Shlex(snp_pycompat::shlex::ShlexError), Empty }
    /// shlex.split(entry); "pre-commit hazmat …"/"snp hazmat …" (first two tokens, any quoting) → [self_exe, "__hazmat", …];
    /// then args (02 §11.3, 03 H.8.1). Other `pre-commit …` entries are not rewritten (parity).
    pub fn hook_cmd(entry: &str, args: &[Arc<str>], self_exe: &Path) -> Result<Vec<OsString>, HookCmdError>;
}
```

`snp-probe` (W1, WP-15): a tiny dev binary with subcommands `args|count|env|cwd|tty|stdin-kind|exit|signal|append|
write|truncate|bytes|interleave|sleep|daemonize|stdin|fail-on|ntstatus` (10 §7.3.5, 14 §A.6.2) used as a portable hook
entry in tests.

#### 1.4.9 `snp-net` (W1, WP-16) — the only crate that touches the network

```rust
use std::{io, path::{Path, PathBuf}};
use snp_core::{cancel::CancelToken, env::Env};
#[derive(Clone, Debug, PartialEq, Eq)] pub enum Checksum { Sha256([u8; 32]) }
#[derive(Debug, thiserror::Error)] pub enum NetError {
    #[error("network access disabled (SNP_OFFLINE)")] Offline,
    #[error("GET {url}: HTTP {status}")] Http { url: String, status: u16 },
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")] Checksum { url: String, expected: String, actual: String },
    #[error("{url}: response exceeds {limit} bytes")] TooLarge { url: String, limit: u64 },
    #[error("{0}")] Io(#[from] io::Error),
    #[error("{url}: {msg}")] Transport { url: String, msg: String },
    #[error("archive entry {0:?} escapes the destination")] UnsafeEntry(String),
    #[error("insecure URL refused: {0}")] Insecure(String),
    #[error("interrupted")] Cancelled,
}
pub trait Fetch: Send + Sync + std::fmt::Debug {   // seam for tests (S12)
    fn get_text(&self, url: &str, max: u64) -> Result<String, NetError>;
    /// Streams to `dest_dir/<rand>.part`, hashing on the fly; renames to `file_name` only after the checksum matches.
    fn get_verified(&self, url: &str, sum: &Checksum, dest_dir: &Path, file_name: &str, cancel: &CancelToken)
        -> Result<PathBuf, NetError>;
}
#[derive(Debug)] pub struct HttpFetch { /* ureq::Agent: rustls + platform verifier; env proxies; 30 s connect, 60 s idle, 3 retries */ }
impl HttpFetch { pub fn from_env(env: &Env) -> Self; }   // SSL_CERT_FILE/DIR, HTTPS_PROXY/NO_PROXY, SNP_OFFLINE, SNP_ALLOW_INSECURE_MIRROR
impl Fetch for HttpFetch {}
pub mod archive {
    #[derive(Copy, Clone, Debug)] pub struct Extract { pub strip_components: u8, pub allow_symlinks_inside: bool, pub max_total_bytes: u64 }
    /// Extracts into `<dest>.part-<rand>` (same FS) in two passes — regular files and directories first, symlinks last —
    /// then renames to `dest` (ADR-077). Rejected: absolute, `..` and drive-prefixed names; device, FIFO and socket
    /// entries; hardlinks except to an already extracted regular file inside the tree; symlinks whose target, resolved
    /// against the extracted tree, leaves it or passes through another symlink (no chains such as `x -> .`,
    /// `y -> x/..`); parent directories are opened without following symlinks (Linux: openat2 RESOLVE_BENEATH |
    /// RESOLVE_NO_SYMLINKS where available). setuid/setgid/sticky stripped. Never writes through pre-existing symlinks.
    pub fn tar_gz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), super::NetError>;
    pub fn tar_xz(archive: &Path, dest: &Path, o: &Extract) -> Result<(), super::NetError>;   // feature "xz" (node on unix)
    pub fn zip(archive: &Path, dest: &Path, o: &Extract) -> Result<(), super::NetError>;
}
pub mod tools {   // catalogue: where to download and how to verify (§6.3)
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub enum Tool { Uv, Node, Go, Rustup, RubyBuild, Rbenv }
    #[derive(Clone, Debug)] pub struct Artifact { pub url: String, pub checksum: super::Checksum, pub layout: Layout }
    #[derive(Clone, Debug)] pub enum Layout { TarGz { strip: u8 }, TarXz { strip: u8 }, Zip { strip: u8 }, Executable }
    /// Checksum manifests (`SHASUMS256.txt`, go.dev JSON, `.sha256`) are always fetched over https from the canonical
    /// origin (or an explicit https `SNP_NODE_SHASUMS_URL`/`SNP_GO_JSON_URL`), never from an `http://` mirror allowed by
    /// `SNP_ALLOW_INSECURE_MIRROR` — a non-TLS mirror then only serves bytes that the TLS manifest verifies.
    pub fn resolve(tool: Tool, version: &str, target: &str, f: &dyn super::Fetch) -> Result<Artifact, super::NetError>;
    pub const UV_PINNED: &str;                  // + embedded sha256 per target triple (generated table)
    pub fn host_target() -> &'static str;       // e.g. "x86_64-unknown-linux-gnu" (musl/riscv64 aware)
}
```

#### 1.4.10 `snp-lang` (framework W1 WP-16; languages W2 WP-24/WP-25) — planning + install, never runs hooks

```rust
use std::{ffi::OsString, io, path::{Path, PathBuf}, sync::Arc};
use snp_core::{cancel::CancelToken, env::{Env, EnvPatch}, failure::Failure, lang::LanguageId, output::Printer};
/// The only way backends run setup/probe commands (S12): real impl = normalize_cmd against the CHILD env +
/// snp_proc::spawn::capture; tests use `harness::ShimRunner`, which records argv/cwd/env and replays scripted outputs.
pub trait CmdRunner: Send + Sync + std::fmt::Debug {
    fn run(&self, argv: &[OsString], env: &Env, cwd: &Path) -> Result<snp_proc::spawn::Captured, LangError>;
}
#[derive(Debug)] pub struct ProcRunner { pub cancel: CancelToken }   // spawns with Role::Tool, Group::Inherit
impl CmdRunner for ProcRunner {}
/// Implemented over the store by `snp_repo::StoreTools` (WP-31); tests use a temp-dir impl.
pub trait ToolDirs: Send + Sync + std::fmt::Debug {
    fn tool_dir(&self, tool: &str, version: &str) -> PathBuf;
    /// `tools-v1/<tool>.lock`; `on_wait` prints the brand's `Locking … directory` line once per process.
    fn lock_tool(&self, tool: &str, c: &CancelToken) -> Result<snp_os::lock::FileLock, Failure>;
    fn cache_dir(&self, name: &str) -> PathBuf;                 // isolated tool caches (uv, go, npm, …)
}
/// Default-version and exe-lookup memoisation for one process, plus persistent facts in `cache-v1/probes` only where
/// sound (§6.1, ADR-079): a fact is persisted only when the resolved executable is a native binary (not a `#!` script)
/// and not a known proxy (pyenv/asdf/rbenv/nodenv shims, mise and rustup proxies, macOS `/usr/bin/*` xcrun shims); its key
/// is the realpath + (len, mtime_ns, ino) of that binary plus a digest of the env that steers it (PATH, PYENV_VERSION,
/// ASDF_*, MISE_*, RUSTUP_TOOLCHAIN, DEVELOPER_DIR, VIRTUAL_ENV, CONDA_PREFIX). Everything else is memoised per process.
#[derive(Debug, Default)] pub struct ToolProbe { /* OnceLock caches; optional persistence dir */ }
impl ToolProbe {
    pub fn new(persist_dir: Option<PathBuf>) -> Self;
    /// Memoised `Language::default_version` result for `lang` under `env`; `probe` runs at most once per process.
    pub fn default_version(&self, lang: LanguageId, env: &Env, cwd: &Path,
                           probe: &mut dyn FnMut() -> Result<Arc<str>, LangError>) -> Result<Arc<str>, LangError>;
    /// `find_executable` memoised per (PATH, PATHEXT, name).
    pub fn find_exe(&self, name: &std::ffi::OsStr, env: &Env, cwd: &Path) -> Option<PathBuf>;
    /// (len, mtime, ino/index) of an executable for health fast paths; None when it cannot be stat'ed.
    pub fn fingerprint(&self, exe: &Path) -> Option<Fingerprint>;
}
#[derive(Debug)] pub struct Host<'a> {
    pub base_env: &'a Env, pub cwd: &'a Path, pub self_exe: &'a Path, pub probe: &'a ToolProbe,
    pub fetch: Option<&'a dyn snp_net::Fetch>, pub tools: &'a dyn ToolDirs, pub runner: &'a dyn CmdRunner,
    pub printer: &'a Printer, pub cancel: &'a CancelToken, pub brand: snp_core::brand::Brand,
}
pub struct EnvCtx<'a> { pub host: &'a Host<'a>, pub prefix: &'a Path, pub version: &'a str, pub env_dir: &'a Path }
pub struct InstallCtx<'a> { pub cx: EnvCtx<'a>, pub deps: &'a [Arc<str>], pub is_local: bool }
pub struct PlanCtx<'a> { pub host: &'a Host<'a>, pub prefix: &'a Path, pub version: &'a str, pub env_dir: Option<&'a Path>,
    pub env: &'a Env, pub entry: &'a str, pub args: &'a [Arc<str>], pub is_local: bool, pub color: bool,
    pub docker_tag: Option<&'a str> }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum Health { Healthy, Unhealthy(String) }
/// `tools`: the shared toolchains this env links to (`("node", "22.11.0")`, `("uv", …)`) — passed to
/// `Store::commit_install_marker` as store-owned references, so `sweep` never parses InstallState (ADR-070).
#[derive(Debug, Clone)] pub struct InstallOutcome { pub installer: Arc<str>, pub fingerprint: Option<Fingerprint>,
                                                    pub tools: Vec<(Arc<str>, Arc<str>)> }
/// Persisted as `.snp-install-state.json` (committed last by snp-store); malformed ⇒ not installed (X13-ENV-01).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)] pub struct InstallState { pub v: u32, pub language: Arc<str>,
    pub language_version: Arc<str>, pub deps: Arc<[Arc<str>]>, pub installer: Arc<str>, pub fingerprint: Option<Fingerprint>,
    pub snp_version: Arc<str> }
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)] pub struct Fingerprint { pub exe: PathBuf,
    pub len: u64, pub mtime_ns: i128, pub ino_or_index: u64, pub detail: Arc<str> }
impl InstallState { pub fn to_json(&self) -> Vec<u8>; pub fn from_json(b: &[u8]) -> Option<Self>; }
/// What to run; the executor (snp-run) applies shuffle/partitioning/pool/pty uniformly.
#[derive(Debug, Clone)] pub enum RunPlan {
    Xargs { cmd: Arc<[OsString]>, env: Env, shuffle: bool, serial: bool, pre: Vec<PreStep> },
    Fail { entry: Arc<str> },                          // in-process, no spawn
    Pygrep { args: Arc<[Arc<str>]>, entry: Arc<str> }, // in-process native, no shuffle, serial partitions
    Error { code: i32, output: Vec<u8> },              // C-hook rows (X13-EXE-01..04): "Executable `` not found", …
}
/// e.g. docker's per-run `docker build` (skipped when `skip_if_ok` succeeds; ADR-035).
#[derive(Debug, Clone)] pub struct PreStep { pub argv: Vec<OsString>, pub env: Env, pub cwd: PathBuf, pub skip_if_ok: Option<Vec<OsString>> }
#[derive(Debug)] pub enum LangError {
    Fatal(String), Assertion(String), Value(String),
    CalledProcess { cmd: Vec<OsString>, rc: i64, stdout: Vec<u8>, stderr: Vec<u8> },
    Io { op: &'static str, path: PathBuf, err: io::Error }, Net(snp_net::NetError), Interrupted,
}
impl From<LangError> for Failure {}     // python type + exit code (1 Fatal, 130 Interrupted, else 3)
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum VersionSupport { DefaultOnly, Any, Ignored }
#[derive(Copy, Clone, Debug)] pub struct Support { pub version: VersionSupport, pub deps: bool }
pub trait Language: Send + Sync + 'static {
    fn id(&self) -> LanguageId;
    fn support(&self) -> Support;
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError>;        // memoised by ToolProbe
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError>;   // env_dir does NOT exist on entry
    fn health(&self, cx: &EnvCtx<'_>, recorded: Option<&InstallState>) -> Result<Health, LangError>;
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError>;           // in_env as data
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError>;
}
pub fn backend(id: LanguageId) -> &'static dyn Language;                    // static match over the ZSTs in `langs`
/// Verbatim assert_version_default / assert_no_additional_deps texts (brand-aware); run BEFORE any FS mutation (04b §5.1).
pub fn check_support(id: LanguageId, version: &str, deps: &[Arc<str>], brand: snp_core::brand::Brand) -> Result<(), LangError>;
pub const LOCAL_TEMPLATE: &[(&str, &[u8])];                                    // 4.6.2 empty_template_* bytes (03 §7.7)
pub use snp_core::profile::LOCAL_TEMPLATE_VERSION;                              // single definition (snp-store keys use it)
pub mod common {   // shared helpers, owned by the framework (WP-16)
    pub fn exe_exists(name: &std::ffi::OsStr, env: &Env, cwd: &Path) -> bool;      // lang_base.exe_exists
    pub fn setup_cmd(r: &dyn super::CmdRunner, argv: &[OsString], env: &Env, cwd: &Path) -> Result<Vec<u8>, super::LangError>;
    pub fn bin_dir(env_dir: &Path) -> PathBuf;                                     // bin | Scripts
    pub fn docker_tag(repo_key_hex: &str) -> String;                               // "pre-commit-" + md5(hex) (ADR-035)
    pub fn rmtree(path: &Path, root: &Path) -> std::io::Result<()>;               // readonly-aware, contained
}
pub mod langs {    // one ZST per language; W0 stubs, implemented by WP-16 (envless), WP-24 (group 1), WP-25 (group 2)
    pub mod envless { pub struct Fail; pub struct Pygrep; pub struct Unsupported; pub struct UnsupportedScript; pub struct DockerImage; }
    pub mod python  { pub struct Python; }    pub mod node { pub struct Node; }     pub mod golang { pub struct Golang; }
    pub mod rust    { pub struct Rust; }      pub mod ruby { pub struct Ruby; }     pub mod docker { pub struct Docker; }
    pub mod conda   { pub struct Conda; }     pub mod coursier { pub struct Coursier; } pub mod dart { pub struct Dart; }
    pub mod dotnet  { pub struct Dotnet; }    pub mod haskell { pub struct Haskell; } pub mod julia { pub struct Julia; }
    pub mod lua     { pub struct Lua; }       pub mod perl { pub struct Perl; }     pub mod r { pub struct R; }
    pub mod swift   { pub struct Swift; }
}
#[cfg(feature = "testing")] pub mod harness {   // 04b §5.12 shim harness: golden argv/cwd/env logs per language
    #[derive(Debug, Default)] pub struct ShimRunner { /* scripted responses keyed by argv[0] basename; records calls */ }
    impl super::CmdRunner for ShimRunner {}
    #[derive(Debug, Clone)] pub struct Recorded { pub argv: Vec<OsString>, pub cwd: PathBuf, pub env_delta: Vec<(OsString, Option<OsString>)> }
}
```

#### 1.4.11 `snp-pyparsers` (W1, WP-17) — CPython-compatible validators for native hooks (09 §9.1, §9.4–9.5)

```rust
pub mod json {
    #[derive(Copy, Clone, Debug)] pub enum PyMinor { P312, P313 }
    #[derive(Debug, Clone, PartialEq)] pub enum JsonValue { Null, Bool(bool), Int(Box<str>), Float(f64), Str(String),
        Array(Vec<JsonValue>), Object(Vec<(String, JsonValue)>) /* order kept; duplicate keys per object_pairs_hook */ }
    pub fn check(bytes: &[u8], v: PyMinor) -> Result<(), String>;                 // exact json.JSONDecodeError text
    pub fn loads(bytes: &[u8], v: PyMinor) -> Result<JsonValue, String>;          // iterative, CPython depth thresholds
    #[derive(Clone, Debug)] pub struct DumpOpts { pub indent: String, pub ensure_ascii: bool, pub sort_keys: bool, pub top_keys: Vec<String> }
    pub fn dumps_pretty(v: &JsonValue, o: &DumpOpts) -> String;                   // pretty-format-json encoder
}
pub mod yamlcheck {
    #[derive(Copy, Clone, Debug)] pub enum Mode { Safe, AllowMultipleDocuments, Unsafe }
    pub fn check(text: &str, m: Mode) -> Result<(), String>;                       // ruamel.yaml ≥ 0.19 messages
}
pub mod toml { pub fn check(bytes: &[u8]) -> Result<(), String>; }                 // tomllib (CPython 3.13)
pub mod xml { pub fn check(bytes: &[u8]) -> Result<(), String>; }                  // expat well-formedness
pub mod difflib { pub fn unified_diff(a: &[&str], b: &[&str], from: &str, to: &str) -> String; }
pub mod ini { pub fn aws_credentials(text: &str) -> Result<Vec<(String, String)>, String>; } // configparser subset
```

#### 1.4.12 `snp-hooks-data` (W1, WP-17) — generated data, no logic beyond lookups

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct Epoch(pub u16);
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum HookClass { Native, NativeWithCaveats, Delegate, FailLanguage, Stub }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Gate { Exact, Caveat, Delegate }    // CI parity-gate result (09 §9.9)
#[derive(Debug)] pub struct TagProfile { pub tag: &'static str, pub commit: &'static str, pub manifest_yaml: &'static str,
                                         pub hooks: &'static [(&'static str /*id*/, HookClass, Epoch)] }
pub fn is_pre_commit_hooks_url(url: &str) -> bool;                 // 09 §6.2 normalisation; never logs userinfo
pub fn profile_for_rev(rev: &str) -> Option<&'static TagProfile>;  // exact tag or full 40-hex commit of a tag (Tier A)
pub fn verified_blob(blob_hex: &str) -> Option<(&'static str /*module*/, Epoch)>;   // Tier B
pub fn module_for_entry(entry: &str) -> Option<&'static str>;     // console-script table (setup.cfg)
pub fn import_closure(module: &str) -> &'static [&'static str];
/// Lookup in the generated table `src/generated/parity_gates.rs` (initial version by WP-17: Exact for NATIVE ids,
/// Delegate for caveat ids; regenerated by WP-27's `xtask pch-gates`). No TOML at run time, no build script.
pub fn parity_gate(hook_id: &str) -> Gate;
pub const BUILTIN_MANIFEST_YAML: &str;                             // `repo: builtin` manifest (= v6.0.0 for shared ids)
pub const GENERATED_FROM: &str;                                    // "pre-commit-hooks v3.0.0..v6.0.0 @ <sha>"
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)] pub enum NativeSource { PreCommitHooks, Builtin }
```

#### 1.4.13 `snp-git` (W1, WP-18)

```rust
use std::{ffi::{OsStr, OsString}, io, path::{Path, PathBuf}, sync::Arc};
use snp_core::{env::Env, failure::Failure, output::Printer, path::RepoPath};
use snp_proc::spawn::{Captured, PyStatus, SpawnError, Stdin, Group};
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)] pub struct GitVersion { pub major: u16, pub minor: u16, pub patch: u16 }
#[derive(Clone, Debug)] pub struct GitCaps { pub version: Option<GitVersion>, pub ls_files_dedup: bool, pub path_format: bool,
    pub config_env: bool, pub init_object_format: bool, pub config_hooks: bool }
/// `git` resolved once via normalize_cmd(["git"]) so CalledProcessError argv matches pre-commit (02 G.10).
#[derive(Debug)] pub struct GitExe { pub argv0: Vec<OsString>, pub caps: GitCaps }
impl GitExe {
    /// Version from `cache` when (exe realpath, size, mtime, GIT_EXEC_PATH) matches a passing entry, else
    /// `git --version`. Never persisted for a known proxy (macOS `/usr/bin/git` xcrun shim, scoop/asdf shims; ADR-079).
    /// `Git::discover` runs after this call (it needs `caps` for the 2.19–2.30 fallback); a cache hit costs one stat.
    pub fn resolve(env: &Env, cwd: &Path, cache: Option<&Path>) -> Result<GitExe, Failure>;
    /// Never refuses (ADR-063): < 2.31 → one stderr warning per process unless SNP_ALLOW_OLD_GIT (D-DIAG-01) and the
    /// capability fallbacks; the < 2.19 stash refusal lives in snp-stash (D-DATA-17); config hooks need ≥ 2.54.
    pub fn version_gate(&self, printer: &Printer, env: &Env) -> Result<(), Failure>;
}
/// Seam (S12): real = snp-proc; tests = `testing::ScriptedBackend` (fail any call by argv pattern, record calls).
pub trait GitBackend: Send + Sync + std::fmt::Debug {
    fn capture(&self, argv: &[OsString], env: &Env, cwd: &Path, stdin: Stdin, group: Group) -> Result<Captured, SpawnError>;
    fn stream(&self, argv: &[OsString], env: &Env, cwd: &Path, sink: &mut dyn io::Write) -> Result<(PyStatus, Vec<u8>), SpawnError>;
    fn inherit(&self, argv: &[OsString], env: &Env, cwd: &Path, stdin: Stdin) -> Result<PyStatus, SpawnError>;
}
/// `role` is used for every spawn: the run's backend uses Role::Tool with the run token; the stash builds its own
/// backend with Role::Critical and a fresh token that is never cancelled (ADR-065).
#[derive(Debug)] pub struct ProcBackend { pub cancel: snp_core::cancel::CancelToken, pub role: snp_proc::spawn::Role }
impl GitBackend for ProcBackend {}
#[derive(Clone, Debug)] pub struct RepoInfo { pub root: PathBuf, pub git_dir: PathBuf, pub common_dir: PathBuf,
    pub index_file: PathBuf, pub is_worktree: bool }
/// NUL-separated byte buffer + spans, git order preserved, never sorted (02 G.0 #8).
#[derive(Debug, Clone, Default)] pub struct ZList { /* buf: Vec<u8>, spans: Vec<(u32, u32)> */ }
impl ZList { pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool; pub fn get(&self, i: usize) -> Option<&[u8]>;
             pub fn iter(&self) -> impl Iterator<Item = &[u8]>; pub fn into_parts(self) -> (Vec<u8>, Vec<(u32, u32)>); }
#[derive(Clone, Debug, PartialEq, Eq)] pub struct ObjectId(/* Box<str>: 40 or 64 hex */);
#[derive(Debug)] pub enum DiffIndex { Clean /* rc 0 */, CrlfQuirk /* rc 1, blank */, Patch { bytes_written: u64 } }
#[derive(Copy, Clone, PartialEq, Eq, Debug)] pub struct DiffFingerprint { pub digest: [u8; 32], pub non_empty: bool }
impl DiffFingerprint { pub const EMPTY: DiffFingerprint; }
#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum MergeState { None, Merging }
#[derive(Debug, Clone)] pub struct PrePushNs { pub all_files: bool, pub from_ref: Option<OsString>, pub to_ref: Option<OsString>,
                       pub local_branch: OsString, pub remote_branch: OsString }
/// Index entries as an arena (ADR-079): `ls-files -s -z` (all tracked: DiffOracle + git-mode tags) or
/// `diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB` (staged only: git-mode tags when Stat mode is impossible).
/// `modes[i]` belongs to `paths.get(i)`; one buffer, no per-path allocation.
#[derive(Debug, Clone, Default)] pub struct IndexEntries { pub paths: ZList, pub modes: Box<[u32]> }

#[derive(Clone, Debug)] pub struct Git { /* exe: Arc<GitExe>, env: Env, root: Arc<Path>, backend: Arc<dyn GitBackend> */ }  // cheap clone
impl Git {
    /// One `git rev-parse --show-cdup --is-inside-git-dir --git-dir --git-common-dir --git-path index` (fallback to separate
    /// calls on 2.19–2.30 or on newline-containing paths); FatalError texts verbatim (02 §3).
    pub fn discover(exe: Arc<GitExe>, env: &Env, cwd: &Path, backend: Arc<dyn GitBackend>) -> Result<(Git, RepoInfo), Failure>;
    pub fn with_env(&self, env: Env) -> Git;                         // run vars exported (pre-commit sets os.environ)
    pub fn with_backend(&self, b: Arc<dyn GitBackend>) -> Git;       // the stash's Critical backend (ADR-065)
    pub fn env(&self) -> &Env; pub fn root(&self) -> &Path;
    // ---- user-repo calls: exact pre-commit argv (02 G.1) + only the G.7 rows 1–3 additions ----
    pub fn has_unmerged(&self) -> Result<bool, Failure>;
    pub fn config_is_unstaged(&self, cfg: &OsStr) -> Result<bool, Failure>;          // rc == 1 exactly
    pub fn intent_to_add(&self) -> Result<ZList, Failure>;                            // + --no-renames (hazard b)
    pub fn rm_cached(&self, paths: &ZList, group: Group) -> Result<(), Failure>;      // xargs-partitioned, rc ignored
    pub fn add_intent_to_add(&self, paths: &ZList, group: Group) -> Result<(), Failure>;
    pub fn write_tree(&self) -> Result<ObjectId, Failure>;
    /// GIT_DIFF_OPTS removed (hazard a); stdout streamed into `sink`; FatalError text for other rcs (02 §7).
    pub fn diff_index_patch(&self, tree: &ObjectId, sink: &mut dyn io::Write) -> Result<DiffIndex, Failure>;
    pub fn checkout_all(&self, skip_post_checkout: bool, group: Group) -> Result<(), Failure>; // -c submodule.recurse=0 checkout -- .
    pub fn apply(&self, patch: &Path, autocrlf_false: bool, group: Group) -> Result<bool, Failure>;
    pub fn apply_check(&self, patch: &Path) -> Result<bool, Failure>;                 // stage_fixed (snp-only)
    pub fn add_literal(&self, paths: &[RepoPath], group: Group) -> Result<(), Failure>; // stage_fixed (snp-only)
    pub fn staged_files(&self) -> Result<ZList, Failure>;                             // + --no-renames (output-neutral)
    pub fn all_files(&self) -> Result<ZList, Failure>;                                // --deduplicate, or adjacent dedup on < 2.31 (D-CRASH-26)
    pub fn tracked_with_modes(&self) -> Result<IndexEntries, Failure>;                // ls-files -s -z
    pub fn staged_with_modes(&self) -> Result<IndexEntries, Failure>;                 // diff --staged --raw -z --no-renames …
    /// Entry count from the index header (`DIRC`, version, count; 12 bytes read, no spawn) — Stat-mode gate.
    pub fn index_entry_count(&self, info: &RepoInfo) -> Option<u32>;
    pub fn changed_files(&self, from: &OsStr, to: &OsStr) -> Result<ZList, Failure>;
    pub fn merge_state(&self, info: &RepoInfo) -> MergeState;                         // stat MERGE_MSG ∧ MERGE_HEAD
    pub fn conflicted_files(&self, info: &RepoInfo) -> Result<ZList, Failure>;        // first-seen order union; MERGE_MSG on bytes
    pub fn diff_fingerprint(&self) -> Result<DiffFingerprint, Failure>;              // streaming blake3, O(1) memory
    pub fn show_diff(&self, color: bool) -> Result<(), Failure>;                     // inherits stdout
    pub fn hooks_path(&self) -> Result<Option<OsString>, Failure>;
    pub fn config_origins(&self) -> Result<Vec<PathBuf>, Failure>;                   // `config --list --show-origin -z` files
    pub fn config_get(&self, key: &str) -> Result<Option<OsString>, Failure>;
    pub fn config_get_regexp(&self, pattern: &str) -> Result<Vec<(OsString, OsString)>, Failure>;
    pub fn pre_push(&self, stdin: &[u8], remote_name: &OsStr) -> Result<Option<PrePushNs>, Failure>; // 02 §16.3 + X13-HI-04
    /// Raw call with the hook env for pre-commit-hooks emulation (exact upstream argv, 09 §3, §9.6).
    pub fn run_raw(&self, args: &[&OsStr]) -> Result<Captured, Failure>;
}
/// snp-owned repositories: env = no_git_env(base) + GIT_CONFIG_COUNT{core.fsmonitor=, core.useBuiltinFSMonitor=false}.
#[derive(Clone, Debug)] pub struct OwnedRepo { /* exe, env, dir, backend */ }
#[derive(Clone, Debug)] pub struct RevInfo { pub repo: String, pub rev: String, pub frozen: Option<String>, pub hook_ids: Vec<String> }
impl OwnedRepo {
    /// pre-commit's exact clone sequence (03 §7.6) + one object-format retry on `mismatched algorithms` (D-SEC-06).
    /// `CloneMode::Foreground` = pre-commit's env and prompts; `Prefetch` (background attempts only, ADR-064) adds
    /// GIT_TERMINAL_PROMPT=0, GIT_ASKPASS= (empty: no askpass GUI), GCM_INTERACTIVE=never and, for ssh/scp URLs,
    /// GIT_SSH_COMMAND='ssh -o BatchMode=yes'; its stderr is never surfaced.
    pub fn clone_rev(exe: Arc<GitExe>, base: &Env, dest: &Path, repo: &str, rev: &str, mode: CloneMode,
                     b: Arc<dyn GitBackend>) -> Result<OwnedRepo, Failure>;
    /// `git clone --local --no-checkout <src> <dest>` + checkout <rev> (+ shallow submodules); falls back to clone_rev.
    pub fn clone_local(exe: Arc<GitExe>, base: &Env, src: &Path, dest: &Path, rev: &str, b: Arc<dyn GitBackend>) -> Result<OwnedRepo, Failure>;
    pub fn open(exe: Arc<GitExe>, base: &Env, dir: &Path, b: Arc<dyn GitBackend>) -> OwnedRepo;
    pub fn ls_tree_blobs(&self, rev: &str, dir: &str) -> Result<Vec<(String, String)>, Failure>;
    pub fn read_blob(&self, rev: &str, path: &str) -> Result<Vec<u8>, Failure>;
    pub fn rev_info(&self, repo: &str, tags_only: bool, freeze: bool) -> Result<RevInfo, Failure>;   // autoupdate 03 §11.2
    pub fn shadow_try_repo(&self, user: &Git, tmp: &Path) -> Result<(PathBuf, String), Failure>;   // try-repo 03 §12
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum CloneMode { Foreground, Prefetch }
/// Background prefetch is allowed for https://, file:// and local paths; for ssh/scp URLs only when none of
/// GIT_SSH_COMMAND, GIT_SSH and `core.sshCommand` is set (BatchMode can then be injected); never otherwise (ADR-064).
pub fn prefetch_eligible(url: &str, env: &Env, core_ssh_command_set: bool) -> bool;
pub fn no_git_env(env: &Env) -> Env;                                                    // git.py:27-48 allowlist (Windows: names upper-cased, §2.5)
/// Per-run cache of the git facts pre-commit-hooks compute, with their exact upstream argv (non `-z`, quotePath and
/// renames honoured: bug-compat) and the hook env (09 §3, §9.6). Used by snp-hooks natives.
pub mod pch_facts {
    #[derive(Debug)] pub struct PchFacts { /* Git + OnceLocks */ }
    impl PchFacts {
        pub fn new(git: super::Git) -> Self;
        pub fn added_files(&self) -> Result<std::sync::Arc<[Vec<u8>]>, Failure>;      // diff --staged --name-only --diff-filter=A
        pub fn tracked_files(&self) -> Result<std::sync::Arc<[Vec<u8>]>, Failure>;    // ls-files (quoted, non -z)
        pub fn index_modes(&self) -> Result<std::sync::Arc<[(u32, Vec<u8>)]>, Failure>; // ls-files --stage
        pub fn lfs_files(&self, paths: &[&[u8]]) -> Result<Vec<Vec<u8>>, Failure>;    // check-attr filter (09 §4.E); whole-index memo (ADR-082)
        pub fn current_branch(&self) -> Result<Option<String>, Failure>;              // symbolic-ref HEAD
        pub fn raw(&self, args: &[&OsStr]) -> Result<Captured, Failure>;               // any other catalogued call (rev-parse --git-dir memoised, ADR-082)
    }
}
/// 11 §A.5.1 hook-script classifier (pure, bounded file reads: ≤ 1 MiB, multi-pattern scan; lowest class rank wins).
/// Used by snp-cmd (install/uninstall/doctor) and by snp-run's `run` hint (ADR-070).
pub mod hookscripts {
    #[derive(Debug, Clone, PartialEq, Eq)] pub enum Owner { Missing, NotAFile, Snp { stale: bool }, SnpV1,
        PreCommit { era: Option<u8> }, Prek, Lefthook, Foreign }
    pub const SNP_SCRIPT_ID: &str = "5bb9bd89cf6c92c58fee7677ceaf4fc3";
    pub const SNP_PRIOR_IDS: &[&str];
    pub fn classify(path: &Path) -> io::Result<Owner>;
    impl Owner { pub fn is_manager(&self) -> bool; pub fn owner_name(&self) -> Option<&'static str>; } // "pre-commit" | "prek" | "snp 1.x"
}
#[cfg(feature = "testing")] pub mod testing {
    #[derive(Debug, Default)] pub struct ScriptedBackend { /* rules: (argv glob, Response); call log */ }
    #[derive(Debug, Clone)] pub enum Response { Real, Exit { code: i64, stdout: Vec<u8>, stderr: Vec<u8> }, SpawnError(i32), Panic }
    impl ScriptedBackend { pub fn new() -> Self; pub fn on(&self, argv_glob: &str, r: Response) -> &Self; pub fn calls(&self) -> Vec<Vec<OsString>>; }
    impl super::GitBackend for ScriptedBackend {}
}
```

#### 1.4.14 `snp-testkit` (W1, WP-18, dev-only) — 10 §7.3

```rust
use std::{ffi::{OsStr, OsString}, path::{Path, PathBuf}};
/// Binaries under test. Library code cannot read `CARGO_BIN_EXE_*` (it exists only in the owning package's tests):
/// callers pass `env!("CARGO_BIN_EXE_snp")`, or `Bins::locate()` finds them next to the test executable in the target
/// dir (CI runs `cargo build --workspace --bins` first).
#[derive(Debug, Clone)] pub struct Bins { pub snp: PathBuf, pub probe: PathBuf }
impl Bins { pub fn locate() -> Option<Bins>; }
#[derive(Debug)] pub struct TestEnv { /* canonicalised temp root, hermetic env (10 §7.3.2), fixed GIT_*_DATE counter */ }
impl TestEnv {
    pub fn new(bins: Bins) -> Self;
    pub fn root(&self) -> &Path;
    pub fn env(&self) -> &snp_core::env::Env;                       // child env only; never the process env
    pub fn with_env(&self, k: &str, v: impl Into<OsString>) -> TestEnv;   // derived env (same root)
    pub fn git_repo(&self, name: &str) -> RepoBuilder;
    pub fn fixture_repo(&self, fixture: Fixture) -> PathBuf;        // recreation of a pre-commit testing/resources repo
    pub fn snp(&self, cwd: &Path) -> Cmd;                           // bins.snp (snp brand)
    pub fn snp_as_pre_commit(&self, cwd: &Path) -> Cmd;             // through a `pre-commit` symlink/copy (compat brand)
    pub fn oracle(&self, which: Oracle, cwd: &Path) -> Option<Cmd>; // SNP_ORACLE_DIR/target/oracle; never PATH
    pub fn filters(&self) -> Vec<(String, String)>;                 // insta filters (10 §7.3.4)
    pub fn probe(&self) -> &Path;                                   // bins.probe
}
#[derive(Copy, Clone, Debug)] pub enum Oracle { Pc462Py312, Pc462Py313 }
/// One variant per repository of pre-commit's `testing/resources` (10 §3, recreated verbatim under
/// `crates/snp-testkit/resources/<name>/`), plus the conftest scenario helpers of 10 §7.3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Fixture {
    Resource(&'static str),                         // e.g. "script_hooks_repo", "python_hooks_repo" (10 §3 names)
    MergeConflict, ConflictingSubmodule, CommitMsgRepo, PrepareCommitMsgRepo { failing: bool }, PushExample,
    ConsumingRepo(&'static str), OutOfDate { tagged: bool }, HookDisappearing, ImgStaged, SubStaged }
/// Deterministic repository builder (identical SHAs across runs: fixed dates, author, umask, core.autocrlf=false).
#[derive(Debug)] pub struct RepoBuilder { /* … */ }
impl RepoBuilder {
    pub fn file(self, path: &str, bytes: &[u8]) -> Self;  pub fn exec_file(self, path: &str, bytes: &[u8]) -> Self;
    pub fn symlink(self, path: &str, target: &str) -> Self;  pub fn stage(self, path: &str) -> Self;
    pub fn stage_all(self) -> Self;  pub fn commit(self, msg: &str) -> Self;  pub fn tag(self, name: &str) -> Self;
    pub fn config(self, key: &str, value: &str) -> Self;  pub fn build(self) -> PathBuf;
}
/// A process invocation with the hermetic env.
#[derive(Debug)] pub struct Cmd { /* … */ }
impl Cmd {
    pub fn arg(self, a: impl AsRef<OsStr>) -> Self;  pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(self, a: I) -> Self;
    pub fn env(self, k: &str, v: impl AsRef<OsStr>) -> Self;  pub fn env_remove(self, k: &str) -> Self;
    pub fn stdin(self, bytes: &[u8]) -> Self;  pub fn tty(self) -> Self;  // stdout/stderr on a pty (posix)
    pub fn output(self) -> Output;
}
#[derive(Debug)] pub struct Output { pub code: i32, pub stdout: Vec<u8>, pub stderr: Vec<u8> }
#[macro_export] macro_rules! snp_snapshot { ($env:expr, $out:expr) => { /* insta with filters */ } }
```

#### 1.4.15 `snp-config` (W2, WP-21)

```rust
use std::{ffi::{OsStr, OsString}, path::{Path, PathBuf}, sync::Arc};
use snp_core::{brand::Brand, env::Env, failure::Failure, lang::LanguageId, output::{WarnSink, DiagSink},
               path::RepoPath, stage::{HookType, StageSet}};
use file_identify::TagSet;
#[derive(Clone, Debug)] pub struct ConfigSource { pub path: PathBuf, pub display: OsString, pub project_root: PathBuf }
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum ExtMode { Enabled, Strict }   // Strict = SNP_COMPAT_STRICT=1
#[derive(Debug)] pub struct LoadOpts<'a> { pub brand: Brand, pub ext: ExtMode, pub regexes: &'a snp_pyre::RegexCache,
                          pub warn: &'a dyn WarnSink /* "[WARNING] …" stdout, cfgv item order */,
                          pub diag: &'a dyn DiagSink /* regex FutureWarnings, snp notes → stderr */ }
#[derive(Debug, Clone)] pub struct ConfigError { pub manifest: bool, pub rendered: String /* "\n==> File …\n=====> leaf" */ }
impl From<ConfigError> for Failure {}        // Exc{Fatal, InvalidConfigError|InvalidManifestError}
pub fn load_config(src: &ConfigSource, o: &LoadOpts<'_>) -> Result<Config, ConfigError>;
/// `display` is converted with `to_string_lossy` for cfgv's `load_from_filename_with(…, display_filename: Option<&str>, …)`.
pub fn load_manifest(path: &Path, display: &OsStr, o: &LoadOpts<'_>) -> Result<Manifest, ConfigError>;
pub fn load_manifest_bytes(bytes: &[u8], display: &OsStr, o: &LoadOpts<'_>) -> Result<Manifest, ConfigError>; // Tier A data
/// pre-commit's schemas reproduced item by item with cfgv 0.3 MapItem impls (01 §3–§5). `repo: builtin` validation
/// (no `rev`; `entry`/`additional_dependencies` forbidden; `language` absent or `system`) is part of the config schema
/// as cfgv MapItems with cfgv-style contexts and messages (ADR-070); the unknown-id check stays in snp-repo's merge.
pub mod schema {
    pub fn config_schema(brand: snp_core::brand::Brand, ext: super::ExtMode) -> &'static cfgv::Schema;
    pub fn manifest_schema(brand: snp_core::brand::Brand) -> &'static cfgv::Schema;
}
#[derive(Debug, Clone)] pub struct Config {
    pub source: ConfigSource,
    pub repos: Vec<RepoConfig>, pub default_install_hook_types: Vec<HookType>,
    pub default_language_version: indexmap::IndexMap<LanguageId, Arc<str>>, pub default_stages: StageSet,
    pub files: Arc<str>, pub exclude: Arc<str>, pub fail_fast: bool, pub minimum_pre_commit_version: Arc<str>,
    pub root_ext: ext::RootExt, pub raw: cfgv::Value,        // raw = validated+defaulted value (meta hooks, dump)
}
#[derive(Debug, Clone)] pub enum RepoConfig {
    Remote { repo: Arc<str>, rev: Arc<str>, hooks: Vec<ConfigHook> },
    Local { hooks: Vec<HookDef> }, Meta { hooks: Vec<HookDef> }, Builtin { hooks: Vec<ConfigHook> },
}
/// A config hook of a remote/builtin repo: config-level dict (stage/language transforms applied, NO defaults),
/// kept raw because check-useless-excludes reads it unmerged (01 §9.2) and merging is whole-key override.
#[derive(Debug, Clone)] pub struct ConfigHook { pub id: Arc<str>, pub raw: cfgv::Map, pub ext: ext::HookExt }
/// Fully defaulted manifest-style hook (hook.py 22 fields). Strings Arc<str>; lists Arc<[…]>.
#[derive(Debug, Clone)] pub struct HookDef {
    pub id: Arc<str>, pub name: Arc<str>, pub entry: Arc<str>, pub language: LanguageId, pub alias: Arc<str>,
    pub files: Arc<str>, pub exclude: Arc<str>, pub types: TagSet, pub types_or: TagSet, pub exclude_types: TagSet,
    pub additional_dependencies: Arc<[Arc<str>]>, pub args: Arc<[Arc<str>]>, pub always_run: bool, pub fail_fast: bool,
    pub pass_filenames: bool, pub description: Arc<str>, pub language_version: Arc<str>, pub log_file: Arc<str>,
    pub minimum_pre_commit_version: Arc<str>, pub require_serial: bool, pub stages: StageSet, pub stages_set: bool,
    pub verbose: bool, pub extra_keys: Vec<Arc<str>>, pub ext: ext::HookExt,
}
#[derive(Debug, Clone)] pub struct Manifest { pub hooks: Vec<HookDef> }   // by-id lookup: last duplicate wins
impl Manifest { pub fn get(&self, id: &str) -> Option<&HookDef>; }
#[derive(Debug)] pub enum MergeError {   // `[ERROR] …` + exit 1 (01 §6.4)
    UnknownId { id: Arc<str>, repo: Arc<str> }, LanguageVersionOnEnvless { id: Arc<str>, lang: LanguageId },
    DepsOnEnvless { id: Arc<str>, lang: LanguageId } }
/// repository._hook(): manifest ∪ overrides (whole-key), language_version via default_language_version (the
/// language's own default is resolved by snp-repo with probes), empty stages ⇒ default_stages, env-less guards,
/// W11 unknown-key warning (sorted) after merging. `overlay` (§8.4) is applied last, after manifest ∪ config:
/// `{entry}` expands against the merged entry; fast-path G4 and the InstallKey see the patched HookDef.
pub fn merge_hook(m: &HookDef, c: &ConfigHook, cfg: &Config, src: &str, overlay: Option<&overlay::HookPatch>,
                  warn: &dyn WarnSink) -> Result<HookDef, MergeError>;
pub fn finalize_local(h: &HookDef, cfg: &Config, src: &str, overlay: Option<&overlay::HookPatch>,
                      warn: &dyn WarnSink) -> Result<HookDef, MergeError>;
pub mod ext {   // §8
    use std::sync::Arc;
    #[derive(Debug, Clone, Default)] pub struct HookExt { pub priority: Option<Priority>, pub env: Vec<(Arc<str>, Arc<str>)>,
        pub groups: Vec<Arc<str>>, pub stage_fixed: Option<StageFixed>, pub skip: Vec<Condition>, pub only: Vec<Condition>,
        pub timeout: Option<std::time::Duration>, pub fail_text: Option<Arc<str>>, pub root: Option<snp_core::path::RepoPath>,
        pub use_stdin: bool, pub fail_on_changes: Option<FailOnChanges> }
    #[derive(Debug, Clone)] pub struct RootExt { pub minimum_snp_version: Option<Arc<str>>,
        pub priorities: indexmap::IndexMap<Arc<str>, u32>, pub fail_on_changes: FailOnChanges, pub skip: Vec<Condition>,
        pub only: Vec<Condition>, pub allow_disable: bool, pub fast_path: FastPath, pub fast_path_exclude: Vec<Arc<str>>,
        pub fast_path_coalesce: bool, pub output: OutputMode }
    impl Default for RootExt {}   // allow_disable: true, fast_path: Auto, fail_on_changes: Always, output: Default
    #[derive(Debug, Clone)] pub enum Priority { Value(u32), Alias(Arc<str>) }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum StageFixed { Verify, Trust }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FailOnChanges { Always, Never, Ci, NonCi }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum FastPath { Off, Exact, Auto }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum OutputMode { Default, Summary, Quiet }
    #[derive(Debug, Clone)] pub enum Condition { Merge, Rebase, MergeCommit, CherryPick, Revert, Bisect, Ref(Arc<str>),
        Env(Arc<str>), Ci(bool), Run(Arc<str>), All(Vec<Condition>) }
    pub fn parse_go_duration(s: &str) -> Result<std::time::Duration, String>;
}
pub mod overlay {   // §8.4 — split: root items are applied to the Config, hook patches are data merged by merge_hook
    #[derive(Debug, Clone)] pub struct Overlay { /* hooks by alias|id, skip_groups, fail_on_changes, repos */ }
    /// One hook entry of the overlay: validated hook keys (any hook key except id/alias/repo), `entry` possibly
    /// containing `{entry}`, parsed extension keys. `keys()` counts as user config for fast-path G4 (§7.2).
    #[derive(Debug, Clone)] pub struct HookPatch { pub raw: cfgv::Map, pub ext: super::ext::HookExt }
    impl HookPatch { pub fn keys(&self) -> impl Iterator<Item = &str>; }
    /// `<config dir>/.pre-commit-config.local.y[a]ml` or SNP_LOCAL_CONFIG; ignored when tracked by git or when CI is
    /// truthy unless named by SNP_LOCAL_CONFIG; `Err(note)` = ignored with a stderr note.
    pub fn locate(cfg: &std::path::Path, env: &snp_core::env::Env, is_tracked: &dyn Fn(&std::path::Path) -> bool)
        -> Result<Option<std::path::PathBuf>, String>;
    pub fn load(path: &std::path::Path, o: &super::LoadOpts<'_>) -> Result<Overlay, super::ConfigError>;
    /// Root items only: skip_groups, fail_on_changes, appended repos; prints the active-overlay stderr line.
    pub fn apply_root(cfg: &mut super::Config, ov: &Overlay, diag: &dyn snp_core::output::DiagSink);
    impl Overlay {
        /// Keyed by alias first, else id (an id matching several hooks applies to all of them).
        pub fn hook_patch(&self, id: &str, alias: &str) -> Option<&HookPatch>;
        /// Overlay keys that matched no resolved hook → `snp: local config overrides unknown hook '<id>'` (stderr).
        pub fn unmatched<'a>(&'a self, matched: &[&str]) -> Vec<&'a str>;
    }
}
pub mod migrate {   // migrate-config (01 §10 + D-DATA-09/14/15)
    #[derive(Debug)] pub enum Outcome { Migrated(Vec<u8>), AlreadyMigrated }
    pub fn migrate_config(bytes: &[u8], display: &std::ffi::OsStr) -> Result<Outcome, super::ConfigError>;
}
pub mod revline {   // autoupdate _original_lines + _write_new_config (03 §11.3, 15 §A4.6)
    #[derive(Debug, Clone)] pub struct RevUpdate { pub repo_index: usize, pub new_rev: Arc<str>, pub frozen_comment: Option<Arc<str>> }
    #[derive(Debug)] pub enum Rewrite { Lines(Vec<u8>), FullReformat(Vec<u8>) /* PyYAML re-dump path */ }
    pub fn rewrite(original: &[u8], updates: &[RevUpdate]) -> Result<Rewrite, super::ConfigError>;
}
pub mod validate {  // validate-config / validate-manifest: per-file `print(e)` blocks, no git
    #[derive(Copy, Clone, Debug)] pub enum FileKind { Config, Manifest }
    pub fn validate_file(kind: FileKind, path: &std::ffi::OsStr, o: &super::LoadOpts<'_>, strict: bool) -> Option<String>;
}
pub const SAMPLE_CONFIG: &str;                        // byte-exact (01 §11; sha256 8d958ac8…)
pub fn effective_dump(cfg: &Config, json: bool) -> String;
pub fn json_schema() -> &'static str;                 // with extension keys
```

#### 1.4.16 `snp-store` (W2, WP-22)

```rust
use std::{ffi::OsStr, io, path::{Path, PathBuf}, sync::Arc};
use snp_core::{brand::Brand, cancel::CancelToken, env::Env, failure::Failure, lang::LanguageId};
use snp_os::lock::FileLock;
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum StoreSource { SnpHome, XdgCacheHome, PlatformDefault }
#[derive(Clone, Debug)] pub struct StoreLocation { pub path: PathBuf, pub source: StoreSource }
/// SNP_HOME > $XDG_CACHE_HOME/snp > expanduser("~/.cache")/snp (Windows %USERPROFILE%\.cache\snp); relative values
/// resolved against the invocation cwd; realpath_nonstrict'd. PRE_COMMIT_HOME is read ONLY to compute pre-commit's
/// store and refuse it (path equality or README signature) with FatalError M7 (ADR-011).
pub fn resolve_location(env: &Env, invocation_cwd: &Path) -> Result<StoreLocation, Failure>;
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)] pub struct RepoKey(pub [u8; 16]);    // hex(32) on disk
impl RepoKey { pub fn hex(&self) -> String; }
#[derive(Clone, Debug)] pub enum RepoId { Url(Arc<str>), LocalPath(PathBuf) /* canonical abs path (D-DATA-08) */ }
impl RepoId {
    /// The single derivation used by snp-repo (resolution) and snp-cmd (gc marking): `repo` names an existing local
    /// directory (relative values resolved against `project_root` = `ConfigSource::project_root` at run time, the recorded
    /// project root of a tracked config at gc time) ⇒ `LocalPath(realpath)`, else `Url(repo verbatim)` (ADR-070).
    pub fn from_config(repo: &str, project_root: &Path) -> RepoId;
}
/// store.db_repo_name: `repo` or `repo:dep1,dep2` (deps in config order, no sorting); local repos use `local[:deps]`.
pub fn db_repo_name(repo: &str, deps: &[Arc<str>]) -> String;
/// The `Initialized` marker's name: db_repo_name for a URL, `abs:<realpath>[:deps]` for a local directory (ADR-082).
pub fn marker_repo_name(id: &RepoId, repo: &str, deps: &[Arc<str>]) -> String;
/// A shared toolchain an env links to (store-owned reference format; `sweep` reads only these, never InstallState).
#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct ToolRef { pub tool: Arc<str>, pub version: Arc<str> }
/// A tracked config and the project root its relative `repo:` paths resolve against (configs-v1 records both).
#[derive(Clone, Debug, PartialEq, Eq)] pub struct TrackedConfig { pub path: PathBuf, pub project_root: PathBuf }
#[derive(Clone, Debug)] pub enum RepoKind { Remote { id: RepoId, rev: Arc<str> }, Local }
#[derive(Clone, Debug)] pub struct RepoMeta { pub kind: RepoKind, pub deps: Arc<[Arc<str>]> }
#[derive(Clone, Debug)] pub struct RepoHandle { pub key: RepoKey, pub src: PathBuf, pub envs: PathBuf, pub meta: RepoMeta }
/// Message-parity markers (ADR-032): "Initializing environment for {db_repo_name}." and the virtual install triplet.
#[derive(Clone, Debug)] pub enum MarkerKey { Initialized { db_repo_name: Arc<str>, rev: Arc<str> }, Installed { install_key: [u8; 16] } }
#[derive(Debug)] pub struct StoreGuard(/* FileLock */);
#[derive(Debug)] pub struct Store { /* location, lazy OnceLock<Created>, readonly flag */ }
impl Store {
    pub fn new(loc: StoreLocation) -> Store;                                     // no I/O (lazy, 03 Q12)
    /// try-repo's throwaway store rooted in its TempDir (ADR-059): repos-v1, keys-v1, configs-v1, patches and the
    /// global lock live under `root`; tools-v1 and cache-v1 come from `shared` (invisible: tool downloads are snp's own).
    pub fn ephemeral(root: &Path, shared: Option<&Store>) -> Store;
    pub fn location(&self) -> &StoreLocation;
    pub fn exists(&self) -> bool;
    pub fn ensure_created(&self, brand: Brand, diag: &dyn snp_core::output::DiagSink) -> Result<(), Failure>; // 0700, README, store-v1.json
    pub fn shared(&self, on_wait: &mut dyn FnMut(), c: &CancelToken) -> Result<StoreGuard, Failure>;    // runs using repos
    pub fn exclusive(&self, on_wait: &mut dyn FnMut(), c: &CancelToken) -> Result<StoreGuard, Failure>; // gc/clean
    pub fn remote_key(id: &RepoId, rev: &str, deps: &[Arc<str>]) -> RepoKey;
    pub fn local_key(deps: &[Arc<str>]) -> RepoKey;                              // includes snp_core::profile::LOCAL_TEMPLATE_VERSION
    pub fn lookup(&self, key: &RepoKey) -> Option<RepoHandle>;                   // one stat of the marker; self-heals
    /// Under the per-repo lock: re-lookup; else `fill(scratch/<rand>/src)`, write `.snp-repo.json`, rename into place.
    pub fn materialize(&self, key: &RepoKey, meta: RepoMeta, fill: &mut dyn FnMut(&Path) -> Result<(), Failure>,
                       on_wait: &mut dyn FnMut(), c: &CancelToken) -> Result<RepoHandle, Failure>;
    pub fn repo_lock(&self, key: &RepoKey, on_wait: &mut dyn FnMut(), c: &CancelToken) -> Result<FileLock, Failure>;
    pub fn tool_lock(&self, tool: &str, on_wait: &mut dyn FnMut(), c: &CancelToken) -> Result<FileLock, Failure>; // tools-v1/<tool>.lock
    pub fn env_dir(&self, repo: &RepoHandle, lang: LanguageId, version: &str) -> PathBuf;  // sanitized-or-hashed (§5.4)
    pub fn read_install_marker(env_dir: &Path) -> Option<Vec<u8>>;               // `.snp-install-state.json`
    /// Writes the store-owned `.snp-tools.json` sidecar (`tools`), then `.snp-install-state.json` (temp + fsync + rename:
    /// the commit point). `json` stays opaque to the store.
    pub fn commit_install_marker(env_dir: &Path, json: &[u8], tools: &[ToolRef]) -> io::Result<()>;
    pub fn marker_exists(&self, k: &MarkerKey) -> bool;
    pub fn set_marker(&self, k: &MarkerKey);                                      // best effort, create_new
    pub fn mark_config_used(&self, cfg_realpath: &Path, project_root: &Path);     // best effort, lock-free, never creates the store
    pub fn tracked_configs(&self) -> Vec<TrackedConfig>;
    pub fn forget_configs(&self, dead: &[PathBuf]);
    pub fn all_repos(&self) -> Vec<RepoHandle>;
    pub fn retire_repo(&self, key: &RepoKey) -> io::Result<()>;
    pub fn sweep(&self, now: std::time::SystemTime) -> SweepReport;               // scratch/trash orphans, old restored patches, tools no `.snp-tools.json` references
    pub fn clean(&self) -> Result<(), Failure>;                                   // retire snp's layout (keeps recovery data, .lock, unknown files: D-DATA-04)
    pub fn patches_dir(&self) -> PathBuf; pub fn spill_dir(&self) -> PathBuf; pub fn scratch_dir(&self) -> PathBuf;
    pub fn crash_log(&self, brand: Brand) -> PathBuf;                             // <store>/pre-commit.log | snp.log
    pub fn tool_dir(&self, tool: &str, version: &str) -> PathBuf; pub fn cache_dir(&self, name: &str) -> PathBuf;
    pub fn relocated_from(&self) -> Option<PathBuf>;                              // store-v1.json path differs → old path
}
#[derive(Debug, Default)] pub struct SweepReport { pub removed_dirs: usize, pub removed_patches: usize, pub legacy_v1_removed: bool }
```
(gc *marking* — pre-commit's `_mark_used_repos` rules — lives in `snp_cmd::gc`, which owns the config dependency.)

#### 1.4.17 `snp-files` (W2, WP-22)

```rust
use std::{ffi::OsStr, path::Path, sync::Arc};
use snp_core::failure::{Failure, PyExc};
use file_identify::TagSet;
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)] pub struct FileId(pub u32);
/// Returned by value (Copy): the error payload stays in the table's head column.
#[derive(Debug, Clone, Copy)] pub enum TagOutcome<'a> { Tags(TagSet), Vanished, Unreadable(&'a PyExc) }
/// What a filter needs beyond the name tier (§2.3): stat (type/mode tags), encoding (text/binary), shebang tags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)] pub struct Needs { pub stat: bool, pub encoding: bool, pub shebang: bool }
/// One byte arena + spans; tier columns allocated on first need (§2.3, ADR-079).
#[derive(Debug)] pub struct FileTable { /* arena, spans, git_modes, name/stat/head/view columns */ }
impl FileTable {
    /// From git -z output (git order kept); Windows: '\' → '/' in the matching view (02 §10).
    pub fn from_zbuf(buf: Vec<u8>, spans: Vec<(u32, u32)>, root: &Path) -> FileTable;
    pub fn from_args(paths: Vec<std::ffi::OsString>, root: &Path) -> FileTable;  // --files / commit-msg / try-repo / stdin
    /// git index modes (Unix, core.fileMode=true, core.symlinks=true, after the stash: worktree == index) (§2.3);
    /// matched by path against the table; the caller iterates `snp_git::IndexEntries` without copying paths
    /// (snp-files does not depend on snp-git).
    pub fn set_trusted_modes<'a>(&mut self, entries: impl IntoIterator<Item = (&'a [u8], u32)>);
    pub fn retain_lexists(&mut self);                                           // once, before any hook (02 §10)
    pub fn len(&self) -> usize; pub fn is_empty(&self) -> bool;
    pub fn bytes(&self, id: FileId) -> &[u8];  pub fn os(&self, id: FileId) -> std::borrow::Cow<'_, OsStr>;
    pub fn match_view(&self, id: FileId) -> &str;                               // lossy UTF-8 (D-CRASH-12)
    /// The path relative to `prefix` when it lies under it (boundary-aware: `a/b` is under `a`, `ab/c` is not); the
    /// `root` extension and the v1.1 workspace mode use this (§1.6).
    pub fn relative_to(&self, id: FileId, prefix: &snp_core::path::RepoPath) -> Option<std::borrow::Cow<'_, [u8]>>;
    /// Lazy, memoised, thread-safe tiered identify. Each tier has its own cell, so a later call needing a higher tier
    /// never reads a partial result: the outcome combines exactly the tiers `n` requires.
    pub fn tags(&self, id: FileId, n: Needs) -> TagOutcome<'_>;
    /// Fills the required tiers on the pool (runs inline when the caller holds a permit, §1.4.8).
    pub fn prefetch(&self, ids: &[FileId], n: Needs, pool: &snp_proc::pool::JobPool);
}
#[derive(Debug, Clone)] pub struct HookFilter { pub files: Arc<snp_pyre::PyRegex>, pub exclude: Arc<snp_pyre::PyRegex>,
    pub types: TagSet, pub types_or: TagSet, pub exclude_types: TagSet, pub needs: Needs,
    pub prefix: Option<snp_core::path::RepoPath> }
impl HookFilter {
    /// Five filter fields as plain arguments (snp-files does not depend on snp-config) plus the boundary-aware path
    /// `prefix` (the `root` extension in v1; the workspace project prefix in v1.1): files outside it never match.
    pub fn compile(files: &str, exclude: &str, types: TagSet, types_or: TagSet, exclude_types: TagSet,
                   prefix: Option<snp_core::path::RepoPath>, cache: &snp_pyre::RegexCache) -> Result<Self, snp_pyre::PyReError>;
}
/// Classifier.from_config: top-level include/exclude (re.search on the '/'-path), then lexists.
#[derive(Debug)] pub struct Classifier { pub table: Arc<FileTable>, pub kept: Vec<FileId> }
impl Classifier {
    pub fn new(table: Arc<FileTable>, include: &snp_pyre::PyRegex, exclude: &snp_pyre::PyRegex,
               pool: &snp_proc::pool::JobPool) -> Result<Self, Failure>;   // nested pool use runs inline (§1.4.8)
    /// filenames_for_hook: order & duplicates kept; Vanished → dropped (X13-CLS-01); Unreadable → Err (X13-CLS-02, exit 3);
    /// a regex backtrack-limit hit → FatalError naming pattern and path (§3.3).
    pub fn for_hook(&self, f: &HookFilter, pool: &snp_proc::pool::JobPool) -> Result<Vec<FileId>, Failure>;
    pub fn by_types(&self, ids: &[FileId], f: &HookFilter) -> Result<Vec<FileId>, Failure>;   // check-useless-excludes
}
```

#### 1.4.18 `snp-stash` (W2, WP-23) — see §4 (stash) and §3.7 (DiffOracle)

```rust
use std::{ffi::OsString, path::{Path, PathBuf}, sync::Arc};
use snp_core::{cancel::CancelToken, clock::Clock, failure::Failure, output::Printer, path::RepoPath};
use snp_git::{Git, RepoInfo, ZList};
/// Owns cheap handles (Git and Printer are Arc-backed) so the guard has no lifetime and can live inside RunState.
/// `git` must be built with `Git::with_backend(ProcBackend { cancel: CancelToken::new(), role: Role::Critical })`: a
/// token that is never cancelled, so restore commands are never cut short (ADR-065). `cancel` is the run token
/// (observed only by the lock wait). `verify` = SNP_STASH_VERIFY=1 (warn-only post-restore check, §4.2 step 10e).
#[derive(Debug)] pub struct StashCtx { pub git: Git, pub info: RepoInfo, pub patches_dir: PathBuf, pub printer: Arc<Printer>,
    pub cancel: CancelToken, pub clock: Arc<dyn Clock>, pub signals: Option<Arc<snp_proc::signals::SignalGuard>>,
    pub lock_timeout: std::time::Duration /* SNP_STASH_LOCK_TIMEOUT, default 60 s, ≤ 1 h */, pub verify: bool,
    pub old_git: bool /* caps.version < 2.19: refuse to stash (D-DATA-17) */ }
#[derive(Debug, Clone, PartialEq, Eq)] pub enum Stashed { Nothing, Patch(PathBuf) }
#[derive(Debug)] pub struct StashGuard { /* state machine; restores on drop (panic path only, via RunState::drop) */ }
pub fn enter(cx: StashCtx) -> Result<StashGuard, Failure>;
impl StashGuard {
    pub fn stashed(&self) -> &Stashed;
    pub fn diff_index_clean(&self) -> bool;                              // diff-index rc 0 ⇒ baseline digest known empty
    pub fn child_env_token(&self) -> Option<(&'static str, OsString)>;   // ("_SNP_STASH_LOCK", token) for nested runs
    /// `stage_fixed` (§8.3): apply --check; journal `staging-fixed {paths}` durably; `git --literal-pathspecs add -- <paths>`;
    /// Ok(false) = would not apply (nothing staged).
    pub fn stage_fixed(&mut self, paths: &[RepoPath]) -> Result<bool, Failure>;
    pub fn restore(self) -> Result<(), Failure>;
}
/// A journaled run that never restored (§4.6). Recovery hints use `git apply --whitespace=nowarn` (ADR-084; `--binary` patches carry
/// full index lines and their preimage blobs are in the object store via write-tree).
#[derive(Debug, Clone)] pub struct Unrestored { pub patch: PathBuf, pub tree: Option<String>, pub worktree: PathBuf,
    pub pid: u32, pub state: Box<str>, pub staged_fixed: Vec<RepoPath> }
impl Unrestored { pub fn hint(&self) -> String; }                       // the exact stderr notice (§4.6)
/// Liveness is the worktree lock, never a pid (ADR-067): call only while holding `<git_dir>/snp-stash.lock` (or after a
/// successful non-blocking probe of it). Reads the git-dir pointer `<git_dir>/snp-stash.journal` (works with any store)
/// and the journals in `patches_dir` for this worktree. Never auto-applied.
pub fn find_unrestored(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Vec<Unrestored>;
/// Non-blocking: try the worktree lock, scan, release (run/hook-impl start; skipped for nested runs and when held).
pub fn recovery_scan(git_dir: &Path, patches_dir: &Path, worktree_root: &Path) -> Vec<Unrestored>;
/// `snp doctor --stash` / SNP_STASH_VERIFY (warn-only, 06's verify): `git apply --check --reverse <patch>` against the
/// worktree after a restore (true = the patch's changes are present).
pub fn verify_restored(git: &Git, patch: &Path) -> Result<bool, Failure>;
pub mod diff_oracle {   // ADR-027
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum DiffMode { Stat, Git }
    #[derive(Debug)] pub struct DiffOracle { /* mode, last DiffFingerprint, snapshot: stats + diff-input sources + racy hashes */ }
    #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Change { Unchanged, Changed }
    impl DiffOracle {
        /// Stat mode only on Unix, `stat_oracle_safe` filesystems, ≤ 16_384 index entries, no `attr.tree` /
        /// `GIT_ATTR_SOURCE`, SNP_DIFF_MODE != "git", and `tracked` + `origins` supplied by the step-6 fan-out (no extra
        /// spawn). `None` for either ⇒ Git mode.
        pub fn new(git: &snp_git::Git, info: &snp_git::RepoInfo, env: &snp_core::env::Env,
                   baseline: Option<snp_git::DiffFingerprint>, tracked: Option<Arc<snp_git::IndexEntries>>,
                   origins: Option<Arc<[PathBuf]>>) -> Result<Self, Failure>;
        pub fn mode(&self) -> DiffMode;
        /// Before a hook that may modify files (stat snapshot, or nothing in Git mode).
        pub fn before_hook(&mut self) -> Result<(), Failure>;
        /// After it: Unchanged without spawning when provably identical, else one real `git diff` and digest compare.
        pub fn after_hook(&mut self) -> Result<Change, Failure>;
        /// Native fixer reported exact modified paths (09 §6.7 caveat rules evaluated by the caller).
        pub fn after_native(&mut self, modified_tracked: bool, needs_real_diff: bool) -> Result<Change, Failure>;
        pub fn non_empty(&mut self) -> Result<bool, Failure>;          // show-diff gate (last digest)
    }
}
```

#### 1.4.19 `snp-hooks` (W2; WP-26 framework/pygrep/fail/hazmat/builtin/P0, WP-27 P1/P2/caveats)

```rust
use std::{ffi::{OsStr, OsString}, path::Path, sync::Arc};
use snp_core::{cancel::CancelToken, env::Env, failure::{Failure, PyExc}, path::RepoPath};
pub use snp_hooks_data::{Epoch, NativeSource};
/// argparse's column rule for native-hook help/errors (COLUMNS via Python int(), else 80: a hook's stdout is a pipe or a
/// 0×0 pty); re-exported so snp-run needs no snp-argparse edge. Natives call `columns(env, None)`.
pub use snp_argparse::columns;
#[derive(Debug)] pub struct HookCx<'a> { pub root: &'a Path, pub env: &'a Env, pub git: &'a snp_git::Git, pub color: bool,
    pub epoch: Epoch, pub windows: bool, pub columns: usize, pub facts: &'a snp_git::pch_facts::PchFacts,
    /// Checked between files; on cancellation a native returns early and the runner reports `Interrupted`.
    pub cancel: &'a CancelToken }
#[derive(Debug, Clone, Default)] pub struct Outcome { pub code: i32, pub output: Vec<u8>, pub modified: Vec<RepoPath>,
    pub crashed: bool, pub reads_only: bool }
pub trait NativeHook: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn prog(&self) -> &'static str;                 // argparse prog = console-script name
    /// Static pre-check of `args` (unsupported regex etc.) → Err(reason) ⇒ caller DELEGATES (09 §6.4 G5).
    fn preflight(&self, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String>;
    /// One xargs partition. `argv` is exactly the console script's sys.argv[1:] — `args` followed by the partition's
    /// files in shuffled partition order — and is parsed by argparse emulation (`framework::parse_args`), so a file named
    /// `-x.py` or `-i` behaves exactly as under Python (unrecognized argument / flag; ADR-072). Effects committed in
    /// input order; the first crash stops the partition (09 §2.6).
    fn run(&self, cx: &HookCx<'_>, argv: &[&OsStr]) -> Outcome;
    fn reports_modifications_exactly(&self) -> bool { true }
}
/// Shared machinery every native uses (WP-26 milestone M1, frozen here so WP-27 codes against it; ADR-071).
pub mod framework {
    use super::*;
    /// argparse emulation of a console script: prog = console-script name, CPython 3.13 texts, `allow_abbrev`.
    /// Err = the complete outcome of `-h` (code 0, help on stdout) or of a usage error (code 2, usage + error text).
    pub fn parse_args(spec: &'static snp_argparse::ParserSpec, prog: &str, argv: &[&OsStr], columns: usize, env: &Env)
        -> Result<snp_argparse::Namespace, Outcome>;
    /// The console script's stdout/stderr when both go to one pipe: `print()` is block-buffered text (8192 bytes),
    /// `sys.stdout.buffer.write` and stderr are written through; the merged bytes follow Python's flush order. Text
    /// writes use CRLF on Windows where upstream writes in text mode (09 §2.7).
    #[derive(Debug, Default)] pub struct PyOut { /* … */ }
    impl PyOut { pub fn print(&mut self, s: &str); pub fn print_bytes_text(&mut self, b: &[u8]); pub fn write_bytes(&mut self, b: &[u8]);
                 pub fn eprint(&mut self, s: &str); pub fn finish(self) -> Vec<u8>; }
    /// What one file step did.
    #[derive(Debug)] pub enum FileStep { Unchanged, Changed(RepoPath), Failed /* rc 1, keep going */, Crash(PyExc) }
    /// Iterates `files` in order; commits effects in order; the first `Crash` writes `crash_line` and stops the
    /// partition (exit 1); `cx.cancel` is checked between files.
    pub fn for_each_file(cx: &HookCx<'_>, files: &[&OsStr], out: &mut PyOut,
                         f: &mut dyn FnMut(&OsStr, &mut PyOut) -> FileStep) -> Outcome;
    /// Python's final traceback line `Type: text\n` (with the module-qualified name where Python prints one).
    pub fn crash_line(exc: &PyExc) -> Vec<u8>;
}
pub fn native(source: NativeSource, id: &str) -> Option<&'static dyn NativeHook>;  // concatenates the registries below
pub mod pch { pub mod p0 { pub static HOOKS: &[&dyn super::super::NativeHook]; }   // owned by WP-26
              pub mod p1 { pub static HOOKS: &[&dyn super::super::NativeHook]; }   // WP-27
              pub mod p2 { pub static HOOKS: &[&dyn super::super::NativeHook]; }   // WP-27
              pub mod caveats { pub static HOOKS: &[&dyn super::super::NativeHook]; } // WP-27
              pub mod stubs { pub static HOOKS: &[&dyn super::super::NativeHook]; } }  // WP-26 (removed hooks)
pub mod pygrep {   // 04a §8.4/§12; compiled once per hook per run (01 Addendum §A.4.5)
    /// Parsed `args + [entry]` and the compiled PyRegex for that parse (manual `Debug`: shows the source and flags).
    pub struct Compiled { /* parsed args + PyRegex */ }
    /// Err((code, output)) = argparse error text (exit 2) or `re.error: …` line (exit 1) — reported once for the hook.
    pub fn prepare(args: &[Arc<str>], entry: &str, columns: usize) -> Result<Compiled, (i32, Vec<u8>)>;
    /// `argv` = `args + [entry] + files` of the partition, re-parsed exactly like `pygrep.py` (a file named like a flag
    /// changes the parse, ADR-072); the pattern is recompiled through `cache` only when the parse differs from `c`.
    pub fn run_partition(c: &Compiled, argv: &[&OsStr], root: &Path, cache: &snp_pyre::RegexCache,
                         cancel: &CancelToken) -> super::Outcome;
}
pub mod fail { pub fn output(entry: &str, files: &[&[u8]]) -> Vec<u8>; }   // f'{entry}\n\n' + '\n'.join(files) + '\n'
pub mod hazmat {   // ADR-006
    /// `snp __hazmat …` (path H, a subprocess per partition): prog `hazmat.py`, CPython-3.13 texts, exit 0/1/2/child.
    pub fn in_hook_main(args: &[OsString], env: &Env) -> i32;
    /// Public `{prog} hazmat …` (path C): ExecutableNotFoundError → Failure (exit 3).
    pub fn cli(tool: &snp_cli::HazmatTool, env: &Env, cancel: &snp_core::cancel::CancelToken) -> Result<i32, Failure>;
    pub static HAZMAT_PY_SPEC: &snp_argparse::ParserSpec;
}
// `repo: builtin`: validation is in snp-config's schema; the manifest is `snp_hooks_data::BUILTIN_MANIFEST_YAML`; the
// builtin variants (`--check` flags, upstream bugs fixed) are natives registered under `NativeSource::Builtin`.
```

#### 1.4.20 `snp-repo` (W3, WP-31)

```rust
use std::{path::PathBuf, sync::Arc};
use snp_core::{env::Env, failure::Failure, lang::LanguageId, output::{Printer, WarnSink}, cancel::CancelToken};
use snp_config::{Config, HookDef, ext::FastPath};
use snp_hooks_data::{Epoch, NativeSource};
/// G5 of §7.2 as a seam (snp-repo cannot depend on snp-hooks): snp-run implements it with
/// `snp_hooks::native(source, id).map(|h| h.preflight(epoch, args))`; `all_hooks` calls it after G1–G4/G6 so the final
/// Native/Delegate decision — including the clone of a Tier A repo whose hook delegates — is made inside `all_hooks`.
pub trait Preflight: Send + Sync + std::fmt::Debug {
    fn check(&self, source: NativeSource, id: &str, epoch: Epoch, args: &[Arc<str>]) -> Result<(), String>;
}
/// `snp_lang::ToolDirs` over the store (tools-v1 dirs and locks, cache-v1 dirs); owned by WP-31.
#[derive(Debug, Clone, Copy)] pub struct StoreTools<'a>(pub &'a snp_store::Store);
impl snp_lang::ToolDirs for StoreTools<'_> {}
#[derive(Debug)] pub struct ResolveCtx<'a> { pub cfg: &'a Config, pub store: &'a snp_store::Store, pub git_exe: &'a Arc<snp_git::GitExe>,
    pub git_backend: Arc<dyn snp_git::GitBackend>, pub env: &'a Env, pub printer: &'a Printer,
    pub pool: &'a snp_proc::pool::JobPool, pub net: &'a snp_proc::pool::Limiter, pub installs: &'a snp_proc::pool::Limiter,
    pub cancel: &'a CancelToken, pub host: snp_lang::Host<'a>, pub fast_path: FastPathPolicy,
    pub preflight: &'a dyn Preflight, pub overlay: Option<&'a snp_config::overlay::Overlay>,
    pub regexes: &'a snp_pyre::RegexCache, pub warn: &'a dyn WarnSink, pub brand: snp_core::brand::Brand }
#[derive(Clone, Debug)] pub struct FastPathPolicy { pub mode: FastPath, pub exclude: Vec<Arc<str>>, pub coalesce: bool,
                                                    pub with_fallback: bool }
#[derive(Clone, Debug)] pub enum HookSrc { Remote { repo: Arc<str>, rev: Arc<str> }, Local, Meta, Builtin }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum MetaHook { CheckHooksApply, CheckUselessExcludes, Identity }
/// Data only (snp-repo does not depend on snp-hooks): snp-run maps (source, id) to the impl via `snp_hooks::native`.
#[derive(Clone, Debug)] pub enum ExecKind { Language, Native { source: NativeSource, id: Arc<str>, epoch: Epoch }, Meta(MetaHook) }
#[derive(Clone, PartialEq, Eq, Hash, Debug)] pub struct InstallKey { pub repo: snp_store::RepoKey, pub lang: LanguageId,
                                                                     pub version: Arc<str>, pub deps: Arc<[Arc<str>]> }
#[derive(Clone, Debug)] pub struct ResolvedHook { pub index: u32, pub src: HookSrc, pub def: Arc<HookDef>, pub prefix: PathBuf,
    pub repo: Option<snp_store::RepoHandle>, pub language_version: Arc<str>, pub exec: ExecKind,
    pub install_key: Option<InstallKey>, pub virtual_install: Option<InstallKey> /* Tier-A message parity, ADR-032 */,
    pub docker_tag: Option<Arc<str>> }
/// The seam snp-run uses (S12); `StoreResolver` is the real implementation.
pub trait Resolve: Send + Sync + std::fmt::Debug {
    /// all_hooks(): every repo in config order; background prefetch of eligible clones (NetLimiter, ADR-064) and
    /// `[INFO] Initializing environment for …` printed in config order at pre-commit's moment (marker-driven, with the
    /// foreground clone exactly as pre-commit when the prefetch is absent or failed); manifest load (+ deprecated-stage
    /// warning on a new clone); unknown-id error; merge (+ overlay hook patch); language default resolution; env-less
    /// guards; fast-path decisions G1–G6 incl. G5 through `cx.preflight` and the clone of delegated Tier A repos.
    fn all_hooks(&self, cx: &ResolveCtx<'_>) -> Result<Vec<ResolvedHook>, Failure>;
    /// install_hook_envs(): dedup by InstallKey, check_support before FS mutation, per-repo locks, parallel across distinct
    /// prefixes (InstallLimiter), `[INFO] Installing environment for …` triplets in config order (incl. virtual ones).
    fn install_envs(&self, hooks: &[&ResolvedHook], cx: &ResolveCtx<'_>) -> Result<(), Failure>;
    fn hook_env(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>) -> Result<Env, Failure>;   // cached per InstallKey
    fn plan(&self, h: &ResolvedHook, cx: &ResolveCtx<'_>, color: bool) -> Result<snp_lang::RunPlan, Failure>;
}
#[derive(Debug, Default)] pub struct StoreResolver { /* env cache, decision log */ }
impl Resolve for StoreResolver {}
pub fn decision_table(cfg: &Config, cx: &ResolveCtx<'_>) -> Vec<(Arc<str>, Arc<str>, String)>; // wrapped by snp_run::doctor_fast_path
```

#### 1.4.21 `snp-run` (W3, WP-32)

```rust
use std::{ffi::OsString, io, path::PathBuf, sync::Arc};
use snp_core::{brand::Brand, cancel::CancelToken, clock::Clock, env::Env, failure::Failure, output::Printer};
/// Constructed once by the binary (§3.1 step 2). `git_backend` is the run's Tool-role backend; the stash builds its own
/// Critical one (§1.4.18). `net`/`installs` are the Limiters of §1.4.8; `probe`/`runner` feed `snp_lang::Host`; the
/// binary builds them as `ToolProbe::new(persist_dir)` and `ProcRunner { cancel }` (snp → snp-lang edge, ADR-081).
#[derive(Debug)] pub struct RunCtx { pub env: Env /* startup */, pub brand: Brand, pub printer: Arc<Printer>,
    pub store: snp_store::Store, pub git_exe: Arc<snp_git::GitExe>, pub git_backend: Arc<dyn snp_git::GitBackend>,
    pub resolver: Arc<dyn snp_repo::Resolve>, pub cancel: CancelToken, pub signals: Option<Arc<snp_proc::signals::SignalGuard>>,
    pub pool: Arc<snp_proc::pool::JobPool>, pub net: Arc<snp_proc::pool::Limiter>, pub installs: Arc<snp_proc::pool::Limiter>,
    pub probe: Arc<snp_lang::ToolProbe>, pub runner: Arc<dyn snp_lang::CmdRunner>,
    pub budget: Arc<snp_proc::spawn::OutputBudget>, pub invocation_cwd: PathBuf, pub arg0: Option<std::ffi::OsString>,
    pub self_exe: PathBuf, pub clock: Arc<dyn Clock>, pub fetch: Option<Arc<dyn snp_net::Fetch>>, pub regexes: Arc<snp_pyre::RegexCache> }
pub fn run(a: snp_cli::RunArgs, cx: &RunCtx) -> Result<i32, Failure>;              // single exit (§4.4)
pub fn hook_impl(a: snp_cli::HookImplArgs, stdin: &mut dyn io::Read, cx: &RunCtx) -> Result<i32, Failure>;
/// pre-commit's throwaway `Store(tempdir)` (03 §12): runs against `Store::ephemeral(tmp, Some(&cx.store))`, so the
/// `Initializing environment` line and the `Installing environment` triplet print on every try-repo; the temp config is
/// never tracked (the user's `-c` is tracked in the main store as for every git command); ADR-059.
pub fn try_repo(a: snp_cli::TryRepoArgs, cx: &RunCtx) -> Result<i32, Failure>;
pub fn install_hooks(a: snp_cli::InstallHooksArgs, cx: &RunCtx) -> Result<i32, Failure>;
pub fn list(a: snp_cli::ListArgs, cx: &RunCtx) -> Result<i32, Failure>;
/// `snp doctor --fast-path`: loads the config and returns `snp_repo::decision_table` rows as (ok, text); the binary wraps
/// it in a `snp_cmd::DoctorCheck` (no snp → snp-config edge needed; ADR-070).
pub fn doctor_fast_path(config: &std::ffi::OsStr, cx: &RunCtx) -> Result<Vec<(bool, String)>, Failure>;
pub mod render {   // pure: events → bytes, snapshot-tested without processes (10 §9.1)
    pub fn cols(names: &[&str]) -> usize;                       // max(80, max len_cjk + 30)
    pub fn start_msg(name: &str, cols: usize) -> String;
    pub fn end_msg(status: Status, color: bool) -> String;
    #[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Status { Passed, Failed, Skipped, NoFiles }
    #[derive(Debug)] pub struct Details<'a> { pub id: &'a str, pub duration: Option<f64>, pub exit_code: Option<i64>, pub modified: bool,
                             pub ext_lines: &'a [String], pub output: &'a [u8], pub color: bool }
    pub fn details(d: &Details<'_>) -> Vec<u8>;
}
```

#### 1.4.22 `snp-cmd` (W3, WP-33)

```rust
use std::{ffi::OsStr, io, path::Path, sync::Arc};
use snp_core::{brand::Brand, cancel::CancelToken, env::Env, failure::Failure, output::Printer, stage::HookType};
#[derive(Debug)] pub struct CmdCtx<'a> { pub env: &'a Env, pub cwd: &'a Path, pub brand: Brand, pub printer: &'a Printer,
    pub store: &'a snp_store::Store, pub git_exe: Option<&'a Arc<snp_git::GitExe>>, pub git_backend: Arc<dyn snp_git::GitBackend>,
    pub cancel: &'a CancelToken, pub pool: &'a snp_proc::pool::JobPool, pub self_exe: &'a Path,
    pub arg0: Option<&'a std::ffi::OsStr> }
/// Writes the scripts only. `install --install-hooks` is sequenced by the binary: `snp_cmd::install(..)?` then
/// `snp_run::install_hooks(InstallHooksArgs { color, config, with_fallback: false }, &run_cx)` — pre-commit's order;
/// the first failure is returned (ADR-070).
pub fn install(a: &snp_cli::InstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn uninstall(a: &snp_cli::UninstallArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn init_templatedir(a: &snp_cli::InitTemplatedirArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn autoupdate(a: &snp_cli::AutoupdateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn gc(cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn clean(cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn validate_config(a: &snp_cli::ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn validate_manifest(a: &snp_cli::ValidateArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn migrate_config(a: &snp_cli::ConfigArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn sample_config(cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn config(a: &snp_cli::ConfigCmd, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub fn cache(a: &snp_cli::CacheArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
/// Checks that need crates snp-cmd cannot depend on are injected by the binary: the fast-path table (wrapping
/// `snp_run::doctor_fast_path`). The stash-recovery check and `--stash` verification live in snp-cmd (snp-stash edge).
pub trait DoctorCheck: std::fmt::Debug { fn name(&self) -> &'static str; fn run(&self, cx: &CmdCtx<'_>) -> Vec<DoctorLine>; }
#[derive(Debug, Clone)] pub struct DoctorLine { pub ok: bool, pub text: String, pub fix: Option<String> }
pub fn doctor(a: &snp_cli::DoctorArgs, cx: &CmdCtx<'_>, extra: &[&dyn DoctorCheck]) -> Result<i32, Failure>;
pub fn self_cmd(a: &snp_cli::SelfArgs, cx: &CmdCtx<'_>) -> Result<i32, Failure>;
pub mod scripts {   // 11 §A.5–A.6, §A.19
    pub use snp_git::hookscripts::{Owner, classify};
    /// 11 §A.19.3: the stable executable to embed as SNP_EXE, never canonicalised. Candidates in order: `absolute(arg0)`
    /// when arg0 contains a separator; then each absolute PATH entry joined with the brand's name — `snp[.exe]` (snp
    /// brand) or `pre-commit[.exe]` (compat brand, so hooks keep the compat brand, ADR-060); then `current`. The first
    /// candidate that is a file and `same_file` as `current` wins (Unix (dev, ino); Windows volume serial + file index).
    pub fn select_exe(brand: Brand, arg0: Option<&OsStr>, env: &Env, current: &Path) -> std::path::PathBuf;
    /// 11 §A.19.4: same file, or a copy with equal length and BLAKE3, or `--version` matching `pre-commit X (snp Y)`.
    pub fn is_snp(file: &Path, current: &Path) -> bool;
    pub fn render(cfg: &OsStr, t: HookType, skip_missing: bool, snp_exe: &Path) -> Vec<u8>;   // §9.7 template, LF
    /// M1/M1b/M2/M3/M12 of 11 §A.5.4 — written to **stderr** in both brands (D-DIAG-02).
    pub fn migration_notes(brand: Brand) -> &'static [&'static str];
}
```

#### 1.4.23 `snp-migrate` (W3, WP-34)

```rust
#[derive(Debug, Clone)] pub struct LefthookOpts { pub root: std::path::PathBuf, pub config: Option<std::path::PathBuf>, pub output: Option<std::path::PathBuf>,
    pub portable: bool, pub local: bool, pub strict: bool }
#[derive(Debug)] pub struct Converted { pub config_yaml: String, pub overlay_yaml: Option<String>, pub report: Vec<String>, pub lossy: bool }
/// 05 §19 (pure: no git, no writes). `snp migrate lefthook --install` is sequenced by the binary: write the outputs
/// atomically → remove lefthook shims (restore `<hook>.old`), delete `.git/info/lefthook*` → `git lfs update --force` when
/// LFS is detected → `snp_cmd::install` for every emitted stage (which chains the git-lfs hooks as `.legacy`).
pub fn lefthook(o: &LefthookOpts) -> Result<Converted, snp_core::failure::Failure>;
pub fn to_builtin(config_bytes: &[u8]) -> Result<Vec<u8>, snp_core::failure::Failure>;      // 09 §7.1
pub fn to_upstream(config_bytes: &[u8]) -> Result<Vec<u8>, snp_core::failure::Failure>;
```

#### 1.4.24 `snp` (binary; W0 skeleton, W3 WP-33)

```rust
// crates/snp/src/main.rs — the only `main`; the only place that may call std::process::exit. `ExitCode` cannot carry a
// code > 255 on stable Rust (Windows `ExitCodeExt::from_raw` is unstable in 1.99.0), so `entry` returns `Exit`.
fn main() -> std::process::ExitCode {
    match snp::entry() {                         // every guard and destructor of `entry` has already run
        snp::Exit::Code(c) => std::process::ExitCode::from(c),
        // clippy's `exit` lint never fires inside `main`; the clippy.toml `disallowed-methods` entry does (ADR-081).
        #[expect(clippy::disallowed_methods, reason = "Windows hook-impl `legacy | run` > 255 (14 A.7.4)")]
        snp::Exit::Raw(r) => std::process::exit(r),
    }
}
// crates/snp/src/lib.rs
#[derive(Copy, Clone, Debug, PartialEq, Eq)] pub enum Exit { Code(u8), Raw(i32) }   // exhaustive
pub fn entry() -> Exit;   // §10.1
```

#### 1.4.25 `snp-difftest` (W3, WP-35, tooling)

```rust
#[derive(Debug, Clone)] pub struct Scenario { /* 10 §7.4.3 schema: requires, repos, tree, config, steps, compare, brands, expect_divergence */ }
pub fn load_scenarios(dir: &std::path::Path) -> Result<Vec<Scenario>, String>;
/// `oracle_snp_prog` = the pc462 oracle launched with argparse prog patched to `snp` (ADR-058), used for snp-brand runs.
#[derive(Debug)] pub struct Runner { pub snp_bin: std::path::PathBuf, pub oracle_bin: std::path::PathBuf,
    pub oracle_snp_prog: std::path::PathBuf, pub root: std::path::PathBuf }
#[derive(Debug)] pub enum Verdict { Same, Diverged { id: Option<String>, diff: String }, StaleDivergence(String) }
impl Runner { pub fn run(&self, s: &Scenario) -> Verdict; }
pub fn normalise(tier: Tier, bytes: &[u8], map: &BrandMap) -> Vec<u8>;
#[derive(Copy, Clone, Debug)] pub enum Tier { A, B, C }
#[derive(Debug)] pub struct BrandMap { /* tests/differential/brand_map.yaml; non-argparse texts only */ }
```

### 1.5 User-crate bindings (the real APIs on the `snp-support` branches)

cfgv 0.3.0 (`/home/dromix/projects/cfgv-rs`, branch `snp-support`, re-verified at `6a04a30`; the commit CI and
releases build against is the one recorded in `deps.lock` (ADR-061); `rust-version = 1.85`, `default-features = false`):
* `cfgv::{Value, Map (= IndexMap<Value, Value>), Set (= IndexSet<Value>), BigInt, Date, DateTime, Schema, Validator,
  MapItem, Cx, CheckFn, RegexCheck, ValidationError, validate_with, apply_defaults_owned, load_from_filename_with}`.
  `Value` already has `Null, Bool, Int, BigInt, Float, String, Bytes, List, Tuple, Map, Set, Date, DateTime` — no new
  variant is requested (the compatibility proposal's `Value::Opaque` is unnecessary).
* pre-commit's custom validators (`StagesMigration[NoDefault]`, `LanguageMigration[Required]`, `WarnMutableRev`,
  `OptionalSensibleRegexAt{Hook,Top}`, `NotAllowed('entry')`, `Deprecated[Default]StagesWarning`, `check_type_tag`,
  `check_min_version`) are `impl cfgv::MapItem` in snp-config; warnings flow through `Cx::new(|msg| warn.warn(&msg))`
  (`Cx` is opaque: `Cx::new(callback)`, `Cx::collect(&mut Vec<String>)`, `Cx::discard()`, `Cx::warn`).
* Loading: `load_from_filename_with(filename: &Path, schema, load_strategy, display_filename: Option<&str>, cx)` — the
  display name is a `&str`, so snp-config passes `display.to_string_lossy()` (an `OsStr` from `-c` may be non-UTF-8; the
  lossy form is what Python prints for an fsdecoded name after `str()` in this message).
* Regex validation: `CheckFn::check_regex_with(snp_pyre::cfgv_is_valid)` — `check_regex_with` still takes a **bool**
  `Fn(&str) -> bool` and wraps it into the general `RegexCheck = Arc<dyn Fn(RegexPattern<'_>) -> Result<bool,
  ValidationError> + Send + Sync>` (bytes patterns arrive Latin-1-decoded; they never reach it, because pre-commit's
  `check_string_regex` = `check_and(check_string, check_regex)` rejects non-strings first). So every rejection,
  including OverflowError/ValueError/RecursionError (D-CRASH-04), renders the single leaf `'<repr>' is not a valid
  python regex`. `check_regex_try_with` + `ValidationError::uncaught` (which could reproduce pre-commit's exit-3
  crash) is deliberately unused.
  FutureWarnings are not routed through cfgv: snp-config calls `snp_pyre::validate` on every `files`/`exclude` after
  validation and emits one stderr line per distinct (pattern, warning) through `DiagSink` (01 Addendum §A.4).
* Extension keys are additional `MapItem`s appended after pre-commit's items and added to the known-key lists of the
  `WarnAdditionalKeys` items, so error precedence and every warning for pre-commit keys is unchanged.

file-identify 0.5.0 (`/home/dromix/projects/file-identify`, branch `snp-support`, re-verified at `1324f40`; pinned by
`deps.lock`; `rust-version = 1.85`, no default features):
* `Tag`, `TagSet` (`Copy` bitset; `union`, `intersection`, `is_subset`, `is_disjoint`, `intersects`, `from_names`),
  `ALL_TAGS`, `IDENTIFY_VERSION` ("2.6.20"), `tags_from_filename(impl AsRef<[u8]>) -> TagSet`,
  `tags_from_interpreter(&str)`, `Meta::{from_metadata(&Metadata, ExecPolicy), from_git_mode_with(u32, ExecPolicy) -> Option<Meta>}`,
  `ExecPolicy::{PythonCompat (default), ModeBits}`, `FileKind`, `classify(Meta, path_bytes) -> Classification::{Done(TagSet),
  NeedsContent(Pending)}`, `Pending::{tags, needs_shebang, needs_encoding, finish(reader), finish_bytes}`,
  `tags_from_metadata(path, &Metadata, ExecPolicy)`, `parse_shebang_from_file_with(path, ExecPolicy) -> Result<Vec<String>>`
  (a non-UTF-8 shebang yields an empty vector, identify parity), `MAX_SHEBANG_BYTES` (64 KiB), `TEXT_PROBE_LEN`.
* Windows: `FileKind::from_file_type` reports a directory junction as `Symlink` (no `unsafe` to read the reparse tag);
  identify reports `directory` — registered divergence D-PLAT-12.
* WP-00 adds `crates/snp-config/tests/user_crates.rs` and `crates/snp-files/tests/user_crates.rs`, which name every
  cfgv / file-identify item listed in this section with explicit types, so a breaking change on either branch fails
  CI at the drift check of ADR-061 instead of inside a later package.

### 1.6 Workspace (multi-project) mode — seams in v1, implementation in v1.1

Deferred to **v1.1** (explicit opt-in only: `snp run --workspace` or root key `workspace: true`; never implicit
discovery, unlike prek, 06 §0 #3). Seams built in v1 (ADR-042):
* `ConfigSource` carries `project_root` (= repo root in v1); `load_config` never assumes the git root.
* `snp-run` plans a `Vec<Project>` (length 1 in v1); the scheduler, printer and summary iterate projects; project
  headers are a renderer feature behind a flag (snp brand only).
* `snp-files`: `HookFilter::compile(…, prefix: Option<RepoPath>, …)` and `FileTable::relative_to` (§1.4.17) — used
  in v1 by the `root` extension; the v1.1 project prefix is the same parameter.
* `snp-repo::ResolveCtx` is per project; the store is project-agnostic (content-addressed keys).
* The stash stays per *worktree*, never per project (one guard for all projects).
* Hook selectors (ADR-069): a positional `hook` argument is always resolved first as a literal id or alias — pre-commit
  accepts any string, so ids such as `lint:js` or `mypy:strict` keep working in both brands. Only in workspace mode
  (v1.1) and only when no literal id/alias matches is `dir:hook` read as a project selector; in v1 an unmatched
  selector gives pre-commit's `No hook with id …` error exactly as today. Tested with colon-containing ids.

---
## 2. Core data model

### 2.1 Config, manifest and the resolved hook

Three layers, each immutable once built, shared by `Arc`:

1. **`cfgv::Value`** — the validated and defaulted YAML value (cfgv 0.3 + `snp-yaml`). Kept only as `Config::raw`
   for meta hooks and `snp config dump`. Never re-parsed per hook.
2. **Typed config** (`snp-config`): `Config` → `RepoConfig` → `ConfigHook`/`HookDef` (§1.4.15). Construction follows
   the cfgv item order (01 §3), so the *first* error and all *earlier* warnings are exactly pre-commit's.
3. **`ResolvedHook`** (`snp-repo`): one per hook occurrence in config order (duplicates kept), carrying the merged
   `HookDef`, the store prefix, the resolved `language_version` (after `default_language_version` and the language
   default probe), the `ExecKind` (language / native fast path / meta) and the `InstallKey` (= pre-commit's
   `install_key` with `prefix` replaced by `RepoKey`).

Merge semantics (`snp_config::merge_hook`) = `repository._hook()` (01 §6.3, 03 §8.2): `dict(manifest).update(config)`
per key (lists replaced), `language_version: default` → `default_language_version[lang]`, empty `stages` →
`default_stages`, env-less guards (`language_version`/`additional_dependencies` on env-less languages →
`[ERROR] …` exit 1), then unknown keys (W11) after merging, sorted. Extension keys (§8) are parsed into `HookExt`;
in `ExtMode::Strict` they stay unknown keys and produce pre-commit's warnings.

Memory shape (100k-file, 50-hook repo target ≤ 30 MB RSS): strings `Arc<str>` (shared repo URLs; the defaults
`''`/`'^$'` interned through `RegexCache`), stage sets `u16`, tag sets the `file-identify` `Copy` bitset, regexes
compiled once per distinct `(pattern, Options)` and lazily (engines built on first match).

### 2.2 Stage / HookType / Language enums

`HookType` (10, source order), `Stage = Hook(HookType) | Manual` (11), `StageSet(u16)`, `LanguageId` (21, sorted).
Legacy stage aliases are accepted only where pre-commit accepts them (`stages`, `default_stages`, `--hook-stage`),
never for `default_install_hook_types`/`-t` (01 §1.3). `system`/`script` are translated before the one-of check so
every message names `unsupported`/`unsupported_script` (01 §1.4). Meta hooks accept only `unsupported` (4.6.2
parity). `Stage::ALL` source order drives help texts and argparse `choices`; sorted order drives cfgv `one_of` messages.

### 2.3 File table (`snp-files`)

```
FileTable
 ├─ arena: Vec<u8>                one allocation: the git -z stdout (or the joined CLI/stdin paths)
 ├─ spans: Box<[(u32,u32)]>       git order (never sorted: diff.orderFile parity, 02 G.0 #8)
 ├─ git_modes: Option<Box<[u32]>> trusted index modes (git-mode fast path), 0 = unknown
 ├─ tier columns, struct-of-arrays, each allocated on first need (Column<T> = OnceLock<Box<[OnceLock<T>]>>):
 │    name: Column<TagSet>                         tags_from_filename (48 B bitset)
 │    stat: Column<StatTier>                       Ok((FileKind, executable)) | Vanished      (one lstat)
 │    head: Column<Result<TagSet, Box<PyExc>>>     text/binary + shebang tags | Unreadable      (≤ 1 open + read)
 │    view: Column<Box<str>>                       lossy UTF-8, '\'→'/' on Windows (only when not ASCII/'/'-clean)
 └─ hooks receive Vec<FileId(u32)>; never cloned path strings; shuffle permutes u32 ids
```
`tags(id, n)` combines exactly the tiers `n` requires from their own cells, so a hook needing the head tier never sees
a partial set cached by an earlier hook that needed only the name tier. A run touching only the name tier costs one
48-byte cell per file (≈ 5 MB for 100 k files); `Unreadable` boxes its `PyExc`.

* **Path representation.** Paths are the raw bytes git printed (`-z`, so `core.quotePath` is irrelevant), repo-root
  relative, `/`-separated. Unix: `OsStrExt::from_bytes` for argv. Windows: git emits UTF-8; `\` is normalised to `/`
  in the *matching view* only (pre-commit normalises before filtering, 02 §10); display paths keep `ntpath` rules.
* **Non-UTF-8 policy (D-CRASH-12, ADR-052).** pre-commit crashes (strict decode, exit 3). snp: regex matching uses the
  lossy UTF-8 view (U+FFFD), identify uses the raw bytes, argv receives the raw bytes, messages print bytes verbatim.
  Byte-exact for every repository pre-commit can handle.
* **Tiered lazy tags (ADR-041).** A hook's filter needs only the tag categories it references. `HookFilter::needs` is
  computed at compile time from `types ∪ types_or ∪ exclude_types`:
  1. **name tier** (zero I/O): `file_identify::tags_from_filename(bytes)`;
  2. **stat tier** (`stat`: any type/mode tag referenced — `file` by default — one `lstat`, fused with the `lexists`
     result): `file`/`directory`/`symlink`/`socket`, `executable`/`non-executable` via `Meta::from_metadata(md,
     ExecPolicy::PythonCompat)` (exact Python `os.access` emulation);
  3. **head tier** (`encoding`: `text`/`binary` referenced; `shebang`: any interpreter-derived tag referenced) — at most
     one `open` + `TEXT_PROBE_LEN` read (+ shebang line ≤ 64 KiB) through `Pending::finish`, `O_NONBLOCK` for
     non-regular files (X13-CLS-03).
  The filter outcome depends only on membership of referenced tags, so evaluating with a partial set whose pending
  categories are irrelevant is exact (S13; property test against full identification).
  A filter referencing only name-derived tags (`python`, `yaml`, …) still needs the stat tier: identify tags a
  symlink, directory or socket with its type tag alone (`link.py` is not `python`), so the name tier alone answers
  only filters that reference no tag at all.
* **git-mode fast path.** When the file list is the staged list *after* the stash (worktree == index), on Unix, with
  `core.fileMode=true` and `core.symlinks=true`, `Meta::from_git_mode_with(mode, PythonCompat)` gives kind/executable
  with zero syscalls (`ls-files -s -z`, or the staged-only `diff --staged --raw -z` when Stat mode is impossible, ran
  in the fan-out, §3.1).
* **Vanished / unreadable.** A file deleted (by an earlier hook) before its tags were needed → `TagOutcome::Vanished`
  → excluded from this and later hooks (X13-CLS-01, D-CRASH-08). A regular file whose content tier is needed but
  cannot be read → `Unreadable` → run aborts with pre-commit's exit-3 line (fail closed, X13-CLS-02). An unreadable file
  whose content no selected hook needs never crashes (pre-commit would; D-CRASH-18).

### 2.4 Run context

```rust
struct RunState<'a> {                       // lives on the main thread's stack for one `run`
    cx: &'a RunCtx,                         // startup Env, brand, printer, store, git exe, resolver, cancel, pool, clock
    run_env: Env,                           // startup env ⊕ PRE_COMMIT=1 ⊕ PRE_COMMIT_* (02 §6.1) ⊕ stash token
    git: Git, info: RepoInfo,               // user repo, discovered once (one rev-parse; 02 G.9 #6)
    store_guard: Option<StoreGuard>,        // shared lock while remote repos are in use (§5.5)
    cfg: Arc<Config>, hooks: Vec<ResolvedHook>, files: Option<Arc<FileTable>>,
    oracle: DiffOracle,                     // last observed `git diff` digest + snapshot (§3.7)
    cols: usize, results: Vec<HookResult>,  // printed strictly in config order
    stash: Option<StashGuard>,
}
impl Drop for RunState<'_> {                // runs BEFORE any field is dropped
    fn drop(&mut self) { if let Some(g) = self.stash.take() { let _ = catch_unwind(AssertUnwindSafe(|| g.restore())); } }
}
```

Drop order is part of the safety argument (ADR-066): a struct's own `Drop` runs before its fields are dropped, so the
panic-path restore happens first, before any other field's destructor could panic during unwinding (which would abort
the process). Worker threads were already joined by `thread::scope`. The normal and error paths never rely on `Drop`:
`run()` takes the guard out and calls `restore()` explicitly (§4.4). Tested with a field whose `Drop` panics.

### 2.5 Environment model: immutable `Env` + per-command patches, never `set_var`

```
startup Env (captured once in main, minus __PYVENV_LAUNCHER__/PYTHONEXECUTABLE)
   ├── run_env      = startup ⊕ {PRE_COMMIT=1, PRE_COMMIT_FROM_REF…} ⊕ {_SNP_STASH_LOCK while held}
   │                  (exported to hooks AND to user-repo git calls, as pre-commit mutates os.environ)
   │     └── hook_env = run_env.patched(language activation patch)          (cached per InstallKey, Arc-shared)
   │            └── child_env = hook_env.patched(hook `env:` extension)     (user env wins, prek semantics)
   ├── owned_git_env = no_git_env(startup) + GIT_CONFIG_COUNT{core.fsmonitor=, core.useBuiltinFSMonitor=false}
   ├── installer_env = no_git_env(hook_env) (+ uv/pip translation for python)   (D-SEC-05: no GIT_INDEX_FILE leaks)
   └── legacy_env   = startup ⊕ {PRE_COMMIT_RUNNING_LEGACY=1, SNP_RUNNING_LEGACY=1}
```

* `EnvPatch` follows `envcontext` exactly: operations apply in order and every `Var(name)` template reads the
  snapshot *before* the patch. Windows keys are ASCII-case-insensitive with first-seen spelling preserved (14 A.1 #17).
* **Windows name case (14 P-83 / D-WIN-05, parity per call site, ADR-074):** where pre-commit passes an `env=` dict copy
  (`no_git_env` for snp-owned repos, autoupdate and try-repo git; any 04a/04b site that builds `env=`), CPython's
  `os.environ` copy carries UPPER-cased names, so snp upper-cases those names; children that pre-commit spawns with the
  inherited environment (hooks, legacy hooks, installers run under `in_env`) keep the original spelling.
* Children always get `env_clear()` + the full computed map: the child environment is a pure function of inputs
  (S6), logged at `SNP_LOG=trace` through an **allow-list** (ADR-077): values are printed only for `PATH`, `PATHEXT`,
  `HOME`, `USERPROFILE`, `TERM`, `LANG`, `LC_*`, `TZ`, `PRE_COMMIT*`, `SNP_*`, `GIT_*` except `GIT_ASKPASS` and
  `GIT_CONFIG_VALUE_*`, and the activation variables snp sets itself; every other variable is logged as
  `NAME=<redacted, N bytes>`; URL userinfo in argv is scrubbed.
* `platform_max_length` uses the **startup** env size (pre-commit computes it at import, 02 §1 step 2, 04b §5.5).

---

## 3. Run pipeline end to end

### 3.1 Sequence (`snp_run::run`), with parity anchors

| # | Step | Parity anchor | Notes (safety / performance) |
|---|---|---|---|
| 1 | `snp::entry`: brand from `argv[0]`; `argv[1] == "__hazmat"` raw dispatch; `[]`→`["run"]`; argparse; `Usage`/`Help`/`Version` exit before anything touches the store | 12 §A.17 #1, #9 | no store, no git, no thread; `--version` ≤ 3 ms |
| 2 | Signals installed (Linux: child subreaper for run/hook-impl/try-repo); `CancelToken`; `Printer(color)`; `Store::new` (lazy); `JobPool` (`pool_size`), `Limiter`s (net 8, installs min(N,4)), `OutputBudget`, `ToolProbe`, `ProcRunner` — all carried by `RunCtx` | 02 §4 | signal thread only cancels the token and applies the §10.4 table |
| 3 | `GitExe::resolve` (cached version, 02 G.4), then `Git::discover` (one combined `rev-parse`; it needs `caps` for the 2.19–2.30 fallback) → FatalError texts unchanged | 02 §3, G.4 | never refuses here; < 2.31 warns once on stderr (D-DIAG-01); a stash on < 2.19 is refused at step 9 (D-DATA-17) |
| 4 | `_adjust_args_and_chdir` as a **pure function** (`ospath::abspath/relpath`, explicit cwd; no chdir) | 02 §3, 15 V-ADJ | cross-drive relpath keeps the absolute path (D-PLAT-06) |
| 5 | `store.mark_config_used(realpath(cfg), project_root)` (best effort, skipped if read-only / store absent); stash recovery scan when the worktree lock is free and the run is not nested (§4.6); the `run` hint (TTY only, §9.7) | 03 §7.8 | never creates the store just for tracking |
| 6 | **Git fan-out** (read-only, scoped threads, same argv as pre-commit): A `ls-files --unmerged`, B `diff --quiet --no-ext-diff <cfg>`, C ITA query, E staged list, F `ls-files -s -z` when Stat mode is possible (Unix, `stat_oracle_safe`, index-header count ≤ 16 384), otherwise the staged-only `diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB` for git-mode tags, G `config --list --show-origin -z` when Stat mode is possible (DiffOracle inputs). Main thread meanwhile reads + validates the config **speculatively** with warnings buffered (ADR-047) | 02 §5 | results consumed strictly in pre-commit's order; E/F/G skipped when unused (P17, all-files, files) |
| 7 | Preflight in exact order: unmerged → from/to-ref pairing → unstaged config → commit-msg filename → `_PRE_COMMIT_SKIP_POST_CHECKOUT` early 0 | 02 §5 | — |
| 8 | `run_env` = startup ⊕ run vars | 02 §6.1 | values are `OsString` (non-UTF-8 refs survive on Unix) |
| 9 | **Stash guard** (`stash = !all_files && files.is_empty()` and no stdin files); on git < 2.19 a non-empty stash is refused before any mutation (D-DATA-17) | 02 §7, §4 here | the guard is the only owner of worktree mutations until restore |
| 10 | Config: accept the speculative load iff B returned rc 0 (no unstaged change ⇒ the stash cannot change it); else `load_config` again inside the stash. Buffered warnings printed now (pre-commit's position). Overlay root items (`overlay::apply_root`, §8.4) | 02 §8 | exact by construction (S13) |
| 11 | `resolver.all_hooks` (snp-repo): eligible clones prefetched in the background, `Initializing` lines and foreground clones in config order at pre-commit's moment; manifest; merge + overlay hook patches; language default probes; fast-path decisions G1–G6 incl. G5 through `ResolveCtx::preflight` and the clone of delegated Tier A repos | 03 §8.1, 09 §6 | per-repo locks; shared store lock held from first repo use until exit |
| 12 | Select by `hook` id/alias (snp brand: several ids) and stage (+ `--group` extensions); `No hook with id …` → 1 | 02 §8 | stash restored on this early return (single exit) |
| 13 | `SKIP` (+ `SNP_SKIP_GROUPS`, overlay `skip`, `skip/only` conditions) → `to_install` | 02 §6.2, 15 V-SKIP | `strip_fsbytes` semantics for non-UTF-8 SKIP |
| 14 | `resolver.install_envs(to_install)` incl. delegated fast-path hooks and virtual install triplets | 03 §8.4, ADR-032 | `check_support` before any FS mutation (04b §5.1) |
| 15 | `cols` from **all** selected hooks (skipped included); file selection (§3.2); `[INFO] Checking merge-conflict files only.`; global filter; `retain_lexists` — the classifier is built lazily on the first hook that consults files (P17: never, when every selected hook is `always_run` ∧ `!pass_filenames`) | 02 §9–10 | — |
| 16 | Baseline digest (ADR-078): **warm path** (no clone, install or download ran in steps 11–14): `DiffFingerprint::EMPTY` when the stash step saw `diff-index` rc 0, else one `git diff` overlapped with steps 11–14; **cold path**: one `git diff` after `install_envs`, pre-commit's position (an IDE autosave during a long install must count as pre-existing, not as a hook modification) | 02 §11.2 | `SNP_VERIFY_BASELINE=1` (CI) takes the real diff and asserts equality |
| 17 | Schedule and execute (§3.4–3.7); print in config order (§3.9) | 02 §11 | one JobPool; cancellation stops new work |
| 18 | `fail_fast` (config ∨ hook ∨ `--fail-fast`): stop after the current failing hook (bucket) | 02 §11.2 | later hooks print nothing |
| 19 | `--show-diff-on-failure` (inside the stash) | 02 §15 | `git --no-pager diff …` inherits stdout (Printer flushed first) |
| 20 | `stage_fixed` staging (extension, §8.3) → `StashGuard::restore()` → ITA re-add → release locks | 02 §7, 05 A4 | restore runs even when 17–19 failed |
| 21 | Exit code: `retval` (0/1) or the `Failure` code; hook-impl ORs with the legacy rc | 02 §2.4 | crash log only for exit-1/3 `Exc` failures |

### 3.2 File selection per mode (first match wins; 02 §9)

| Mode | Source | Notes |
|---|---|---|
| post-checkout, post-commit, post-merge, post-rewrite, pre-rebase | `()` | still stash unless `-a/--files` |
| prepare-commit-msg, commit-msg | `(commit_msg_filename,)` | goes through the normal filters |
| `--from-ref` + `--to-ref` | `diff --name-only --no-ext-diff -z A...B`, retry `A..B` | no `--no-renames` here (changes output, 02 G.0 #9) |
| `--files` non-empty | CLI order, duplicates kept, abspath→relpath | nonexistent dropped by `lexists` |
| `--stdin-files[=nul\|lines]` (snp brand) | stdin, as if given to `--files` | mutually exclusive with `-a`/`--files` |
| `--all-files` | `ls-files -z --deduplicate` (fallback: adjacent dedup on git < 2.31) | gitlinks kept, filtered by types |
| merge conflict (`MERGE_MSG` ∧ `MERGE_HEAD` stat in git dir) | MERGE_MSG conflicts ∪ `diff --name-only --no-ext-diff -z -m <tree> HEAD MERGE_HEAD --` | union in **first-seen order** (pre-commit's set order is per-process random; COMPATIBILITY note N-1) |
| default | `diff --staged --name-only --no-ext-diff -z --diff-filter=ACMRTUXB` + `--no-renames` (output-identical, 02 G.7 row 3) | |

### 3.3 Filtering and regular expressions

1. Global `files`/`exclude` with `re.search` on the matching view, then per hook `files`/`exclude`, then
   `types ⊆ tags ∧ (types_or = ∅ ∨ tags ∩ types_or ≠ ∅) ∧ tags ∩ exclude_types = ∅` — three bitset ops.
2. **Regex layer = 01 Addendum §A.6, verbatim** (ADR-003): own CPython-**3.12** parser is the single accept/reject
   authority (nesting ≤ 484 accepted, ≥ 485 → validation error; repeat ≥ 4294967295 → validation error; `\z`,
   `(?<name>…)`, non-initial global flags rejected; `(?t)` with 3.12's compile-time rule); lowering emits explicit
   classes (`\w`→`[\p{L}\p{N}_]`, `\s`→`[\s\x{1C}-\x{1F}]`, IGNORECASE → simple-fold classes + Turkish-i rule) from
   `regex-syntax` UCD 16; **tier 1** `regex` 1.13 (str) / `regex::bytes` `unicode(false)` (pygrep) for patterns
   without backtracking constructs; **tier 2** `fancy-regex` 0.19.2 (`backtrack_limit(100_000_000)`, Latin-1 lane for
   bytes, never `BytesMode::Ascii`) for look-around/backrefs/conditionals/atomic/possessive, always behind the
   **mandatory relaxed tier-1 prefilter** (§A.6.11).
3. **Anchors** (§A.6.5): non-MULTILINE `$` is canonically `(?=\n?\z)`; the fast tier-1 form `\z` is used only when the
   haystack does not end with `\n` (`Path`), `(?m:$)` for pygrep by-line (`Line`); `\Z`→`\z`; Unicode `\b`/`\B` use
   the exact look-around forms with `W=[\p{L}\p{N}_]`, with the tier-1 `\b`/`\B` fast form only for ASCII (and, for
   `\B`, non-empty) haystacks. The per-match dispatch is
   `use_exact = (cond.nl && h.ends_with('\n')) || (cond.ascii && !h.is_ascii()) || (cond.nonempty && h.is_empty())`.
4. **Fast kinds** (S13, never touch an engine): `''` → always true; `'^$'` → `h.is_empty() || h == "\n"` (the Python
   truth, §A.6.5); literal prefix (`^lit`), suffix (`lit$` → `h.ends_with(lit) || h.ends_with(lit + "\n")`;
   `lit\Z` → `ends_with(lit)`), and contains (`memchr::memmem`) when the AST is a plain literal under no flags.
5. **Errors at match time**: a tier-2 backtrack-limit hit while filtering aborts the run with
   `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}` (exit 1,
   D-SEC-10); for pygrep the hook fails with that message (exit 1).
6. **FutureWarnings** (`[[`, `[a&&b]`, …): one stderr line per distinct (pattern, message):
   `[WARNING] regex {pattern!r}: {msg} (Python FutureWarning)`; silent with `SNP_PYTHON_WARNINGS=0` (D-MISC-04).
7. Validation (config load, `validate-config`) only *parses* patterns; engines are compiled on first match and cached
   per `(source, Options)` for the run.

### 3.4 Scheduling model and runtime: threads, no async (ADR-004)

* **Default (no extension keys):** hooks strictly sequential in config order — pre-commit semantics. Parallelism exists
  only across xargs partitions of one hook (and across native partitions).
* **`priority` extension (prek semantics):** implicit priority = index in the flattened config; hooks are bucketed by
  priority ascending; a bucket is a barrier; hooks inside a bucket run concurrently; `interactive` (v1.1) hooks are
  singleton buckets. `fail_fast` inside a bucket lets running hooks finish (their lines print), later buckets don't start.
* **One global `JobPool`** (snp-proc) with `N` permits bounds *all* concurrently running child processes and native
  partitions (S7, fixes prek #2272). `N = pool_size(--jobs | SNP_JOBS, target_concurrency())`: an explicit value as
  given, otherwise `min(target_concurrency(), available_parallelism())` — `target_concurrency` follows pre-commit
  exactly (`PRE_COMMIT_NO_CONCURRENCY` present → 1, `TRAVIS` present → 2, else `py_cpu_count()`, i.e.
  `sched_getaffinity`), and `available_parallelism` adds the cgroup CPU quota (a 2-CPU pod on a 64-core node runs 2,
  not 64, hooks at once) — then clamped to the fd budget `(RLIMIT_NOFILE soft − 64) / 8` (macOS default 256 → 24). The
  rlimit is never raised (children inherit it). *Partition contents* are always computed with pre-commit's per-hook
  `jobs` value, so pool size never changes argv or output. Clones/downloads use `NetLimiter` (8), installs
  `InstallLimiter` (`min(N, 4)`, 1 under a cgroup memory limit < 4 GiB; compile-heavy installs take every permit):
  installs never starve hooks and never multiply compile-time memory (ADR-065).
* **Re-entrancy:** only process partitions and native partitions hold permits; bucket-level hook threads hold none; a
  permit holder that calls `map_ordered`/`FileTable::prefetch` again (meta hooks classify files) runs that work inline,
  so N = 1 (`PRE_COMMIT_NO_CONCURRENCY`) cannot deadlock (ADR-070; test: check-hooks-apply with
  `PRE_COMMIT_NO_CONCURRENCY=1`).
* Execution uses `std::thread::scope` with `Builder::spawn_scoped` (a refused thread is a `Failure`): the main thread
  owns the guard and the printer ordering; worker threads only spawn, wait on their reader channels and return results. A single-partition hook runs inline on the main thread (no thread hop). A
  panicking worker is joined as `Failure::Exc(PanicException …)`; the main thread still restores the stash.

### 3.5 xargs partitioning and Python-exact shuffle

`run_xargs` per hook (02 §11.6–11.7, 04a §1.9):
1. `files = pass_filenames ? filtered : []`; if `!require_serial && plan.shuffle`: reorder by
   `snp_pycompat::random::precommit_shuffled_indices(n)` (fresh MT19937 seeded 1542676187 per hook).
2. `cmd = normalize_cmd(plan.cmd)` once per hook (cached per `(PATH, PATHEXT, name)` for the run); `ExeError` →
   `(1, "Executable `x` not found")` without partitioning.
3. `partition(cmd, files, jobs, max_len)` exact (`max_args = max(4, ⌈n/jobs⌉)`, byte/UTF-16 lengths, `+1`s,
   `ArgumentTooLong` → C-hook `[Errno 7] Argument list too long: '<exe>'`, X13-EXE-03). Windows `.bat/.cmd` budget +
   re-split when the quoted line would exceed 8191 (D-PLAT-09).
4. Partitions run on the pool (`threads = min(partitions, jobs)` for parity of concurrency), results concatenated **in
   partition order**, exit code = max |rc|, first wins.
5. Native hooks get the same treatment their Python counterparts get (ADR-021): pygrep unshuffled with serial
   partitions; pre-commit-hooks shuffled and parallel; `fail` one call with all files (no partitioning). The would-be
   command prefix for the length budget is a synthetic interpreter path of fixed length (`fast_path_coalesce: true`
   opts into one call). Boundaries can differ from pre-commit only within a few bytes of the 128 KiB limit (Tier C).

### 3.6 Process spawning

| Aspect | Rule |
|---|---|
| argv | resolved absolute exe as program and argv[0]; never a shell (S8) |
| stdin | non-pty: `Stdio::piped()` with the `ChildStdin` dropped immediately (empty pipe; Windows too, so CRT `isatty(0)` is false — 14 A.4.1); pty: `/dev/null`; `use_stdin` extension: a writer thread feeds a per-partition copy of the hook-impl stdin (≤ 64 MiB; larger stdin fails closed, §3.10); snp's own git/tool calls: `Stdio::null()` (ADR-019) |
| stdout/stderr | one `std::io::pipe()` writer cloned for both (ordering preserved), `Command` dropped before reading (07 §7.15); `--color` on POSIX → one pty per partition (master `openpt(RDWR\|NOCTTY\|CLOEXEC)`, slave `open(ptsname, O_RDWR\|O_NOCTTY\|O_CLOEXEC)` — NOCTTY so a session-leader snp never acquires the pty and gets SIGHUP when the master closes; `OPOST\|ONLCR` cleared, 0×0 size, no setsid, slave dropped before reading, read until `Ok(0)`/`EIO`); pty failure → pipe (X13-EXE-06) |
| reading | each read end is owned by a dedicated reader thread feeding a bounded channel (§1.4.8): normal runs read to EOF (parity); a deadline or cancellation ends the wait after the kill escalation + 500 ms drain and abandons the reader — no `poll` (macOS pty masters do not support it) |
| process group | default: inherit snp's group (terminal ^C reaches hooks, parity); `Group::Isolated` (Unix `process_group(0)`, Windows `CREATE_NEW_PROCESS_GROUP` via `CommandExt::creation_flags`) for `Role::Critical` git commands (§4.3) and hooks with a `timeout`; signal forwarding per §10.4 |
| capture memory | one global `OutputBudget` (default 64 MiB across all running children, `SNP_OUTPUT_BUDGET`); a capture that would exceed it continues into an **unnamed** spill file (`unnamed_temp` in `<store>/spill/`, 0700, or the OS temp dir: nothing survives a crash, hook output may contain secrets); printing streams from the spill (ADR-036, ADR-077) |
| fd hygiene (S15) | Linux: std pipes are `pipe2(O_CLOEXEC)`, the pty master and slave are opened with `CLOEXEC`, so no lock. macOS/other Unix: `snp_core::spawnlock::guard()` is held across every `spawn()` and every pty/pipe creation snp performs, so a sibling's non-atomic `pipe()+FD_CLOEXEC` can never leak into a concurrently spawned child (a leaked write end would delay EOF and hang daemonising hooks). Windows: std serialises handle inheritance internally |
| fd budget | the pool is clamped so permits × ~8 fds (merged pipe or pty pair, spill file, reader channel) stay within `RLIMIT_NOFILE`; an `EMFILE` at spawn is a `SpawnError` for that partition, never a panic |
| reaping | a worker blocks in `rustix::process::waitid(P_PID, WEXITED\|WNOWAIT)` (Critical: `\|WSTOPPED`, a SIGTTIN/SIGTTOU stop SIGKILLs the group and fails the command), marks the registry entry exited under its mutex, then reaps with `Child::wait`; killers only signal entries not yet marked exited (no pid reuse). Windows: the registry holds the process handle; hooks run inside the run's Job Object (no KILL_ON_JOB_CLOSE: daemons survive normal runs) |
| deadline | the reader-channel wait observes the deadline, so an escaped grandchild holding the pipe cannot hang a timed hook; on expiry SIGTERM the group, SIGKILL after 2 s; Windows: the hook's Job Object is terminated (kills grandchildren) |
| exit status | Unix `code()` or `-signal()`; Windows `code() as u32` printed unsigned (14 A.1 #4) |
| spawn errors | `SpawnError::python_text()` → `"[Errno 8] Exec format error: './x.sh'\n"` per partition (02 §11.8); no `/bin/sh` fallback on ENOEXEC (posix_spawn path: no `pre_exec`) |

### 3.7 Modification detection (ADR-027)

pre-commit's rule: `files_modified = (stdout of git diff --no-ext-diff --no-textconv --ignore-submodules) changed`,
run before the loop and after each *executed* hook. snp keeps a `DiffFingerprint { blake3, non_empty }` (streamed,
O(1) memory) and decides per executed hook with `snp_stash::diff_oracle::DiffOracle`:

1. **Native hook with `reads_only`** (pygrep, fail, meta, check-* pre-commit-hooks): digest unchanged — no spawn.
2. **Native fixer**: reports the paths whose bytes it changed. None → unchanged. Some → `files_modified` decided by
   09 §6.7 (some path tracked ∧ no `text`/`eol`/`working-tree-encoding`/`filter`/`ident` attribute, no
   `core.autocrlf`/`core.safecrlf`, no assume-unchanged/skip-worktree on it); with a caveat that hook falls back to a
   real before/after `git diff`. The digest becomes stale and is refreshed lazily by the next comparison.
3. **Process hook, `DiffMode::Stat`** (Unix only; an allow-listed filesystem per `snp_os::fsinfo::stat_oracle_safe` —
   ext2/3/4, xfs, btrfs, tmpfs, zfs, f2fs, bcachefs, apfs, hfs; every FUSE/virtiofs/9p/overlay/vfat/exfat/network
   filesystem uses Git mode; index entries ≤ 16 384; neither `attr.tree` nor `GIT_ATTR_SOURCE` set;
   `SNP_DIFF_MODE != git`; ADR-078):
   - Before the hook: snapshot `(dev, ino, size, mtime_ns, ctime_ns, mode, kind)` of every tracked path, plus **every
     input of `git diff`** other than tracked contents: the index file, `$GIT_DIR/config`, `$GIT_COMMON_DIR/config`,
     `config.worktree`, `info/attributes`, `.gitattributes` of the root and of every directory containing a tracked path
     (existence changes count — a new untracked `.gitattributes` can renormalise), the global and XDG git config
     files, the system config (unless `GIT_CONFIG_NOSYSTEM`), every `include.path`/`includeIf` target, the
     `core.attributesFile` target or — when it is unset — the default attributes file
     (`$XDG_CONFIG_HOME/git/attributes`, else `~/.config/git/attributes`), and the system `$(prefix)/etc/gitattributes`
     (from `git config --list --show-origin -z`, fan-out query G, taken once per run and re-taken when any config
     file's stat changes). `GIT_CONFIG_*`/`GIT_DIR`-style env cannot change during a run (snp's env snapshot is
     immutable and hooks cannot modify it).
   - **Racy entries** (mtime ≥ snapshot time − 2 s, git's own racy rule) are content-hashed with blake3 at snapshot
     time instead of disabling the oracle (after a stash checkout, every rewritten file is racy and is hashed).
   - After the hook: re-stat (parallel chunks when > 2 048 entries). Every tuple identical and every racy hash
     identical → **unchanged, no spawn**. Anything else → run the real `git diff`, compare digests (exact), take the
     next snapshot from the stats already collected.
   - Soundness: a content change with identical `(ino, size, mtime_ns, ctime_ns)` requires resetting ctime (changing
     the system clock); racy files are hashed; config/attribute inputs are covered. Gated by a 10 000-iteration
     randomized differential test (random tree edits, attribute/config edits, touch-only, same-size rewrites with
     restored mtimes) against real `git diff`, and `SNP_DIFF_MODE=git` exists as an escape hatch.
4. **`DiffMode::Git`** (Windows, network filesystems, > 16 384 tracked files, or `SNP_DIFF_MODE=git`): pre-commit's
   algorithm verbatim. On Windows stable std exposes no ctime/file-id, so no stat oracle is attempted.
5. **Baseline** (§3.1 step 16): on the warm path known-empty after `diff-index` rc 0 in stash mode, otherwise one
   baseline `git diff` overlapped with resolution; after any clone/install/download the baseline is taken after
   `install_envs` (pre-commit's position). Skipped and no-files hooks reuse the previous digest. Never
   `git diff --quiet` (CRLF and stat-dirty quirks).
6. **Parallel buckets** (extension): per-hook stat snapshots of each hook's input set + the bucket digest; a changed
   file is attributed to the single hook whose inputs contain it; otherwise every hook that could have caused it fails
   (conservative, fail closed; 05 §18.4 A1). blake3 content confirms stat changes.

### 3.8 fail_fast, SKIP, exported variables

* `fail_fast`: evaluated after each hook (bucket); remaining hooks print nothing (02 §11.2).
* `SKIP`: `get_skips` (Python `str.strip`, fsbytes-aware); matches id or alias; skipped hooks print the yellow
  `Skipped` line, count for `cols`, and are not installed. snp adds no alias variable for `SKIP` (`SNP_SKIP_GROUPS`
  only skips extension groups).
* Exported to hooks: exactly 02 §6.1 (truthiness rules, the `--origin`/`PRE_COMMIT_ORIGIN` naming trap, pre-rebase
  pair rule, all-four remote rule), `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` only on the stash's checkout, and
  `_SNP_STASH_LOCK=<token>` while a stash is held (D-EXT-04: lets a nested snp run skip the worktree lock).

### 3.9 Output printer (ADR-028, ADR-029)

* **Python two-layer stdout model (both brands).** `Printer::write*`/`log` = `sys.stdout.buffer.write` + flush;
  `Printer::py_print` = `print()` through CPython's text layer: when stdout is **not** a TTY the text is held in an
  8192-byte pending buffer (flushed into the byte layer when `pending + new > 8192` or `pending ≥ 8192`; the byte
  layer is flushed by the next `write*`) and finally at `finish()`; on a TTY it is line-buffered. `print()` sites:
  validate-config/manifest error blocks, migrate-config messages, hook-impl missing-config texts, sample-config,
  autoupdate's `Configuration has been migrated.`. This reproduces pre-commit's piped ordering byte for byte
  (validation blocks after later warnings, 01 §8.2).
* **Windows text mode.** `py_print`/`py_eprint`/argparse/`SystemExit` texts emit `\r\n` (Python text-mode stdio,
  12 §A.3); `write*` (hook output, status lines) stays LF. Hook output bytes are raw, written lossily only when stdout
  is a real Windows console (`_WindowsConsoleIO` parity).
* **Colour.** `--color` value, else `PRE_COMMIT_COLOR` (validated at end of parse), `auto` ⇒ `snp_os::term::decide_color`
  (Unix: stderr isatty ∧ `TERM != dumb`; Windows: `winapi-util` enables VT on the stderr console handle, exactly
  `color.py`). `NO_COLOR`/`FORCE_COLOR`/`CLICOLOR_FORCE` are ignored in both brands (parity).
* **Status lines.** `cols = max(80, max len_cjk(name) + 30)` over all selected hooks (UCD 15.1 EAW); `name....` is
  written and flushed *before* the hook runs, so a crash banner continues on the same line exactly as in pre-commit
  (13 X.1 placement rule). Colours exactly 02 §4 including `[INFO]\x1b[m`. Lines are produced by the pure
  `snp_run::render` functions, snapshot-tested without processes.
* **Detail block** iff `verbose || hook.verbose || code != 0 || modified`: `- hook id:`, `- duration: {d}s` (verbose,
  via `float::format_duration`), `- exit code: {code}`, `- files were modified by this hook`, extension lines
  (`fail_text`, `stage_fixed`, timeout), then if `bytes.strip(out)` is non-empty: `\n` + stripped bytes + `\n`.
  `log_file`: the output block is printed first, then appended to `root/log_file`; a write failure prints a warning and
  continues (D-CRASH-15).
* **Parallel buckets**: head-of-line ordering — a hook's block is printed when it and all earlier hooks are done; bytes
  identical to the sequential rendering.
* **EPIPE**: every write handles `EPIPE` by latching closed: no further hooks start, the stash is restored, exit 120,
  no stderr noise, no crash log (X13-EH-03).
* **snp-only diagnostics** on `run`/`hook-impl` paths go to **stderr** (`Printer::diag`), keeping stdout identical.

### 3.10 `hook-impl` (02 §16, 03 §6, X13-HI-01)

Order (pre-commit's `hook_impl`): parse (free-string `--hook-type`) → **legacy phase**: `PRE_COMMIT_RUNNING_LEGACY`
or `SNP_RUNNING_LEGACY` non-empty ⇒ the `bug: … migration mode` SystemExit text (stderr, exit 1, byte-identical in
the compat brand); pre-push: read stdin fully — more than 64 MiB fails closed before the legacy hook runs, with
`An error has occurred: FatalError: snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list` (exit 1,
never truncated; D-SEC-12); `--hook-dir` CDPATH repair (D-DATA-07); executable
`<type>.legacy` ⇒ run via `normalize_cmd` with stdin = captured bytes for pre-push and **empty for every other type
(post-rewrite included, parity)**, stdio inherited, child env with both legacy variables; a broken legacy hook (missing
interpreter, directory) → stderr message and legacy rc 1 (D-CRASH-14) → **kill switch**: `SNP=0|false` and the config
does not set `allow_disable: false` (read leniently: a missing or invalid config counts as `true`) ⇒ return the legacy
rc and run nothing else (ADR-038) → config check (missing-config texts via `py_print`; `--skip-on-missing-config`,
`PRE_COMMIT_ALLOW_NO_CONFIG`) → arity check (`SystemExit` with Python list repr) → unknown hook type ⇒
`AssertionError: unexpected hook type: X` (exit 3) → namespace mapping; pre-push via `snp_git::Git::pre_push` (Python
`splitlines` + `rsplit(maxsplit=3)`, malformed line ⇒ exit 3 `ValueError: not enough values to unpack (expected 4,
got N)`; non-UTF-8 input parsed on bytes, D-CRASH-11; any all-zero OID of 40 or 64 hex is null, D-CRASH-10; first
pushable ref wins) → `rc = legacy | run` (Windows: full u32, via `snp::Exit::Raw` and `process::exit` in `main`).
`rest[1:]` is dropped unconditionally (parity, 02 G.0 #3). `hook-impl` prints nothing snp-specific on stdout (the
recovery notice and D-DIAG lines go to stderr).

### 3.11 Meta hooks and native hooks in the pipeline

* **meta** (`check-hooks-apply`, `check-useless-excludes`, `identity`) are implemented in `snp-run` (they need the
  pipeline): config **always** `.pre-commit-config.yaml` regardless of `-c` (+ args/filenames as config paths, quirk
  kept, 01 §9); warnings replayed **bare** into the hook output (no `[WARNING]`, no colour) through `CollectWarnings`;
  `check-useless-excludes` uses config-level remote hook dicts with manifest defaults and `types: []`, `{exclude!r}` via
  `repr_str`; pre-commit's `exclude == '^$'` short-cut stays a string comparison.
* `ExecKind::Native` and `RunPlan::{Fail, Pygrep}` never spawn: the runner computes the same shuffle/partitioning a
  subprocess would get, calls `run`/`run_partition` per partition on the pool with the partition's full argv (args +
  files, ADR-072), concatenates outputs, aggregates codes. A meta hook that classifies files runs its nested pool work
  inline (§3.4).
* An invalid pygrep entry is compiled **once per hook per run**: the hook fails with one line in the exact Python form
  (`re.error: … at position N`, `OverflowError: the repetition number is too large`, …) instead of pre-commit's one
  traceback per partition (Tier B, D-MISC-05).

---
## 4. The staged-files stash guard (`snp-stash`)

This is the highest-risk code: it holds the only copy of the user's unstaged work. The algorithm is **pre-commit's,
byte for byte, in the user-observable git state and messages** (02 §7, ADR-016); the additions are durability,
journalling, mutual exclusion, signal isolation and exit-path coverage that pre-commit lacks. 06's
`read-tree`/`diff-index`-verify restore is rejected as the default (different git state on conflict; false
mismatches under `core.autocrlf`, 14 A.4.5) and survives only as the opt-in, warn-only `SNP_STASH_VERIFY=1` and
`snp doctor --stash` check (`snp_stash::verify_restored`: `git apply --check --reverse <patch>` after the restore).
A lefthook-style backup stash entry (`SNP_STASH_BACKUP`, 05 A5 item 6) is rejected (ADR-067): it would add state
pre-commit never creates (the stash list is compared, K3), and the durable patch + journal + git-dir pointer already
make every interruption recoverable.

### 4.1 State machine

```
Idle ──lock──▶ Locked ──(journal + git-dir pointer created lazily)──▶ Started ──ITA list journaled, rm --cached──▶ ItaCleared
   ──patch durable + journaled + re-verified──▶ PatchWritten ──checkout -- .──▶ Stashed ──(hooks run)──▶
   [StagingFixed {paths} journaled ──git add──▶] Restoring
   ──apply ok──▶ Restored ──ITA re-added──▶ Done (journal + pointer deleted, lock token truncated, unlock)
                                      └──apply fails twice──▶ RolledBack ──apply ok──▶ Restored
                                                                     └──apply fails──▶ Failed (K+: patch + journal kept)
```

Every transition is recorded in the journal *before* the corresponding git mutation, so a crash at any point leaves
enough information on disk to recover (§4.6). Durability (ADR-067): `PatchWritten`/`Stashed` and `StagingFixed` are
fsynced (they protect user data against power loss); `RolledBack`, `Failed`, `Restored` and the lock token are written
without fsync (the page cache survives process death; losing them on power loss only yields a harmless recovery
notice).

### 4.2 Algorithm (exact commands; `E` = run_env)

1. **Worktree lock** (ADR-026, ADR-067): `<git_dir>/snp-stash.lock` (per worktree: `--git-dir` of a linked worktree is
   its own dir), exclusive `try_lock`; on contention print `snp: waiting for another snp run in this worktree…`
   (stderr) once and wait up to 60 s (`SNP_STASH_LOCK_TIMEOUT`, ≤ 1 h), then FatalError. A nested run (a hook that
   itself runs snp) sees `_SNP_STASH_LOCK` in its env equal to the random token written in the lock file **and** finds
   the lock actually held (its own non-blocking `try_lock` fails) — only then does it skip the lock (in pre-commit the
   nested run sees a clean worktree); a token left in a stale lock file never exempts anyone, and the token is
   truncated on release. Taken *after* the `_PRE_COMMIT_SKIP_POST_CHECKOUT` early return, so the nested post-checkout
   run cannot deadlock (02 G.10). With the lock held the recovery scan runs (§4.6). On git < 2.19 (`StashCtx::old_git`)
   a stash that would be non-empty is refused at step 5, before any mutation, with `An error has occurred: FatalError:
   git <V> cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged changes (found
   <git>).` (exit 1, D-DATA-17).
2. **Journal** `<store>/patches/run-<pid>-<rand>.snp-journal` (`{v, state, token, worktree, git_dir, common_dir,
   index_file, head, pid, start_time, snp_version, ita:[…], tree, patch, staged_fixed:[…]}`) and the **git-dir pointer**
   `<git_dir>/snp-stash.journal` (`{journal, patch, tree, state}`, so recovery works from any store — a GUI client with
   another `SNP_HOME` still sees it) are created lazily with `create_new`, immediately before the first mutation (step
   3's `rm --cached` or step 8's checkout) — clean runs never touch the disk; every later state is written (temp +
   rename, fsync per the durability rule of §4.1) *before* the git mutation it describes. If the store is not writable
   and only ITA files exist (no patch to write), the store journal is skipped with a stderr note (pre-commit needs no
   store write there; parity of success); the pointer is still written.
3. **Intent-to-add.** `git diff --no-renames --no-ext-diff --ignore-submodules --diff-filter=A --name-only -z`
   (checked; `--no-renames` fixes hazard (b), D-DATA-12). If non-empty: `[WARNING] Unstaged intent-to-add files
   detected.`; the list is journaled; then `git rm --cached -- …` (xargs-partitioned, rc ignored, as pre-commit).
4. **`tree = git write-tree`** (checked → exit 3 on failure).
5. **Diff.** `git diff-index --ignore-submodules --binary --exit-code --no-color --no-ext-diff <tree> --` with
   `GIT_DIFF_OPTS` removed (hazard (a), D-DATA-11). stdout is **streamed** (never held in memory) directly into the
   final name `patches/patch<unix_secs>-<pid>` created with `create_new` 0600 (EEXIST → `-<n>` suffix: two runs with
   the same second and pid — possible across PID namespaces sharing a store — never overwrite each other), hashing it
   with blake3 on the way; stderr captured.
   * rc 0, or rc 1 with `bytes.strip()`-empty output → delete the file, no stash (`diff_index_clean()` is true
     only for rc 0).
   * other rc → FatalError `{prog} failed to diff -- perhaps due to permissions?\n\n<CalledProcessError format A>`.
6. **Make the patch durable.** `sync_all` the patch, write the journal (`{state:"stashed", tree, patch}`, `create_new`)
   and the pointer, `sync_all` them, fsync the directory **once**. Then **re-verify**: run `diff-index` again into a
   blake3 hasher (no file); a different digest (an editor autosave landed during the fsyncs) rewrites the patch and
   re-verifies, at most 3 times, else FatalError `snp: unstaged files kept changing while stashing; nothing was
   modified` (exit 1). The capture→checkout window is then as short as pre-commit's. Print `[WARNING] Unstaged files
   detected.` / `[INFO] Stashing unstaged files to <abs path>.`. A full disk or EIO here aborts **before any worktree
   mutation**.
7. (No process-wide restore registry, ADR-066: the explicit `restore()` on the main thread, the panic-path
   `RunState::drop` and token cancellation cover every exit path; the panic hook and the signal thread never restore.)
8. **`git -c submodule.recurse=0 checkout -- .`** with `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`, `Role::Critical`
   (checked). A failure here still goes through restore (pre-commit's `finally` semantics).
9. Hooks run. `stage_fixed` (extension) may stage files via `StashGuard::stage_fixed` (§8.3).
10. **Restore** (always, §4.4), every command `Role::Critical` (Isolated group, never signalled, never cancelled,
    `GIT_TERMINAL_PROMPT=0`, a SIGTTIN/SIGTTOU stop fails the command with one stderr line `snp: git needed the
    terminal during the restore and was stopped`), under a `RestoreShield`; on the cancel path `registry::quiesce`
    first kills the cancelled hooks' remaining descendants (§10.4):
    a. `git apply --whitespace=nowarn <patch>`; on failure `git -c core.autocrlf=false apply --whitespace=nowarn <patch>`.
    b. Both failed → journal `rolling-back`; `[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back
       fixes...`; `git -c submodule.recurse=0 checkout -- .` (same env); apply again (both variants).
    c. Still failing → journal `failed`; `Failure::Exc(Unexpected, CalledProcessError …)` with the **K+ hint** written
       to **stderr** after the log line: `[ERROR] Your unstaged changes are saved in <patch>; restore them with:
       git apply --whitespace=nowarn <patch>`, plus the fallback for a worktree the rollback could not reset (keep a
       copy with `git diff --binary <tree>`, `git restore --source=<tree> --worktree` the patch's paths, apply again;
       X13-STASH-02, D-DATA-01, ADR-084). Patch, journal and pointer are kept forever (never gc'd).
    d. Success → `[INFO] Restored changes from <patch>.`; journal `restored`; the patch file is kept (parity; gc prunes
       restored patches after 30 days, D-DATA-04).
    e. `SNP_STASH_VERIFY=1`: `verify_restored` warns on stderr if the patch is not present in the worktree (never
       changes the outcome).
11. **ITA re-add:** `git add --intent-to-add -- …` (partitioned, rc ignored, `Role::Critical`); journal and pointer
    deleted; lock token truncated; worktree lock released.

### 4.3 Crash and signal safety

| Event | Behaviour |
|---|---|
| Error/`Failure` anywhere after step 6 | propagates to `run()`, which calls `guard.restore()` before returning (explicit path) |
| Panic in the main thread | the panic hook only records the payload (try-lock slot) and cancels the token (reason `Panic`); unwinding runs `RunState::drop`, which restores inside `catch_unwind` before any field is dropped and prints restore failures; then the top-level guard prints `An unexpected error has occurred: PanicException: …` (exit 3). `panic = "unwind"` is load-bearing (FD-5) |
| Panic in a worker | the hook cancels the token (no new work starts); joined as `Failure`; the main thread restores normally; poisoned locks are recovered (S14) |
| SIGINT (terminal ^C) | hooks receive it from the kernel (same pgrp, parity) and are not re-signalled; timed (Isolated) hooks get SIGINT by `-pgid`; token cancelled; no new partition/hook starts; after 2 s (`SNP_KILL_GRACE`) remaining Hook/Tool children are SIGKILLed (Critical never); `quiesce` kills leftover descendants (Linux subreaper / Windows job); restore; `Interrupted (^C): KeyboardInterrupt: ` + log line; exit 130 |
| SIGINT sent with kill(2) to snp only (IDE, CI cancel) | origin `Sent`: forwarded as SIGINT to every Hook/Tool child (§10.4), then as above |
| 2nd/3rd terminal ^C during restore | the restore's git children are in their own process group (Unix) / a new process group without a console (Windows: `CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW`), so the kernel/console never delivers it to `git apply`; the registry never signals Critical children; snp's own handler only prints one stderr notice; the restore is never abandoned |
| SIGTERM / SIGHUP | forwarded per origin (§10.4: only when sent by a process — a tty hangup already reached the foreground group); same unwind, descendant quiesce and restore (restore messages printed); then snp re-raises the signal with the default disposition so the parent sees status 143/129, exactly like pre-commit's death by signal — but with the stash restored (D-DATA-02; macOS cannot reach orphaned descendants of Inherit-group hooks — documented residual risk) |
| Windows console close / logoff / shutdown | Windows ends the process as soon as the handler returns: no restore runs; the journal + pointer make it a next-run recovery (§4.6); an in-flight Critical `git apply` has no console and is not killed by the event |
| A restore command stopped by SIGTTIN/SIGTTOU (LFS credential prompt, `tcsetattr` in a post-checkout hook) | its group is SIGKILLed, the command counts as failed, the normal rollback/K+ path keeps the patch; one stderr line names the cause |
| EPIPE on stdout | printer latches closed, token cancelled (StdoutClosed); restore still runs; exit 120 (X13-EH-03, X13-STASH-03) |
| Full disk / EIO writing the patch | before any worktree mutation → FatalError, worktree untouched |
| SIGKILL / power loss after step 6 | journal + git-dir pointer + durable patch on disk; the next snp start in that worktree (any store) detects it (§4.6) |
| Stack overflow / double panic (abort) | cannot run code; the journal enables recovery; R-STACK rules (§11.4) make this unreachable from input |

### 4.4 Single-exit discipline

`run()` is `let result = run_inner(&mut state); let restore = state.stash.take().map(StashGuard::restore);` followed
by error merging (a restore failure wins over hook failures; an `Interrupted` reason is preserved). `run_inner`
contains every `?`; `run()` and `hook_impl()` contain no `?` and no early `return` (checked by `xtask lint-guards`,
which parses `crates/snp-run/src/{run,hook_impl}.rs` with `syn`), and every `snp-stash`/`snp-run` PR needs a second
reviewer. `Drop` is the panic fallback only.

### 4.5 Interaction with `stage_fixed`, show-diff and hook-impl

* show-diff runs inside the stash (parity: it shows hook changes only).
* `stage_fixed` runs after all hooks and before restore: `git apply --check <patch>` against the hook-fixed worktree;
  if it applies, the journal records `staging-fixed {paths}` durably, then `git --literal-pathspecs add -- <accepted
  paths>` (`Role::Critical`; index now contains the fixes and no unstaged user content), then the normal restore
  applies the user's unstaged changes on top. If `--check` fails, nothing is staged,
  the accepted hooks are reported failed and the normal conflict path runs (05 A4/A5).
* `post-checkout` re-entry: the stash checkout sets `_PRE_COMMIT_SKIP_POST_CHECKOUT=1`; the nested `hook-impl` returns
  0 before config load and before the lock (02 §21.10).

### 4.6 Recovery

Liveness is the worktree lock, never a pid (ADR-067: pids are reused, meaningless across PID namespaces sharing a
store, and a pid check cannot see a GUI client's other store). On every non-nested `run`/`hook-impl` start (step 5,
before any preflight can end the run), `recovery_report` tries the worktree lock without blocking; if it is held,
another live run owns the worktree and nothing is reported then — a stashing run reports inside `enter` once it holds
the lock (`StashCtx::report_recovery`), so each notice is printed exactly once per run on every path (ADR-083); if
acquired, every non-`restored` journal for this worktree —
found through the git-dir pointer (any store) and one `readdir` of this store's `patches/` — is orphaned by definition,
and produces on **stderr**: `snp: an earlier run was interrupted before it restored your unstaged changes; they are
saved in <patch> (index tree <tree>). Restore with: git apply --whitespace=nowarn <patch>` (worktree only and
all-or-nothing: it never overwrites a hook fix or work done after the kill, ADR-084), followed — the run's own index
only, never a temporary `GIT_INDEX_FILE` — by `&& git --literal-pathspecs reset -q <tree> -- <stage_fixed paths…>` (or
`git read-tree <tree>` when the list is long), by the intent-to-add re-add (a long list is kept in `<patch>.ita`, read
with `--pathspec-from-file`, and outlives the journal), and by what to do when the apply refuses (keep a copy with `git
diff --binary <tree> > <patch>.now`, `git restore --source=<tree> --worktree` the patch's paths, run it again) — never
auto-applied (the user may have continued working). The notices of a pass are followed once by `snp: this notice repeats until
those changes are back in the worktree; to dismiss it and keep the patch, run: snp doctor --fix --stash`. Dismissal
(ADR-083): an orphan whose patch is already applied to the worktree (`git -c apply.ignoreWhitespace=no apply --check
--reverse --whitespace=nowarn <patch>` succeeds and every mode the patch sets is the worktree file's, ADR-084 —
the user ran the printed command, or restored the work another way) is retired by that pass: its journal and pointer
are removed and `snp: the unstaged changes saved in <patch> are back in the worktree; the recovery notice of that
interrupted run is retired` is printed once (plus the intent-to-add notice when the run had cleared entries).
`snp doctor --fix --stash` retires every orphan of the worktree explicitly, applied or not, naming each kept patch.
Retiring never deletes the patch; its modification time is reset, so `gc`'s 30-day retention (D-DATA-04) counts from
the retirement. `snp doctor` lists the orphans; `snp cache gc`/`gc` never deletes a journaled patch.

### 4.7 Fault-injection coverage (`snp_git::testing::ScriptedBackend`)

FI-01…FI-30: fail each git call (write-tree, diff-index rc 2, checkout, apply ×2, rollback checkout, apply ×2 again,
rm --cached, add -N, stage_fixed add), ENOSPC at patch write and at journal write, panic injected after each state
transition (`snp_core::fault::Point::StashState`), SIGTERM at each transition (process-level test through the
`snp-stash-driver` test binary), EPIPE during restore messages, concurrent second `snp run` (lock), nested run with
token, stale token in a released lock file (must not exempt), CRLF quirk (rc 1 + blank), binary patch,
`GIT_DIFF_OPTS=--unified=0`, ITA + rename (hazard b), second ^C during `git apply` (process-group isolation), ^C with
a hook that exits at once and a slow `git apply` that must survive the 2 s grace window (Critical never killed), a
hook leaving `sh -c '(sleep 1; printf x >> tracked.txt) &'` plus SIGTERM to snp's pid (worktree + index equal the
pre-run state after restore; Linux), an autosave between diff-index and checkout (re-verify rewrites the patch), a
restore command stopped by SIGTTIN (posix), git < 2.19 with unstaged changes (refused before mutation), kill-9 at
`StagingFixed`. Invariant asserted after each: worktree + index + patch + journal together reconstruct the pre-run
state (`git diff <tree>` vs the original unstaged diff bytes), and the recovery notice names a command that restores
it.

---

## 5. Store (`snp-store`)

### 5.1 Location (FD-2, ADR-011)

`SNP_HOME` (non-empty) → `$XDG_CACHE_HOME/snp` (non-empty; honoured on every OS, as pre-commit does) →
`expanduser("~/.cache")/snp` = `~/.cache/snp` on Linux/macOS (not `~/Library/Caches`) and `%USERPROFILE%\.cache\snp`
on Windows (14 P-65). Relative values resolve against the invocation cwd *before* any repo-root computation; the result
is `realpath_nonstrict`'d. **`PRE_COMMIT_HOME` is never used for snp's location**; snp computes pre-commit's store path
only to *refuse* it: if the resolved path equals pre-commit's store or its `README` first line is pre-commit's
signature → FatalError `{prog} refuses to use pre-commit's cache directory {path}; set SNP_HOME to another directory`
(M7). snp never opens, creates or deletes `db.db`, `.lock`, `pre-commit.log`, `repo*`, `patch*` of pre-commit, nor
`~/.pre-commit`. Ownership: created dirs are 0700; a world-writable non-sticky store is refused; a store owned by
another uid produces one stderr warning (containerised CI with volume-mounted caches keeps working, ADR-044,
D-DIAG-05) — except when snp runs as root (euid 0) and the store belongs to a non-root uid: snp would execute tools and
environments that uid can replace, so it refuses with `An error has occurred: FatalError: snp refuses to run as root
from a store owned by uid <n>: <path>; set SNP_ALLOW_FOREIGN_STORE=1 to trust it` (exit 1) unless
`SNP_ALLOW_FOREIGN_STORE=1` (git's `safe.directory` rationale; ADR-077, D-SEC-13).

### 5.2 Layout (versioned buckets; snp 1.x names `db.db`, `repos`, `environments`, `resource-pools` are never reused)

```
<store>/                                  0700; created lazily (only by commands that need it)
├─ README                                 "This directory is maintained by snp (You Shell Not Pass).\nLearn more: https://github.com/grok-rs/snp\n"
├─ store-v1.json                          {"format":1,"path":"<abs store path>"} → relocation detection (envs invalid after a move)
├─ .lock                                  global lock: SHARED by runs using repos, EXCLUSIVE for gc/clean
├─ pre-commit.log | snp.log               last crash log, named by brand (O_NOFOLLOW, truncate; ADR-013)
├─ repos-v1/<key>/                        key = hex(blake3("snp.repo.v1\0" ‖ lp(repo_id) ‖ lp(rev) ‖ json(deps)))[..32]
│     ├─ .snp-repo.json                   commit marker (written last): {repo, rev, deps, kind, object_format, created, snp_version}
│     ├─ src/                             the clone (detached at rev), or the local template files (local keys)
│     └─ envs/<lang-base>-<ver|h16>/      environments; .snp-tools.json (store-owned tool refs) then
│                                         .snp-install-state.json (written last: the commit point)
├─ repos-v1/<key>.lock                    per-repo lock: clone + env installs of this prefix
├─ keys-v1/<hex32>                        message-parity markers (Initialized / Installed; ADR-032)
├─ configs-v1/<hex32>                     config tracking: key = hash(config realpath); contents = realpath bytes NUL
│                                         project-root bytes (relative `repo:` paths resolve against it at gc time)
├─ tools-v1/<tool>/<version>/ + <tool>.lock   uv, node, go, rustup(+toolchains), rbenv/ruby-build; marker .snp-tool.json last
├─ cache-v1/                              git-version facts and probe facts (native binaries only, §6.1), uv/go/npm caches (optional, atomic)
├─ patches/                               patch<secs>-<pid>[-n] (stash patches, create_new) + run-<pid>-<rand>.snp-journal
├─ spill/                                 0700 directory for unnamed spill files (O_TMPFILE / unlinked at creation)
├─ scratch/<rand>/                        in-progress clones/extractions (same filesystem → atomic rename)
└─ trash/<rand>/                          retired dirs pending deletion (gc/clean never delete in place)
```

`repo_id` = the config string verbatim for URLs, or `abs:` + the canonical absolute path when the repo is an existing
local directory (fixes pre-commit's relative-path cache collision, D-DATA-08; messages still print `db_repo_name`
verbatim). The only derivation is `RepoId::from_config(repo, project_root)` + `db_repo_name` in snp-store, used by
snp-repo (resolution) and snp-cmd (gc marking) alike; relative paths resolve against `ConfigSource::project_root` (the
git root in v1, where pre-commit's cwd is after `_adjust_args_and_chdir`). Local pseudo-repos use `"snp.local.v1"` domain separation with `LOCAL_TEMPLATE_VERSION` in the key.
**Every distinct `additional_dependencies` list gets its own key and prefix** (parity: installers mutate the prefix —
`cargo add` edits `Cargo.toml`, dart/dotnet/gem/pip leave build residue, 04a §5, 04b §5.5); a deps variant of a remote
repo is created by `git clone --local --no-checkout` from the base clone + `checkout <rev>` (hard links, no network;
`--shared` alternates are rejected because removing the base would break the variant; ADR-025).

### 5.3 Clone and fetch

Under `repos-v1/<key>.lock` (double-checked: marker → lock → marker again):
1. `scratch/<rand>/src`; owned-repo env (`no_git_env` + fsmonitor injection; never the user's `GIT_DIR`/`GIT_INDEX_FILE`).
2. pre-commit's exact sequence (03 §7.6): `init --template=` (with `-c core.useBuiltinFSMonitor=false`), `remote add
   origin` (abspath if local dir), `-c protocol.version=2 fetch origin <rev> --depth=1`, `checkout FETCH_HEAD`, shallow
   submodules; on any failure the full fallback (`fetch origin --tags`, `checkout <rev>`, `submodule update --init
   --recursive`). A `mismatched algorithms` failure on git ≥ 2.29 re-inits once with the other `--object-format`
   (D-SEC-06). Both strategies failing ⇒ the fallback's CalledProcessError (exit 3, X13-GIT-01).
3. **Prefetch** (ADR-064, an exact optimisation): at resolution start, missing remote `(repo, rev)` clones whose URL
   is `prefetch_eligible` (https://, file://, local paths; ssh/scp only when none of `GIT_SSH_COMMAND`, `GIT_SSH`,
   `core.sshCommand` is set, so `ssh -o BatchMode=yes` can be injected) start in the background on the `NetLimiter`
   with `CloneMode::Prefetch` (no terminal prompt, no askpass GUI, no GCM dialog, BatchMode ssh) into their own
   scratch dirs. **Any** prefetch failure is a cache miss: its scratch dir is discarded, its stderr is never shown, and
   when resolution reaches that repo in config order the clone is redone in the foreground exactly as pre-commit runs
   it — same env, prompts enabled, same moment, under CI too — so credential prompts, error texts (X13-GIT-01) and
   locale behaviour are pre-commit's. Ineligible URLs are cloned only in the foreground, serially in config order (at
   most one interactive prompt is ever active). `SNP_PREFETCH=0` disables prefetching.
4. Write `.snp-repo.json` (durable), `rename(scratch/<rand>, repos-v1/<key>)`; on `EEXIST/ENOTEMPTY` (lost race)
   discard ours. Kill -9 at any step leaves only `scratch/<rand>` (swept after 1 h when no lock holds it).
5. **Messages** (ADR-032): `[INFO] Initializing environment for {db_repo_name}.` (`repo` or `repo:dep1,dep2`, and
   `local[:deps]`) is printed exactly when pre-commit would insert a new db row: the first time this store sees
   `(db_repo_name, rev)` (`keys-v1` marker absent), **in config order, at pre-commit's moment**, whether or not a
   network clone happens (Tier A fast-path repos included). The marker is set when the corresponding store entry is
   complete. The deprecated-stage manifest warning prints once per *new* clone (01 §4.3).

### 5.4 Environments and install state (ADR-009, ADR-012)

* `env_dir = <repo>/envs/<ENV_DIR>-<version>` when `version` matches `^[A-Za-z0-9._+-]{1,64}$` and is not `.`/`..`;
  otherwise `<ENV_DIR>-h<hex16(blake3(version))>`. The name never contains a separator, so `language_version` cannot
  escape (04b §5.1 traversal, D-SEC-01). Deletion uses `remove_tree_within(env_dir, <repo>/envs)`.
* Install (snp-repo, under the per-repo lock): `check_support` → env dir exists without a valid state → retire it →
  remove-on-drop guard → `lang.install()` (env dir absent on entry, 04b §5.6) → `health()` (failure → the verbatim
  `BUG: expected environment for {lang} to be healthy immediately after install` AssertionError, exit 3) →
  `commit_install_marker(InstallState::to_json())` (temp + fsync + rename) → disarm. **Only**
  `.snp-install-state.json` is written; pre-commit's `.install_state_v1/v2` are never written or read.
* "Installed" = state parses ∧ `deps` equal ∧ `language_version` equal ∧ `health()` healthy (fingerprint fast path:
  zero spawns when the recorded interpreter stat is unchanged). Malformed state → reinstall (X13-ENV-01).
* Distinct `InstallKey`s on distinct prefixes install **in parallel** (`InstallLimiter`), each under its own repo lock;
  the `[INFO] Installing environment for {src}.` / `[INFO] Once installed this environment will be reused.` /
  `[INFO] This may take a few minutes...` triplet of key *k* is printed only after key *k−1* finished, so the transcript
  equals the sequential one; the first failure in config order wins (later in-flight installs are cancelled and their
  dirs removed). Installer chatter is captured and shown only inside a CalledProcessError text.

### 5.5 Locking and concurrency between processes

| Lock | Mode | Holder | Purpose |
|---|---|---|---|
| `<store>/.lock` | shared | every run/hook-impl/try-repo/install-hooks from first repo use to exit | gc can never delete a repo in use |
| `<store>/.lock` | exclusive | gc, clean, store-format migration | — |
| `repos-v1/<key>.lock` | exclusive | clone and env installs of that prefix | distinct repos clone/install in parallel |
| `tools-v1/<tool>.lock` | exclusive | toolchain bootstrap | single download per tool/version |
| `<git_dir>/snp-stash.lock` | exclusive (token for nested runs) | stash guard | concurrent commits in one worktree (§4.2) |

Ordering (deadlock freedom): stash lock → global(shared) → repo → tool; no lock other than the shared global one is
held while hooks execute. Contention prints `[INFO] Locking pre-commit directory` (compat) / `[INFO] Locking snp
directory` (snp brand) once per process, then waits (cancel observed every 50 ms). `std::fs::File::lock`/`try_lock`
(flock / LockFileEx); no lock interop with pre-commit is needed (disjoint stores).

### 5.6 gc, clean, crash log, sweep

* **gc** (exclusive; marking in `snp_cmd::gc`): read `configs-v1` (`TrackedConfig { path, project_root }`); for each
  tracked config: `load_config` (warnings printed, parity) → mark used keys with pre-commit's `_mark_used_repos` rules
  (meta skipped, local keys by deps, remote by `(repo, rev)` and per-hook deps, manifest unreadable ⇒ unused), deriving
  every key with `RepoId::from_config(repo, tracked.project_root)` — relative local repos resolve against the recorded
  project root, never gc's cwd (ADR-075); a remote `(repo, rev)` whose manifest is loadable from the embedded Tier A
  profile counts as "manifest loadable" even without a `repos-v1` entry, so its `keys-v1` `Initialized` and virtual
  `Installed` markers are kept (pre-commit keeps the clone it still uses, so a later run prints nothing new); dead
  configs forgotten (incl. configs that fail C-cfg validation, X13-CFG-14); unused repos retired to `trash/` then
  removed (ENOENT-tolerant, X13-CMD-07); then `sweep`: scratch/trash orphans, tools referenced by no `.snp-tools.json`,
  restored patches older than 30 days (journaled-unrestored patches are **never** removed), `keys-v1` markers of
  `(db_repo_name, rev)` keys that are neither present nor Tier-A-loadable, snp 1.x leftovers only when the README
  carries the 1.x signature (an snp-only stderr line `Removed snp 1.x cache data from {store}.`, D-DIAG-05).
  Output: `{n} repo(s) removed.` (n counts `(db_repo_name, rev)` keys, like pre-commit's rows).
* **clean** (exclusive): retire snp's whole layout — every journal, the patches of unrestored journals, `patch*`
  files younger than an hour, `.lock` and files outside snp's layout are kept (D-DATA-04); always prints `Cleaned {store}.` (pre-commit re-creates its store
  before `clean` and always prints, 03 §10; ADR-030). Never touches `~/.pre-commit` or pre-commit's store (D-STORE-02).
* **Crash log** `<store>/<brand.crash_log_name()>` (ADR-013): pre-commit's skeleton (§10.3). If the store cannot be
  created/written: `Failed to write to log at …` and the body on stdout (parity branch, never a double fault).
* **Read-only store:** works when everything needed exists (parity, 03 §7.9); config tracking silently skipped.

### 5.7 try-repo's throwaway store (ADR-059)

pre-commit runs try-repo against `Store(tempdir)` (03 §12 step 3), so every try-repo clones and installs from scratch
and prints `[INFO] Initializing environment for …` and the `Installing environment` triplet on **every** run
(X13-SM-01). snp reproduces this: `snp_run::try_repo` creates a `TempDir` (removed on every exit path) and runs against
`Store::ephemeral(tmp, Some(&main_store))`: `repos-v1`, `keys-v1`, `configs-v1`, `patches/` and the global lock live
in the temp dir; `tools-v1` and `cache-v1` (uv, node, rustup, go toolchains; probe facts) come from the main store —
invisible, because toolchain downloads are snp's own and print nothing. The generated temp config is never tracked;
the user's `-c` config is tracked in the main store as for every non-`COMMANDS_NO_GIT` command (parity with
`main.py`). Acceptance: two consecutive try-repo runs both print the `Initializing` line and the `Installing` triplet
(T10-TRY rows), and the main store gains no `repos-v1` entry.

---

## 6. Languages (`snp-lang`)

### 6.1 Trait, registry, lifecycle

`Language` (§1.4.10) is implemented by 21 zero-sized backends behind `backend(id) -> &'static dyn Language`
(static match). Lifecycle (03 §8, 04a §1.3), orchestrated by `snp-repo`: resolve version (`language_version` →
`default_language_version` → `default_version()` probe, memoised per process by `ToolProbe`; persisted in
`cache-v1/probes` only when the resolved executable is a native binary that is not a known proxy — pyenv/asdf/rbenv/
nodenv shims are `#!` scripts, mise and rustup proxies and macOS `/usr/bin/*` xcrun shims are listed — keyed by its
realpath + `(len, mtime_ns, ino)` and a digest of the steering env `PATH`, `PYENV_VERSION`, `ASDF_*`, `MISE_*`,
`RUSTUP_TOOLCHAIN`, `DEVELOPER_DIR`, `VIRTUAL_ENV`, `CONDA_PREFIX`, ADR-079; so `pyenv local 3.13` is never answered
from a stale fact) → env-less guards → `check_support` (before any FS mutation) → installed? →
install → health → `activate()` (pure `EnvPatch`, cached per `InstallKey`) → `plan()` → executor. Backends never spawn
hooks; they spawn install/probe commands only through `CmdRunner` (= `normalize_cmd` against the *child* env +
`capture`, CalledProcessError format A with the normalised argv on failure).

### 6.2 Per-language plan (all 21 ship in v1; WP = work package)

| Language (WP) | env dir | Versions (default) | Install & bootstrap | Activation (EnvPatch) | Health / notes |
|---|---|---|---|---|---|
| python (24) | `py_env` | Any; default = `python{X.Y}` of the reference interpreter (`python3` then `python` on PATH; Windows `py -3`, `python`), facts cached by exe fingerprint (native interpreters only, §6.1) | Installer per env (04a §11.4 step 1, ADR-073): `SNP_PYTHON_INSTALLER=auto` (default) \| `uv` \| `pip`, recorded in `InstallState.installer` — `pip` is the escape hatch for uv-vs-pip resolver differences (pre-releases, yanked versions, extra-index strategy, 04a §11.3 G7); `uv` forces uv and fails instead of falling back. **uv** (ADR-033): `$SNP_UV` → sibling of snp → PATH uv ≥ 0.8.0 (flags probed once) → pinned download (`UV_PINNED`, sha256 embedded per target). `uv venv <env> --python <abs interp> --seed --no-project --no-config --no-python-downloads`, `uv pip install --python <env py> --no-config [translated pip config] . *deps` (cwd prefix, installer env). Untranslatable pip config or non-allow-listed `-`-prefixed deps ⇒ pip backend (`<interp> -m venv`, `-m pip install . *deps`). Missing explicitly-requested interpreter ⇒ uv-managed download (`SNP_PYTHON_DOWNLOADS=never` disables; D-CRASH-19). Local hook without deps skips the placeholder `pip install .` (invisible) | `PIP_DISABLE_PIP_VERSION_CHECK=1`, `PYTHONHOME`⊘, `VIRTUAL_ENV`, PATH `bin`/`Scripts`; strip `UV_PYTHON*`, `UV_PROJECT*`, `UV_CONSTRAINT`, `UV_OVERRIDE`, `UV_RESOLUTION`, `UV_CONFIG_FILE` (04a §11.5) | fingerprint fast path, else pre-commit's `version_info` probe accepting uv's pyvenv.cfg format; uv cache `cache-v1/uv` unless `UV_CACHE_DIR` |
| node (24) | `node_env` | Any; `system` iff `exe_exists(node) ∧ exe_exists(npm)` (non-Windows, `$HOME`/shims heuristic kept), else `default` = `index.json[0]` (latest Current), `lts`, `X.Y.Z`; nodeenv `.node-version`/`[nodeenv]` quirk replicated (04a Q9); explicit `system` on Windows allowed (D-CRASH-20) | nodejs.org (or `SNP_NODE_MIRROR`) archive verified against `SHASUMS256.txt` (TLS), musl/riscv64 → unofficial-builds, arm64→x64 retry; shared `tools-v1/node/<ver>` linked into the env; `npm install --allow-git=root --install-links -g <pkgs>` (4.6.2 rule; empty ⇒ AssertionError text) | `NODE_VIRTUAL_ENV`, `NPM_CONFIG_PREFIX`/`npm_config_prefix`, `NPM_CONFIG_USERCONFIG`⊘, `NODE_PATH`, PATH | `node --version` (stamp-cached); Windows `.cmd` shims → batch budget + Rust quoting (D-PLAT-03) |
| golang (24) | `golangenv` | Any; `system` iff `exe_exists(go)`; `default` = go.dev JSON `[0]` | go.dev archive verified by the JSON sha256 (404 ⇒ pre-commit's ValueError text) → `<env>/.go`; `go install ./...` + each dep with installer env + `GOPATH=<env>`, `GOBIN`⊘, `GOTOOLCHAIN=local`; remove `<env>/pkg` (read-only-safe) | managed: `GOROOT`, `GOTOOLCHAIN=local`, PATH; system: PATH | none; GOPATH not set at run time (parity) |
| rust (24) | `rustenv` | Any; `system` iff `cargo --version` (cwd `/`) exits 0; `default` = stable | rustup-init **binary** verified by `.sha256`, persistent `tools-v1/rustup` (`--profile minimal`); lib deps `cargo add` in the (per-deps) prefix; `cargo install --bins --root <env>` per package (`cli:` deps in config order) | PATH `<env>/bin`, `RUSTUP_TOOLCHAIN` (managed) | none |
| ruby (24) | `rbenv` | Any; `system` iff `ruby` ∧ `gem`; else embedded rbenv/ruby-build/ruby-download tarballs (sha256 embedded) | `rbenv download` → `rbenv install` fallback, `rehash`; `gem build` (sorted `*.gemspec`) + `gem install --no-document --no-format-executable --no-user-install --install-dir … --bindir …` | `GEM_HOME`, `GEM_PATH`⊘, `BUNDLE_IGNORE_CONFIG=1`, `RBENV_ROOT`/`RBENV_VERSION`, PATH | Windows: system only (parity) |
| docker (25) | `docker` | DefaultOnly | `docker build --pull --tag <tag> --label PRE_COMMIT .`, tag = `pre-commit-<md5(repo-key hex)>` (ADR-035; never the `src` basename, which would collide across repos); per run `docker build` (pull=False) is skipped when `docker image inspect <tag>` succeeds (invisible; `SNP_DOCKER_ALWAYS_BUILD=1`) | none | rootless probe failure ⇒ "not rootless" (D-CRASH-21); mountinfo parser fuzzed |
| docker_image (16) | — | n/a | n/a | none | `docker run --rm [--tty] [-u uid:gid] -v <cwd>:/src:rw,Z --workdir /src` + entry |
| conda (25) | `conda` | DefaultOnly | `conda`/`mamba`/`micromamba` (`PRE_COMMIT_USE_MAMBA`/`MICROMAMBA` non-empty) `env create -p <env> --file environment.yml`, one `install -p` with deps | `PYTHONHOME`⊘, `VIRTUAL_ENV`⊘, `CONDA_PREFIX`, PATH (+ Windows dirs) | — |
| coursier (25) | `coursier` | DefaultOnly | `cs`/`coursier` found on the unpatched PATH; channel apps (sorted) then deps, under activation | PATH `<env>`, `COURSIER_CACHE` | empty channel dir: env created (no crash) |
| dart (25) | `dartenv` | DefaultOnly | `dart pub get` + `dart compile exe` per `executables` key (names validated as single components); per-dep `pub cache add` in a temp `PUB_CACHE` | PATH `<env>/bin` | — |
| dotnet (25) | `dotnetenv` | DefaultOnly, no deps | `dotnet pack` → sorted nupkgs → nuspec id (zip ≤ 1 MiB entry, `roxmltree`) → `dotnet tool install` with a cleared `nuget.config` | PATH `<env>/bin` | — |
| haskell (25) | `hs_env` | DefaultOnly | `cabal update`; `cabal install --install-method copy --installdir <env>/bin` (sorted `*.cabal` + deps; none ⇒ FatalError text) | PATH | — |
| julia (25) | `juliaenv` | Ignored (parity) | copy (Julia)Project/Manifest, run JULIA_CODE byte for byte with `--startup-file=no` | `JULIA_LOAD_PATH`, `JULIA_PROJECT`⊘, `JULIA_DEPOT_PATH` | entry script prefix-joined unless local |
| lua (25) | `lua_env` | DefaultOnly | `luarocks --tree <env> make` (sorted rockspecs), one `install` per dep | PATH, `LUA_PATH`, `LUA_CPATH` (one memoised `luarocks config --lua-ver`) | luarocks needed at run time (parity) |
| perl (25) | `perl_env` | DefaultOnly | `cpan -T . *deps` under activation | PATH, `PERL5LIB`, `PERL_MB_OPT`, `PERL_MM_OPT` (shlex.quote) | — |
| r (25) | `renv` | DefaultOnly | renv copy + the two scripts (prefix as an escaped R string literal), `r.version`, `renv::install` deps | `R_PROFILE_USER`, `RENV_PROJECT`⊘ | health: 2 Rscript runs only when the `(Rscript path, size, mtime)` stamp changed; entry validation texts verbatim |
| swift (25) | `swift_env` | DefaultOnly, no deps | `swift build --package-path <prefix> -c release --build-path <env>/.build` (cwd root) | PATH `<env>/.build/release` | Unix only (parity) |
| fail (16) | — | n/a | n/a | — | in-process `RunPlan::Fail` |
| pygrep (16) | — | n/a | n/a | — | in-process `RunPlan::Pygrep` |
| unsupported / `system` (16) | — | n/a | n/a | — | `run_xargs` with PATH resolution + shebang emulation |
| unsupported_script / `script` (16) | — | n/a | n/a | — | `prefix.path(cmd[0])` (local ⇒ root) |

`exe_exists`, `assert_version_default`, `assert_no_additional_deps`, the CalledProcessError format, readonly-aware
`rmtree` and the local-template bytes are framework helpers (`snp_lang::common`).

### 6.3 Toolchain bootstrap and verification (snp-net, ADR-034)

| Tool | Source | Verification |
|---|---|---|
| uv | GitHub releases, pinned `UV_PINNED` | sha256 **embedded** per target (trust root = the snp release) |
| node | nodejs.org/dist (mirror override) | `SHASUMS256.txt` over TLS |
| go | go.dev/dl `?mode=json&include=all` | per-file sha256 from the JSON (TLS) |
| rustup-init | static.rust-lang.org | `.sha256` sidecar (TLS); toolchains then verified by rustup itself |
| rbenv / ruby-build | embedded archives (pre-commit's vendored versions) | sha256 embedded |
| CPython | via uv (python-build-standalone) | uv verifies; only when an explicitly requested version is missing |

Flow: `tools-v1/<tool>.lock` → `tools::resolve` → `Fetch::get_verified` into `tools-v1/<tool>/.dl` → safe extract into
`<version>.part-<rand>` (two-pass safe extraction, §1.4.9) → marker → rename (Windows: `rename_with_retry`).
Proxies, `SSL_CERT_FILE/DIR`, platform verifier; `SNP_OFFLINE=1` fails clearly; `http://` mirrors need
`SNP_ALLOW_INSECURE_MIRROR=1` and even then serve only archive bytes — the checksum manifest always comes over TLS from
the canonical origin or an explicit https `SNP_NODE_SHASUMS_URL`/`SNP_GO_JSON_URL` (ADR-077); a checksum mismatch fails
closed naming URL, expected and actual digests.

### 6.4 `language_version` validation, Windows and macOS

The §5.4 naming makes traversal impossible; versions reach tools only as single argv elements (Python paths go to
`uv --python`/`-p` after `expanduser`). Windows: case-insensitive env keys, dir-major `PATHEXT` lookup (own
`find_executable`, never `which`, ADR-014), batch-file argument refusal (Rust's BatBadBut quoting) rendered as
``Executable `{cmd0}` cannot receive an argument containing a line break: {repr}`` rc 1 (D-PLAT-03), Git-for-Windows
basename fallback for `#!/bin/sh`-style interpreters (D-PLAT-02), CPU count via `available_parallelism` (differs only
above 64 logical CPUs, D-PLAT-10). macOS: GUI clients' launchd PATH ⇒ the embedded `SNP_EXE` in hook scripts is
mandatory and must be a stable path (§9.7, 11 §A.19.3: a PATH entry such as `/opt/homebrew/bin/snp`, never a versioned
Cellar realpath); `/usr/bin/git` shim resolved once per process (14 A.7.10; its version is not persisted, §6.1).

---

## 7. Native hooks (`snp-hooks`) and the pre-commit-hooks fast path

### 7.1 Always-native: `fail`, `pygrep`, `meta`, `hazmat`

* **fail** — `RunPlan::Fail`: no process; output `f'{entry}\n\n' + '\n'.join(files) + '\n'`, rc 1, one call with all
  files (no partitioning, 04a §8.3).
* **pygrep** — `RunPlan::Pygrep`: per **xargs partition** the argv `[*args, entry, *files]` — exactly what
  `python -m pre_commit.languages.pygrep` receives — is parsed by `snp-argparse` with pygrep's spec (prog `pygrep.py`,
  CPython 3.13 texts, abbreviations, `--multiline=1` error), so a tracked file named `-i` or `-x.py` behaves as in
  Python (flag / `unrecognized arguments`, exit 2; ADR-072); `Mode::Bytes` regex (`-i` ⇒ ASCII folding, `--multiline`
  ⇒ `MULTILINE|DOTALL`), the four matching procedures of 04a §12.4 byte for byte (unshuffled, serial partitions); an
  unreadable file stops the rest of that partition with the Python last-line text (04a Q8); the pattern is compiled
  once per hook for the `args + [entry]` parse (§3.11) and recompiled only for a partition whose file names change the
  parse. No Python fallback, ever.
* **meta** — §3.11.
* **hazmat** (ADR-006): `entry: pre-commit hazmat …` / `snp hazmat …` is rewritten to `[self_exe, "__hazmat", …]`
  (path H): a real child process per partition, prog `hazmat.py`, CPython-3.13 argparse texts, exit 0/1/2/child, `n1`
  OR (`-N & 0xFF`), exec failures print only Python's final traceback line (exit 1), SIGINT: wait 250 ms then SIGKILL the
  child and die by SIGINT. Direct CLI `{prog} hazmat …` (path C): envelope, `ExecutableNotFoundError` exit 3, usage
  exit 2. `__hazmat` is dispatched on raw `argv[1]` before parsing under both brands (D-MISC-02).

### 7.2 Fast path for `pre-commit/pre-commit-hooks` (09 §6; ON by default, FD-8, ADR-031)

**Detection tiers.** `is_pre_commit_hooks_url` (https/ssh/scp, `.git`, trailing `/`, case-insensitive GitHub path).
*Tier A*: exact known tag or the full commit of a known tag → embedded per-tag manifest and epoch profile; **no clone,
no venv, no network** when every selected hook of the repo is native. *Tier B*: any other rev or URL → the normal
clone (blobless `--filter=blob:none` allowed), and `git ls-tree` blob ids of the module and its import closure must all
be in `VERIFIED_BLOBS` and the manifest's `(entry, language)` must match the console-script table; else DELEGATE.

**Gating (all must hold, 09 §6.4).** G1 enabled (`--no-fast-path` / `SNP_NO_FAST_PATH` / root `fast_path: off` /
`fast_path_exclude`); G2 profile resolved; G3 class allowed by the mode — `auto` (default) = NATIVE hooks plus
NATIVE-WITH-CAVEATS hooks whose CI parity gate (`snp_hooks_data::parity_gate`) has passed; `exact` = NATIVE only; G4
neither the user's hook config nor a local-overlay hook patch sets `language`, `entry` or non-empty
`additional_dependencies` (`ConfigHook.raw` keys ∪ `HookPatch::keys()`); G5 `preflight(args)` ok (argparse *errors*
are emulated, not delegated); G6 epoch ≥ v3.0.0 and not a DELEGATE epoch.

**Order of decisions (ADR-070).** All six gates are decided inside `all_hooks`: G1–G4 and G6 from data, G5 through
`ResolveCtx::preflight` (implemented by snp-run over `snp_hooks::native(..).preflight`, since snp-repo cannot depend on
snp-hooks). A hook that delegates gets its repo cloned (a Tier A repo is cloned at that point, printing nothing extra:
its `Initializing` line was already decided by the keys marker), a real prefix and its python `InstallKey`, all inside
`all_hooks`. Therefore the delegate set is known **before** `install_envs`, and a delegated hook's clone/venv installs
at step 14 like any other hook (transcript parity). `snp install-hooks --with-fallback` additionally provisions delegate environments of
native hooks (offline CI that may later set `SNP_NO_FAST_PATH`).

**Cold-run transcript (ADR-032).** For every fast-path repo, `[INFO] Initializing environment for {repo}.` prints
exactly when pre-commit would print it (keys marker, §5.3), and the `Installing environment` triplet prints exactly
once for the python environment pre-commit would have built (a *virtual* `InstallKey` with a `keys-v1` `Installed`
marker), even though nothing is installed. Warm and cold transcripts are therefore byte-identical to pre-commit.

**Execution.** Native hooks run once per xargs partition with pre-commit's shuffle and partition sizes; effects are
committed in input order and the first crash stops the partition (Python semantics); writes use `rewrite_in_place`
(inode-preserving, never temp+rename); git facts use the identical argv and hook env (no `-z`; quotePath and renames
honoured — bug-compat, 09 §0 #5); per-hook `Epoch` selects behaviour and display names come from the per-tag manifest;
modifications are reported precisely (§3.7). `fast_path_coalesce: true` (root extension) runs one call per hook.

**Observability:** `SNP_LOG=debug` logs every decision; `snp doctor --fast-path` prints the decision table; output is
otherwise byte-identical (`-v` included). **Safety:** Tier A executes zero network bytes; Tier B reads only metadata
from the clone; a moved upstream tag cannot inject code.

### 7.3 `repo: builtin`

`- repo: builtin` with no `rev` (error if present). Keys as `repo: local` except `entry` (forbidden), `language`
(absent or `system`), `additional_dependencies` (forbidden) — validated by cfgv `MapItem`s in snp-config's config
schema, so `validate-config` and `run` agree and errors carry cfgv contexts (ADR-070). Defaults from `BUILTIN_MANIFEST_YAML` (= pre-commit-hooks
v6.0.0 for shared ids). Semantics "upstream-compatible, bugs fixed" (09 §7.2) plus `--check` flags. Unknown id →
``{id} is not present in repository builtin.  Typo? …``. pre-commit rejects these configs (documented, extension-only);
`snp migrate builtin` / `snp migrate upstream` convert both ways. snp-only builtin ids (09 §7.3) are v1.1.

### 7.4 Implementation inventory (09 §1)

| Package | Hooks |
|---|---|
| WP-26 (P0 + framework) | `trailing-whitespace`, `end-of-file-fixer`, `mixed-line-ending`, `fix-byte-order-marker`, `check-merge-conflict`, `detect-private-key`, `check-added-large-files`, `no-commit-to-branch`, `check-json`, `check-executables-have-shebangs`; the two `language: fail` hooks (manifest only); removed/stub hooks (`check-byte-order-marker` and `fix-encoding-pragma` stubs from v5.0.0, v3.x `autopep8-wrapper`/`flake8`/`pyflakes` stubs); pygrep, fail, hazmat, `repo: builtin` |
| WP-27 (P1/P2 + caveats) | `pretty-format-json`, `file-contents-sorter`, `requirements-txt-fixer`, `sort-simple-yaml`, `check-case-conflict`, `check-shebang-scripts-are-executable`, `check-symlinks`, `destroyed-symlinks`, `forbid-new-submodules`, `name-tests-test`, `check-vcs-permalinks`, the pre-v5.0.0 real `check-byte-order-marker`/`fix-encoding-pragma`; caveat hooks `check-yaml`, `check-toml`, `check-xml`, `detect-aws-credentials` behind their parity gates |
| always DELEGATE | `check-ast`, `check-builtin-literals`, `check-docstring-first`, `debug-statements`, `double-quote-string-fixer` (need CPython's `ast`/`tokenize`) |

---
## 8. Config extensions (lefthook-inspired), local overlay, `snp migrate lefthook`

The full key reference (types, defaults, validation, examples) is `docs/COMPATIBILITY.md` §7; this section fixes the
mechanics.

### 8.1 Key policy (05 §18.1, ADR-039)

* **P1 plain keys**, prek names/semantics where prek defined them (`priority`, `priorities`, `groups`, `env`).
* **P2 unknown keys, including `x-*`,** produce pre-commit's byte-identical `[WARNING] Unexpected key(s) …` lines in
  snp too (parity; prek silently drops `x-*`, snp does not).
* **P3** never retype a pre-commit key (prek's `files: {glob: …}` is rejected with pre-commit's error, 06 Q2).
* Under pre-commit, extension keys cost one `Unexpected key(s)` warning per hook at run time (root/repo keys at load)
  and change nothing else. In snp, recognised extension keys produce no warning (D-EXT-01) and take effect.
  `SNP_COMPAT_STRICT=1` makes snp treat them exactly as pre-commit does (unknown keys, no overlay, `SNP=0` ignored) —
  the switch the differential harness uses.
* Extensions are **brand-independent**: the same commit behaves the same whether git runs snp via an snp script or a
  `pre-commit` alias; only CLI surface and texts change with `argv[0]`.
* Validation: extension items are cfgv `MapItem`s appended after pre-commit's items (§1.5); values are compiled at load
  (regexes, globs (`globset`, `*` crosses `/`), Go durations, conditions) and reported as cfgv-style errors with
  pre-commit context lines plus snp leaves (exit 1).

### 8.2 Keys (summary)

| Level | Key | Semantics |
|---|---|---|
| hook | `priority` (u32 or alias) | lower first; equal ⇒ same bucket ⇒ concurrent (§3.4); implicit = flattened index |
| root | `priorities` (map name→u32) | aliases for `priority` |
| hook | `env` (map str→str) | literal values (no `$VAR` expansion, prek parity); applied after language activation (user wins); names validated (no `=`, NUL) |
| hook | `groups` (list) | prek semantics (ADR-080): names non-empty, no whitespace, no leading `@` (reserved for selectors); `--group G` (repeatable, OR), `--require-group G` (repeatable, AND), `--no-group G` (exclusion wins), the selector `@ungrouped` (hooks without groups) in all three; `SNP_SKIP_GROUPS`/overlay `skip_groups` (exclusion) |
| hook | `stage_fixed` (`true` = `verify` \| `trust` \| `false`) | §8.3 |
| hook, root | `skip` / `only` (condition or list) | `merge`, `rebase`, `merge-commit`, `cherry-pick`, `revert`, `bisect`, `{ref: glob}`, `{env: NAME}`, `{ci: bool}`, `{run: cmd}` (sh -c, 30 s, cached per run); keys in one map AND, list items OR; skipped hooks print the yellow `Skipped` line |
| hook | `timeout` (Go duration) | whole-hook wall clock; isolated process group; TERM → 2 s → KILL; `Failed` + `- hook timed out after <d>`; counts for fail_fast |
| hook | `fail_text` (str) | `- <fail_text>` appended to a failure block |
| hook | `root` (relative dir) | cwd = `<repo>/<root>`, files filtered by boundary-aware prefix and passed relative to root; missing ⇒ `Failed` + `- root directory '<r>' does not exist` |
| hook | `use_stdin` (bool) | gets a copy of the git hook's stdin (cap 64 MiB) instead of an empty pipe |
| hook, root | `fail_on_changes` (`always` \| `never` \| `ci` \| `non-ci`) | `ci` = `CI` non-empty and not `0`/`false` |
| root | `minimum_snp_version` (semver) | `snp {v} is required but snp {SNP_VERSION} is installed.` exit 1 |
| root | `allow_disable` (bool, default true) | `false` ⇒ `SNP=0` is ignored (stderr note) |
| root | `fast_path` (`off`\|`exact`\|`auto`), `fast_path_exclude` (ids), `fast_path_coalesce` (bool) | §7.2 |
| root | `output` (`default`\|`summary`\|`quiet`) | `summary`: footer with wall time, per-hook durations, counts (stdout, after the last hook); `quiet`: failures only. Default output unchanged |

`interactive` and `follow` (COULD) are reserved names that validate but are rejected with "not available in this
version" until v1.1.

### 8.3 `stage_fixed` (fix → verify → stage, 05 §18.4 A4)

Active only for the `pre-commit` stage on the staged file set with a stash (otherwise ignored with a debug note).
Per hook H with staged inputs I: `M` = files in I changed by H (stat + blake3), `O` = other changed tracked files.
`O ≠ ∅` ⇒ normal failure. `M = ∅` ⇒ normal result. `M ≠ ∅ ∧ rc = 0` ⇒ accepted. `M ≠ ∅ ∧ rc ≠ 0` ⇒ `verify`: re-run
H on I once; passes (rc 0, no new changes) ⇒ accepted, else Failed with the second output (`trust` skips the re-run).
Accepted ⇒ status `Passed` plus `- files were modified by this hook and staged`; staging happens once after all hooks
via `StashGuard::stage_fixed` (apply-check first, §4.5); files also modified by a non-`stage_fixed` hook are never
staged. `fail_on_changes` combines (e.g. stage locally, fail in CI).

### 8.4 Local overlay (05 §18.4 A3)

`<config dir>/.pre-commit-config.local.yaml` (or `.yml`), or `SNP_LOCAL_CONFIG=<path>` (empty/`0` disables),
`--no-local-config`. **Ignored** (one stderr line) when it is **tracked by git** (it is per-developer by definition) or
when `CI` is truthy, unless named explicitly by `SNP_LOCAL_CONFIG`. Schema: `hooks: {<alias|id>: {skip, entry
("{entry}" = merged base entry), args (replace), env (merge, overlay wins), priority, stage_fixed, …any hook key except
id/alias/repo}}`, `skip_groups`, `fail_on_changes`, `repos` (appended). Unknown ids → stderr warning
`snp: local config overrides unknown hook '<id>'`. When active, one stderr line `snp: using local overrides from
{path}` (never stdout); `snp config dump` marks overridden fields. pre-commit never reads it.

**Mechanics (ADR-070).** The overlay is split: `overlay::apply_root` applies `skip_groups`, `fail_on_changes` and the
appended `repos` to the `Config` at step 10; hook entries stay data (`HookPatch`) passed through `ResolveCtx::overlay`
and applied by `merge_hook`/`finalize_local` **after** manifest ∪ config, so `{entry}` expands against the merged entry
of remote and builtin hooks too, and the patched `HookDef` is what fast-path G4 (patch keys count as user keys), the
InstallKey (patched `language`/`additional_dependencies`) and the executor see. `Overlay::unmatched` produces the
unknown-id warnings after `all_hooks`.

### 8.5 `snp migrate lefthook`

Implements 05 §19 (CLI, pipeline, field mapping, bucket scheduling from `parallel`/`piped`/`group`, `run`→`entry` rules
with argv-safe `sh -c '…' --` wrappers passing data as `"$@"`, gobwas glob → Python regex validated by
`snp_pyre::validate`, `file_types` → identify tags, `--local` overlay conversion, git-lfs detection and re-chaining as a
legacy hook, report, `--strict` exit 2 on lossy features, `--portable` emits only pre-commit keys). lefthook YAML is
read with `snp-yaml` `Schema::Yaml12Core`, TOML with `toml`, JSON/JSONC with `serde_json` + comment stripping. Output is
validated by `snp-config` and, in CI (WP-35), executed by the 4.6.2 oracle (portable: zero errors; snp mode: only
`Unexpected key(s)` warnings).

---

## 9. CLI

### 9.1 Parser strategy (ADR-010)

A table-driven argparse-compatible engine (`snp-argparse`, ~1.2 kLOC, no deps) reproduces argparse's acceptance,
precedence (unknown options deferred to the top level; ambiguity at consumption; end-of-parse env-default conversion),
REMAINDER handling, negative numbers, short clusters and the HelpFormatter/textwrap layout of CPython ≥ 3.13.14.
**clap is not linked into the binary.** `xtask completions` builds a `clap::Command` from the same `ParserSpec`
tables to emit bash/zsh/fish/powershell completions and man pages (embedded with `include_str!`; `snp completions
<shell>` prints them); a unit test asserts both option sets are equal.

### 9.2 Invocation and `argv[0]` (ADR-023)

`Brand::detect(args_os().next())` before parsing (never `current_exe()`). `argv[1] == "__hazmat"` →
`snp_hooks::hazmat::in_hook_main` with no parser, no store, no logging (both brands). Empty argv → `run`. **Compat
brand**: exactly pre-commit's 16 subcommands and options (choices lists, help, abbreviations byte-identical); snp-only
commands/flags are **not** accepted. **snp brand**: the same surface plus the extensions below, guarded by
`prefix_stable()` so every abbreviation valid for pre-commit keeps its meaning (`--files-from-stdin` was rejected
because it would make `--fi`/`--fil`/`--file` ambiguous; the flag is `--stdin-files`; `--config-hooks` was rejected
because `--con`/`--conf`/`--confi` are unique abbreviations of `install`/`uninstall`'s `-c/--config` today; the flag
is `--git-config-hooks`, ADR-057). A planted-option fixture keeps both rejected names in the prefix-stability test.

### 9.3 Commands and flags

| Command | Flags (pre-commit 4.6.2, 12 §A.8.1) | snp-brand extensions |
|---|---|---|
| `run [hook]` | `--color`, `-c/--config`, `-v/--verbose`, `-a/--all-files` ⟂ `--files [F…]`, `--show-diff-on-failure`, `--fail-fast`, `--hook-stage`, `--remote-branch`, `--local-branch`, `-s/--source/--from-ref`, `-o/--origin/--to-ref`, `--pre-rebase-upstream`, `--pre-rebase-branch`, `--commit-msg-filename`, `--prepare-commit-message-source`, `--commit-object-name`, `--remote-name`, `--remote-url`, `--checkout-type`, `--is-squash-merge`, `--rewrite-command` | several positional hook ids (literal ids/aliases, `:` allowed, §1.6), `-j/--jobs N`, `--group G`*, `--require-group G`*, `--no-group G`*, `--no-fast-path`, `--stdin-files[=nul\|lines]` (⟂ `-a`/`--files`), `--summary`, `--no-local-config` |
| `install` | `--color -c -f/--overwrite --install-hooks -t/--hook-type* --allow-missing-config` | `--git-config-hooks` (git ≥ 2.54; ADR-057) |
| `uninstall` | `--color -c -t*` | `--git-config-hooks` |
| `install-hooks` | `--color -c` | `--with-fallback` |
| `init-templatedir DIR` | `--color -c --no-allow-missing-config -t*` | — |
| `hook-impl` (hidden from the list, present in choices) | `--color -c --hook-type --hook-dir --skip-on-missing-config rest…` | — |
| `autoupdate` | `--color -c --bleeding-edge --freeze --repo* -j/--jobs` | — |
| `clean`, `gc`, `sample-config` | `--color` | — |
| `try-repo REPO [hook]` | `--color -c --ref/--rev` + all run options | — |
| `validate-config [F…]`, `validate-manifest [F…]` | `--color` | `--strict` (unknown hook keys reported early), `--portable` (list extension keys pre-commit would warn about) |
| `migrate-config` | `--color -c` | — |
| `hazmat {cd SUBDIR,ignore-exit-code,n1} CMD… --` | `--color` | — |
| `help [cmd]` | — | — |
| `list` | — | `--json`, `--hook-stage`, `--group`, `--require-group`, `--no-group`, `-c` |
| `config dump`, `config schema` | — | merged config incl. overlay (`--json`); JSON schema |
| `cache {dir,size,gc,clean}` | — | store inspection |
| `migrate {lefthook,builtin,upstream}` | — | 05 §19, 09 §7.1 |
| `doctor [--fix] [--fast-path] [--stash]` | — | 11 §A.15 |
| `self {alias install\|uninstall\|status, check}` | — | `check` = embedded vector self-test + capability probes |
| `completions SHELL` | — | from build-time generated scripts |

`*` = repeatable (append). Environment variables (`SNP_*`) work in both brands.

### 9.4 Help, usage, errors, `--version`

Help/usage/error text is golden-tested against the canonical oracle `pc462-py313` at `COLUMNS=80` and the width set of
12 §A.16. Usage errors: stderr, exit 2, the owning parser's usage block, except unrecognized arguments → top-level
usage + `{prog}: error: unrecognized arguments: …` (ADR-008). `PRE_COMMIT_COLOR` is converted at end of parse only
when `--color` was absent (ADR-007). `--version`/`-V`: compat brand prints exactly `pre-commit 4.6.2`; snp brand prints
`snp 2.0.0` (identical layout, own product name; an explicit row of the brand mapping, since substituting the product
name alone would give `snp 4.6.2`). "Which tool is this?" is answered by `snp doctor`, `snp self check`,
`snp self alias status` and the crash log. Width = `COLUMNS` (Python `int`) → stdout terminal width → 80, minus 2.

### 9.5 Branded strings

Compat brand: every byte as pre-commit 4.6.2 — including `pip install --upgrade pre-commit` in the min-version text and
pre-commit's own bug-report URL in the migration-mode text — except what the separate store forces (store paths, the
crash-log body) and the registered divergences. snp brand: the `brand_map` substitutions (§9.6).

### 9.6 snp-brand texts: argparse output and `brand_map` (ADR-058)

**argparse texts are not substituted.** Usage, help and usage-error blocks are wrapped by argparse's HelpFormatter
relative to `usage: {prog} {cmd} `; with `snp` the first line is 7 characters shorter, so wrap points and continuation
indentation change (e.g. `init-templatedir`'s continuation indent moves from 35 to 28 columns). The snp brand's
argparse texts are therefore **rendered** by snp-argparse from `SNP_SPEC` with prog `snp`, and golden-tested against
two independent CPython 3.13.14 references produced by `xtask cli-goldens`: (a) the pc462-py313 oracle with argparse
`prog` patched to `snp` (a launcher that rewrites `prog='pre-commit'` before `main()` builds its parser) for every
parser whose snp-brand spec equals the compat spec, and (b) a real `argparse.ArgumentParser` built from
`snp_argparse::export_json(SNP_SPEC, "snp")` for every parser that carries snp-only options or commands (D-EXT-07).
The difftest's snp-brand runs use the prog-patched oracle (`Runner::oracle_snp_prog`) so argparse blocks inside run
transcripts (usage errors) compare exactly; snp-only parsers compare against the committed goldens.

**`brand_map`** (`tests/differential/brand_map.yaml`) applies to every **non-argparse** text: substitute `pre-commit` →
`snp` only where the string names the tool or a command to type (11 §A.8.2 B2): `--version` (`pre-commit 4.6.2` →
`snp 2.0.0`, a whole-line rule); `{prog} installed at`; `Use -f to use only {prog}.`; ``Skipping `{prog}`.``; missing-config block lines
3–4; show-diff banner lines 2–3; hints `` `{prog} autoupdate` ``, `` `{prog} migrate-config` ``, ``{prog} autoupdate
--repo``; `{prog} normalizes …`, `{prog} failed to diff`, `for now, {prog} requires/does not support …`, `{prog}
requires system-installed "cs" …`, `{prog} has detected a mix of cygwin…`, `Locking {prog} directory`,
`Try upgrading identify and pre-commit?` → `Try upgrading snp?`, the min-version sentence →
`pre-commit version {required} is required but snp {SNP_VERSION} is compatible with pre-commit 4.6.2.  Perhaps upgrade
snp.`, the migration-mode text → `bug: snp's script is installed in migration mode` / `` run `snp install -f
--hook-type {T}` to fix this `` / `Please report this bug at https://github.com/grok-rs/snp/issues`, crash-log name.
**Never** substituted: file names (`.pre-commit-config.yaml`, `.pre-commit-hooks.yaml`), env var names,
`pre-commit hook(s) made changes.`, `Your pre-commit configuration is unstaged.`, `https://pre-commit.com/…` URLs,
cfgv object names. The difftest applies `brand_map` to the (prog-patched) oracle output and then requires byte
equality, so the snp brand is tested as exactly as the compat brand.

### 9.7 Hook script (installed by `install`/`init-templatedir`; ADR-048)

11 §A.6 template, POSIX `sh`, LF on every OS, written atomically (replaces symlinks instead of writing through them):

```sh
#!/bin/sh
# File generated by snp: https://github.com/grok-rs/snp
# ID: 5bb9bd89cf6c92c58fee7677ceaf4fc3
# pre-commit compat ID: 138fd403232d2ddd5efb44317e38bf03
# prek compat ID: 182c10f181da4464a3eec51b83331688

HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

# start templated
SNP_EXE=/home/u/.cargo/bin/snp
SNP_HOOK_TYPE=pre-commit
set -- hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit --hook-dir "$HERE" -- "$@"
# end templated

if [ -n "${SNP_BIN:-}" ] && [ -x "$SNP_BIN" ]; then
    exec "$SNP_BIN" "$@"
elif [ -x "$SNP_EXE" ]; then
    exec "$SNP_EXE" "$@"
elif command -v snp > /dev/null 2>&1; then
    exec snp "$@"
elif command -v pre-commit > /dev/null 2>&1; then
    exec pre-commit "$@"
fi
case "${SNP:-}" in
    0|false)
        # kill switch with no snp binary: still run the chained legacy hook (git-lfs), never fail the commit
        shift 6   # rendered: the number of templated tokens before git's original arguments
        if [ -x "$HERE/$SNP_HOOK_TYPE.legacy" ]; then exec "$HERE/$SNP_HOOK_TYPE.legacy" "$@"; fi
        exit 0;;
esac
echo '`snp` not found.  Did you forget to activate your virtualenv?' 1>&2
exit 1
```

**Which executable is embedded (ADR-060, 11 §A.19.3–§A.19.4).** `SNP_EXE` = `snp_cmd::scripts::select_exe(brand,
arg0, env, current_exe)`: candidates are `absolute(arg0)` when `arg0` contains a path separator, then every absolute
PATH entry joined with the brand's own name (`snp[.exe]` in the snp brand, `pre-commit[.exe]` in the compat brand),
then `current_exe()`; the first that is a file and the same file as `current_exe()` wins and is **never
canonicalised** (Linux `/proc/self/exe` resolves to versioned paths — Linuxbrew Cellar, mise, cargo-binstall dirs —
that vanish on upgrade, after which GUI clients whose PATH lacks `snp` would fail closed on every commit). So a
`pre-commit install` through the alias embeds the alias path and every commit keeps the compat brand (hook-time output
byte-identical to pre-commit's, K3); an `snp install` embeds an `snp` path. `is_snp` (same file, an equal-length copy
with the same BLAKE3, or `--version` matching `pre-commit X (snp Y)`) decides whether an existing `SNP_EXE` still points
at snp (`doctor`). Test: install through a symlinked bin dir, delete the real versioned path, commit → the hook still
runs.

**Messages when replacing other managers' scripts (ADR-060, 11 §A.5.4).** Classification is
`snp_git::hookscripts::classify` (11 §A.5.1). pre-commit's own stdout lines (`{prog} installed at …`, `Running in
migration mode …`, `Use -f to use only {prog}.`, `{prog} uninstalled`, `Restored previous hooks to …`) are unchanged.
The snp-specific notes M1 (`Replaced hook script from {owner} at {hook_path}` [+ ` (Python: {INSTALL_PYTHON})`]), M1b,
M2 (`Removed {legacy_path}: it is a hook script from {owner}, not a user hook`), M3 (``Note: {hook_path} still runs
{owner}; run `{prog} install -t {T}` to switch it to snp``) and M12 go to **stderr** in both brands (D-DIAG-02), so a
compat-brand `pre-commit install` over Python pre-commit's script is byte-identical on stdout (pre-commit overwrites
its own scripts in place silently). prek and snp 1.x scripts are manager scripts: replaced in place where pre-commit
would move them to `.legacy` and print the migration-mode lines (D-BRAND-02); a `.legacy` that is a manager script is
deleted (D-DATA-03).

Rendering: the templated `set --` line is `hook-impl`, `--config=<cfg>`, `--hook-type=<T>`, optional
`--skip-on-missing-config`, then the fixed tail `--hook-dir "$HERE" -- "$@"` — exactly pre-commit's `ARGS` order, so the
`pre-commit` fallback accepts it; the `shift` count in the kill-switch branch is rendered to match the number of
templated tokens. Every token and `SNP_EXE` go through `shlex.quote` (bytes on Unix; forward slashes on Windows). The
script never checks `SNP` when a binary exists: `hook-impl` runs the legacy hook first and then honours the kill switch
(ADR-038). The `# … ID:` lines are load-bearing (pre-commit's and prek's `is_our_script`); a template change bumps the
snp ID and moves the old one into `SNP_PRIOR_IDS`. `snp install` treats pre-commit's `CURRENT_HASH` and 5
`PRIOR_HASHES`, prek's ID and the snp 1.x marker as manager scripts and overwrites them in place (never `.legacy`); a
`.legacy` that is itself a manager script is deleted; a foreign `.legacy` is never clobbered without `-f`
(D-DATA-03). Hook scripts are written with `atomic_replace` (Windows: `rename_with_retry`, ADR-068). git ≥ 2.54 config
hooks: `snp install --git-config-hooks` writes `hook.snp-<type>.{command,event}` with the `select_exe` path and a
trailing ` --`.

**`run` hint (11 §A.14, D-DIAG-03).** `snp_run::run` (never `hook-impl`) prints, on stderr only, when `SNP_NO_HINTS` is
unset/empty, stderr is a terminal, `core.hooksPath` is unset and some `T` of the config's `default_install_hook_types`
has `<common-dir>/hooks/T` classified `PreCommit`, `Prek` or `SnpV1`: ``hint: {hook_path} runs {owner}, not snp; run
`{prog} install` to switch`` — once per such `T`, right after the config is loaded (before the first hook line). It
uses `snp_git::hookscripts::classify` (snp-run has no snp-cmd edge).

---

## 10. Errors, exit codes, crash log, signals, logging

### 10.1 `main` and the top-level guard (ADR-015)

```rust
// crates/snp/src/lib.rs
pub fn entry() -> Exit {
    let argv: Vec<OsString> = std::env::args_os().collect();          // the only argv read
    std::panic::set_hook(Box::new(record_panic));                    // records + cancels; never prints, never locks (ADR-066)
    if argv.get(1).is_some_and(|a| a == "__hazmat") {                 // no store, no logging, no parser (03 H.8.3)
        return hazmat_exit(catch_unwind(|| snp_hooks::hazmat::in_hook_main(&argv[2..], &Env::capture_process())));
    }
    let brand = Brand::detect(argv.first().map(OsString::as_os_str));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| dispatch(brand, argv)));
    let failure = match outcome { Ok(Ok(code)) => return exit(code), Ok(Err(f)) => f,
                                  Err(_) => Failure::Exc { kind: ExcKind::Unexpected, exc: PyExc::panic(take_panic_record()) } };
    report(&failure, brand)                                          // infallible: never panics, bounded allocation
}
```

`record_panic` (the panic hook) runs on the panicking thread, possibly while it holds the Printer, registry or other
locks, so it only stores payload, location and backtrace into a `try_lock`-guarded slot (dropped if busy) and calls
`CancelToken::cancel(Panic)` (lock-free state). It never prints, never touches the Printer or the child registry and
never restores; the restore runs only on the main thread (explicit `restore()` or `RunState::drop`), and other threads
only read shared state (ADR-066). `dispatch` owns the stash/store guards through `snp-run`; by the time `report` runs
every guard has been dropped, so restore messages precede the crash banner (13 X.1). `exit(code)` maps 0–255 to
`Exit::Code` and anything larger (Windows hook-impl `legacy | run`) to `Exit::Raw`, which `main` passes to
`process::exit` after every destructor of `entry` ran (14 A.7.4). SIGTERM/SIGHUP termination re-raises the signal after
restore (§4.3). `dispatch` sequences `install` + `install --install-hooks` and `snp migrate lefthook --install`
(§1.4.22–§1.4.23). (`&argv[2..]` above is illustrative; real code uses `.get(2..)`.)

### 10.2 Exit-code map (13 X.6)

| Code | Meaning | Producer |
|---|---|---|
| 0 | success / all skipped / nothing to push | — |
| 1 | hook failed or modified files; FatalError, InvalidConfigError, InvalidManifestError; `SystemExit` texts (hook-impl argc, hazmat, migration mode); C-cfg and C-hook conversions (13 X.3) | snp-run, snp-config, snp-cmd |
| 2 | usage error (argparse engine) | snp-argparse |
| 3 | unexpected error (K/K+ rows: git, IO, installs, network, AssertionError, CalledProcessError, PanicException) | all |
| 120 | stdout closed (EPIPE) — fail closed, never 0, silent | snp-core Printer |
| 130 | SIGINT / console Ctrl-C, after restore (`Interrupted (^C): KeyboardInterrupt: ` + log line) | snp-proc signals |
| 143 / 129 | SIGTERM / SIGHUP: death by the re-raised signal after restore (no banner) | snp-proc signals |
| `legacy\|run` | hook-impl | snp-run |
| 101 | **never** (the Rust panic default is caught) | — |

### 10.3 Envelope and crash log (ADR-013)

* Line 1 on stdout: `An error has occurred: {ty}: {text}` (exit 1) / `An unexpected error has occurred: {ty}: {text}`
  (exit 3) / `Interrupted (^C): KeyboardInterrupt: ` (130). It continues a dangling `name....` segment on the same line
  (parity). `{ty}` is the unqualified Python class (`CalledProcessError`, `FileNotFoundError`,
  `ExecutableNotFoundError`, …) from `pyerr::os_error_class` and the `PyExc` constructors (13 X.8 #1).
* Line 2: `Check the log at {store}/{pre-commit.log|snp.log}`, or `Failed to write to log at …` + body on stdout.
  Then the optional K+ hint on **stderr** (ADR-045).
* Crash log (overwritten, `O_NOFOLLOW`, never contains environment variables), pre-commit's skeleton:
  ````
  ### version information

  ```
  pre-commit version: 4.6.2            (snp brand: "snp version: 2.0.0")
  git --version: git version 2.43.0
  sys.version:
      snp 2.0.0 (rustc 1.99.0) [x86_64-unknown-linux-gnu]
  sys.executable: /home/u/.cargo/bin/snp
  os.name: posix
  sys.platform: linux
  ```

  ### error information

  ```
  An unexpected error has occurred: CalledProcessError: …
  ```

  ```
  <error source chain, one per line; Rust backtrace for PanicException; panic hint naming https://github.com/grok-rs/snp/issues>
  ```
  ````
  The body below the version block is Tier B by design (pre-commit prints a Python traceback there).
* The reporter never creates the store unless it must write the log; it never calls into config/git; every write is
  infallible from the caller's view.

### 10.4 Signals (ADR-026, ADR-065)

Unix: one `signal-hook` iterator thread over `SignalsInfo<WithOrigin>` for SIGINT/SIGTERM/SIGHUP (caught handlers →
children get default dispositions after exec; SIGPIPE is reset to default in children by std). Windows: `ctrlc`
(`termination` feature). This table is **normative** (ADR-065) and is the posix signal-test matrix of §13.6:

| Received | Origin | Inherit-group Hook/Tool children | Isolated Hook children (`timeout`) | Critical children | snp |
|---|---|---|---|---|---|
| SIGINT | Kernel (terminal ^C: si_code SI_KERNEL / si_pid 0) | already delivered by the kernel: not re-sent | SIGINT to `-pgid` | never signalled | cancel(Interrupt) |
| SIGINT | Sent (kill(2)/sigqueue: SI_USER/SI_QUEUE) | SIGINT to each pid | SIGINT to `-pgid` | never | cancel(Interrupt) |
| SIGTERM / SIGHUP | Kernel (tty hangup) | already delivered: not re-sent (no double signal, which many tools treat as "force quit") | same signal to `-pgid` | never | cancel(Terminate/Hangup) |
| SIGTERM / SIGHUP | Sent | same signal to each pid | same signal to `-pgid` | never | cancel(Terminate/Hangup) |
| any | Unknown origin | as Sent for TERM/HUP, as Kernel for INT | as above | never | as above |
| grace elapsed (`SNP_KILL_GRACE`, 2 s) | — | SIGKILL each pid still alive | SIGKILL `-pgid` | never | — |
| cancel path, before restore | — | `quiesce`: Linux (subreaper) SIGKILL + reap every remaining descendant; Windows: terminate the run's hook Job Object; macOS: none reachable (documented residual risk) | already gone | never | — |
| 2nd/3rd signal while a `RestoreShield` is alive | any | — | — | never | one stderr notice; restore continues |
| Windows CTRL_C / CTRL_BREAK | console | delivered by the console to Inherit children | not delivered (new process group); terminated with the job after the grace | not delivered (no console) | cancel(Interrupt) |
| Windows CTRL_CLOSE / LOGOFF / SHUTDOWN | console | — | — | not delivered | process ends when the handler returns: journal recovery (§4.6) |

Sequence: (1) token cancelled with the reason; (2) forwarding per the table; (3) the pool hands out no new permits, no
new hook starts, and reader waits end after kill + 500 ms drain (§3.6); (4) escalation after the grace (pid-reuse-safe);
(5) the main thread returns `Failure::Interrupted`; `quiesce` kills leftover descendants; the stash guard restores
under a `RestoreShield` with its Critical git children in an isolated group, never signalled; (6) report and exit
(SIGINT 130; SIGTERM/SIGHUP re-raised). `__hazmat` children: 250 ms then SIGKILL, then die by the same signal
(03 H.8.7).

### 10.5 Logging (`tracing`)

Developer diagnostics only: off by default (no subscriber → ~0 cost), enabled by `SNP_LOG=<filter>` (`RUST_LOG` is
ignored so users' Rust hooks are unaffected), written to **stderr** or `SNP_LOG_FILE`; never stdout, never into hook
output. Spans: `run`, `resolve`, `clone{repo}`, `install{key}`, `hook{id}`, `partition{i}`, `git{argv}`,
`stash{state}`, `fastpath{id}`. Environment dumps are allow-list based (§2.5): only known-safe names show values;
every other variable is `NAME=<redacted, N bytes>`; URL userinfo is scrubbed from argv. User-facing `[INFO]/[WARNING]/[ERROR]` lines are **not** tracing
events; they go through `Printer`.

---

## 11. Security model

### 11.1 Trust boundaries

| Input | Trust | Consequence |
|---|---|---|
| The repository's `.pre-commit-config.yaml` (+ overlay) | trusted *code* (entries execute) but untrusted *data* for snp's own invariants | must not make snp write outside its dirs, hang the parser, or exhaust memory |
| Remote hook repos at the pinned rev | trusted for execution (pre-commit's model); manifests untrusted as data | `language_version` traversal impossible (§5.4); manifests parsed with the same limits; Tier A runs no remote code |
| File names / git output | untrusted data | never interpolated into shells or format strings; bytes end to end; argv only |
| Network | untrusted | TLS + checksum before use; size caps; no `http://` unless `SNP_ALLOW_INSECURE_MIRROR=1` |
| The store | semi-trusted (other local users) | 0700, world-writable refused, root refuses a store owned by another uid unless `SNP_ALLOW_FOREIGN_STORE=1`, fd-relative deletion (no symlink following, no swap race), markers validated (schema version, containment) |
| Environment | trusted but leaky | owned-repo/installer env sanitisation (§2.5), `UV_*` strip list, allow-list env logging (values of unknown variables never logged) |

### 11.2 Subprocess environment policy

Hooks receive the full environment (parity) minus `__PYVENV_LAUNCHER__`/`PYTHONEXECUTABLE` plus pre-commit's variables.
snp-owned operations use allow-lists: git in owned repos = pre-commit's `no_git_env` allow-list (`GIT_EXEC_PATH`,
`GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_SSL_CAINFO`, `GIT_SSL_NO_VERIFY`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`,
`GIT_CONFIG_VALUE_*`, `GIT_HTTP_PROXY_AUTHMETHOD`, `GIT_ALLOW_PROTOCOL`, `GIT_ASKPASS`; ASCII-case-insensitive on
Windows) + fsmonitor injection (ADR-018); installers = `no_git_env(hook_env)` (fixes worktree `GIT_INDEX_FILE`
corruption, prek #2505; D-SEC-05); uv: strip list of 04a §11.5. Docker containers see only `-e` passed values.

### 11.3 No shell interpolation

`hook_cmd` = Python `shlex.split` port; spawn by argv; hook scripts render every embedded token with `shlex.quote` and
are property-tested by `sh -c 'printf %s "$1"'` round-trips; migrated lefthook wrappers pass data as `"$@"` after `--`;
`skip/only {run: …}` conditions are the only `sh -c` snp executes itself, with config-authored strings (trusted code)
and no file names.

### 11.4 Resource bounds (R-STACK and caps)

| Resource | Bound |
|---|---|
| Recursion on input | none unbounded: YAML composer/constructor iterative or depth-capped (512); regex parser explicit depth ≤ 484; JSON/YAML checkers iterative with CPython thresholds; `cfgv::Value` drop safe because depth ≤ 512 |
| YAML size / nodes | 16 MiB file, 1 M constructed nodes (alias bombs), arena ids (no copies during compose) |
| Regex | Python repeat bound; tier-1 `size_limit` 64 MiB (compile acceptance) and `dfa_size_limit` = the crate default 2 MiB — it is the lazy-DFA cache capacity **per regex per thread**, not an acceptance limit, so 64 MiB would allow threads × patterns × 64 MiB; a thrashing cache falls back to the linear engines with identical results; tier 2 `delegate_size_limit` 64 MiB, `delegate_dfa_size_limit` 2 MiB, `backtrack_limit` 1e8 → error, never a hang; emitted tier-2 nesting < 64 or `Unsupported` (ADR-079; supersedes the 01 Addendum §A.6 `dfa_size_limit(64<<20)` constants) |
| Hook output | one global 64 MiB in-memory budget, then spill to an unnamed file |
| pre-push / `use_stdin` | 64 MiB; more fails closed with an exact FatalError (exit 1), never truncated (§3.10, D-SEC-12) |
| File descriptors | pool clamped to `(RLIMIT_NOFILE soft − 64) / 8`; the limit is never raised (children inherit it) |
| Shebang line | 64 KiB (file-identify); text sniff 1 KiB |
| Downloads | per-tool max size (512 MiB), 30 s connect / 60 s idle, 3 retries |
| Threads / processes | `JobPool` permits (cgroup-aware default); `Limiter`s (compile-heavy installs exclusive); one signal thread; one watchdog thread; one reader thread per running capture; every thread created through `Builder` (refusal → error) |

### 11.5 Filesystem safety

Atomic temp+rename writes for everything snp owns (hook scripts replace symlinks instead of writing through them,
D-DATA-05; Windows renames retried on sharing violations, ADR-068); in-place rewrites only for user files edited by
native fixers (preserve inode/mode/hardlinks, 09 §9.8); `remove_tree_within` (fd-relative on Unix) for every deletion;
archive extraction in two passes rejects absolute/`..`/drive entries, devices/FIFOs, hardlinks outside the tree and
symlinks that resolve outside it or through another symlink, and strips setuid bits (ADR-077); the crash log is opened
`O_NOFOLLOW`; `.legacy` hooks are never clobbered unless they are manager scripts or `-f` (D-DATA-03); patches and
journals are `create_new` 0600; spill files are unnamed.

### 11.6 `unsafe` policy and platform FFI

`forbid(unsafe_code)` in all 26 crates without exception (FD-5). Platform needs are met by audited safe APIs in
dependencies: `rustix` (pty with CLOEXEC, termios, `access`, `waitid`, `kill`, `sched_getaffinity`, `statfs`, `poll`),
`signal-hook` (incl. `emulate_default_handler`) / `ctrlc`, `winapi-util` (VT on the stderr console), `process-wrap`
(Windows Job Objects), std `CommandExt::{process_group, creation_flags}`, `std::fs::File::lock`. The 14 §A.7.1 `snp-sys`
FFI crate is rejected; its uses are replaced (colour: `winapi-util`, exact parity; CPU count: `available_parallelism`,
divergent only above 64 logical CPUs, D-PLAT-10; junction detection: not possible, D-PLAT-12).

### 11.7 Supply chain

`cargo-deny` (advisories with `yanked = deny`, bans §1.3, license allow-list, sources = crates.io + the two path
crates), `cargo audit` nightly, committed `Cargo.lock` + `--locked` everywhere, minimum versions `rustls ≥ 0.23.45`,
`tar ≥ 0.4.46`, no build scripts in snp crates, `cargo auditable` release builds, SBOM (cargo-cyclonedx), GitHub
artifact attestations, checksums for every release asset, dependency budget ≤ 120 packages in the release graph.
`libyaml-safer` (single maintainer) is wrapped by `snp-yaml`'s event adapter, fuzzed, and fork-ready via
`[patch.crates-io]`.

---

## 12. Performance and memory plan

### 12.1 Budgets (release `dist` profile, warm caches; baselines from 10 §0.2; ADR-043)

| ID | Scenario | pre-commit 4.6.2 | Linux x86_64 (gate) | macOS arm64 | Windows x64 |
|---|---|---|---|---|---|
| B1 | `--version` | 172 ms | **≤ 3 ms (hard)** | ≤ 5 ms | ≤ 15 ms |
| B2 | `run`, 1 local `true` hook (`always_run`, no filenames) | 234 ms | ≤ 10 ms (hard) | ≤ 15 ms | ≤ 0.4× pre-commit |
| B3 | `run`, 10 such hooks, nothing staged | 243 ms | **≤ 25 ms (hard)** | ≤ 40 ms | ≤ 0.5× pre-commit (each `git diff` is a real spawn, 10–30 ms on Windows, 02 G.4) |
| B4 | `run -a`, 1 hook with filenames, 50 k files | — | ≥ 5× faster (soft) | same | same |
| B5 | B4 + `types`/`types_or`/`exclude_types` | — | ≤ 1.3× B4 | | |
| B6 | 13 pre-commit-hooks hooks, 960 files, fast path, warm | ≈ 1.5 s | ≤ 60 ms (soft) | | |
| B7 | B6 cold (fresh store, network excluded) | tens of s | ≤ 1 s | | |
| B8 | stash with a 10 MB unstaged patch | — | ≤ pre-commit, RSS + ≤ 2 MB (streamed patch) | | |
| B9 | `git commit` through the installed hook | — | ≤ B3 + 3 ms | | |
| B10 | `validate-config` with 1 000 hooks | — | ≤ 50 ms | | |
| B11 | `gc` with 500 repos / 200 configs | — | ≤ 1 s | | |
| B12 | peak RSS, B4 (50 k) / B6 | — | **≤ 30 MB / ≤ 25 MB (hard)** | | |

Never slower than pre-commit or prek at p50 on any scenario (musl artifacts included, ADR-079). B3 spawn model on
Linux (ADR-082): 1 combined `rev-parse` + ≤ 6 overlapped fan-out queries (A, B, C, E, F, G; E is not taken when no hook
reads files, P17) + the stash's own `write-tree` and `diff-index` (pre-commit's G08/G09, 02 §7) + 10 hooks + 0 `git
diff` (baseline elided by diff-index rc 0 on the warm path; DiffOracle stat path) ⇒ ≤ 18 spawns for B3 (8 git), ≈ 13 on
the critical path (≈ 1 ms each measured); B2 (one hook, no Stat mode below three process hooks): 7 git + 1 hook ≤ 8.

### 12.2 Techniques (each owned by a crate; every one gated by an equivalence test, S13)

| # | Technique | Crate |
|---|---|---|
| T1 | Static argparse tables, no clap, `__hazmat`/`--version` before any init, no thread for `--version` | cli, snp |
| T2 | Lazy store (zero syscalls until written), lazy tracing, lazy regex engines (validation = parse only) | store, pyre |
| T3 | One combined `rev-parse`; concurrent read-only git fan-out consumed in pre-commit order; cached `git --version` | git, run |
| T4 | Speculative config load during the fan-out, accepted only when the config has no unstaged change (ADR-047) | run, config |
| T5 | DiffOracle stat path + racy hashing (Unix); baseline elision on `diff-index` rc 0 | stash, run |
| T6 | Streaming blake3 of `git diff` and of the stash patch (O(1) memory) | git, stash |
| T7 | Arena paths + `u32` ids; shuffle on ids; `TagSet` bitsets | files, pycompat |
| T8 | Three-tier lazy tagging driven by filter needs; git-mode zero-syscall path; P17 classifier skip | files, run |
| T9 | Regex fast kinds (`''`, `^$`, prefix/suffix/contains), tier-1 DFA, mandatory tier-2 prefilter, pattern cache | pyre |
| T10 | One global `JobPool`, inline single partitions, scoped threads, separate Net/Install limiters | proc, run |
| T11 | posix_spawn-eligible `Command`s (no `pre_exec`), merged single pipe, pty only with colour | proc |
| T12 | Activation env computed once per `InstallKey`; exe resolution cached per `(PATH, PATHEXT, name)` | lang, repo |
| T13 | Spawn-free health checks (fingerprints); probe facts cached in the store for native binaries only (§6.1) | lang, store |
| T14 | Parallel clones (prefetch) and installs with ordered transcript replay; uv; shared toolchains; local deps-variant clones | repo, lang |
| T15 | Native pygrep/fail/meta/hazmat-free paths; pre-commit-hooks fast path (no clone/venv/Python per partition) | hooks, repo |
| T16 | Global output budget + spill; head-of-line printing | proc, run |
| T17 | Release profile: fat LTO, cgu=1, system allocator, `panic=unwind` (+4.9 % size, runtime noise, 07 §4.3) | workspace |

### 12.3 Measurement and gates

`cargo xtask bench setup|run|gate` (WP-36): hyperfine `-N -w 5 -r 50` against the oracle (`pc462-py312`) and pinned
prek, JSON history, PR comment; gate fails on a > 5 % median regression with non-overlapping 95 % CIs or any hard-budget
breach. RSS via `/usr/bin/time -v` (median of 10). Spawn budgets: integration tests run against a `--features testing`
build of `snp` with `SNP_TEST_SPAWN_LOG` (`snp_proc::spawn_log`, feature `testing`) and assert upper bounds per
scenario (catches accidental O(files) spawns); timing gates use the `dist` build. Micro-benchmark thresholds stated in
package acceptance lists are enforced in two places: a release-mode `#[test] #[ignore = "perf"]` per threshold with a
3× safety factor run by the `perf-smoke` CI job (W1/W2, catches gross regressions), and the exact targets by `cargo
xtask bench gate` (WP-36). The bench matrix includes the musl artifacts (system allocator vs mimalloc, ADR-079) and a
B12 case with 32 threads × 20 patterns on non-ASCII paths (regex cache memory). Syscalls: nightly Linux `strace -f
-c` on B1/B3 with committed hard ceilings (B1 ≤ 64 syscalls; B3 ≤ 4230 over the whole process tree, ADR-083).
Allocations: bench feature `dhat-heap` (B3 ≤ 2 MB total allocated). Criterion micro-benches: pyre kinds vs engines on 100 k paths, shuffle + partition on 50 k,
tagging tiers, YAML load of the pre-commit-hooks manifest, argparse parse of a hook-impl argv, `len_cjk`.

---

## 13. Testing architecture

### 13.1 Layers (10 §7.1, made concrete)

| Layer | What | When |
|---|---|---|
| L0 static | fmt, clippy (deny set), cargo-deny, `xtask check-dag`, `xtask check-unsafe`, `xtask lint-guards`, typos, machete | every push |
| L1 unit | per crate; committed CPython vectors (pycompat, repr, shlex, paths, MT19937, EAW, duration, argparse, YAML resolver, regex accept/match) — no Python at test time | every push |
| L2 integration | `crates/snp/tests/it/*.rs` (one test binary), `snp-testkit::TestEnv` hermetic env, insta snapshots with filters; every catalog row of 10 §5, 13 X.4 and 11 §A.20 is a test named with its namespaced ID (§13.8) | every push |
| L3 differential | `snp-difftest` vs pre-commit 4.6.2 (`pc462-py312` runtime oracle, `pc462-py313` CLI-text oracle, both built hash-locked by `xtask oracle install`) in **both brands** | curated on push, generated nightly |
| L4 property/fuzz | proptest (256 cases push / 10 000 nightly), cargo-fuzz targets with 256 KiB stacks and 512 MB RSS | push (short) / nightly |
| L5 toolchain/network | `tc_<lang>_*`, `net_*` (nextest default-filter excludes) | nightly / label |
| L6 platform | Linux x64/arm64 (gnu + musl smoke), macOS arm64 + x64, Windows x64/arm64 × git 2.18.5 (warn; stash refused, D-DATA-17; `--all-files` fallback, D-CRASH-26 — both `expect_divergence` scenarios), 2.30 (warn; fallback), 2.31, 2.34, 2.39, 2.43, 2.54 (config hooks), latest, Git for Windows latest | PR |
| L7 performance | §12.3 | nightly + PR comment |
| L8 fault injection | snp-stash FI matrix, kill-9 crash matrix for store/install/stash, signal matrix, EPIPE, ENOSPC (tmpfs with a size limit), concurrency matrix | every push (posix) |

Rules: no test mutates the process env or cwd; every spawned process gets the hermetic env (10 §7.3.2); tests run with
and without `PRE_COMMIT_NO_CONCURRENCY=1`; binaries are referenced by `CARGO_BIN_EXE_*` inside their package and by
`snp_testkit::Bins` elsewhere, never through PATH (a stale `~/.cargo/bin/snp` 1.x exists on dev machines);
fault-injection and spawn-log tests run against the `--features testing` build.

### 13.2 Differential harness (`snp-difftest`, ADR-054)

Scenario YAML (10 §7.4.3 + `brands: [compat, snp]`, `expect_divergence: D-…`) materialised at the **same absolute
root** for both tools (identical paths/SHAs; fixed `GIT_*_DATE`; `taskset -c 0-3` so CPU counts agree); snp runs
through a `pre-commit`-named symlink (compat brand) and as `snp` (against the prog-patched oracle, with `brand_map`
applied to non-argparse text, §9.6), with and without `SNP_COMPAT_STRICT=1` for extension scenarios. Compared: stdout/stderr/exit (Tier A), worktree bytes + modes,
index (`ls-files -s`), refs, stash list, config file bytes, logical store effects. Normalisation tiers: **A** exact;
**B** semantic (crash-log body, Python tracebacks → last line, usage/help vs the py312 oracle with the 12 §A.4.1
deltas, localized `[WinError]` text, YAML error bodies only where a registered gap exists); **C** ignored (durations,
patch names, store roots — the stores differ by design — installer chatter between `Installing environment` lines,
partition boundaries within bytes of ARG_MAX). Every divergence must be listed in
`tests/differential/divergences/<ID>.yaml` (one file per register ID, owned by the implementing package, WBS); the
run fails on unlisted diffs **and** on listed diffs that no longer reproduce. stderr-only snp diagnostics (class DIAG)
are compared with their D-DIAG scenario and otherwise stripped by a documented Tier-C rule only on stderr. Shrinking: proptest over the scenario model, then delta debugging; each failure writes a `repro.sh`.

### 13.3 Golden, vector and snapshot tests

`cargo xtask oracle vectors|goldens` regenerates committed JSON vectors and golden texts from the oracles (CI needs no
Python): pycompat (3 256+ cases, 15 §A1), PyYAML typing/quoting/emitter (500 scalars × 3 styles, 112 dump lines), YAML
error corpus, regex accept/reject + `search` truth tables (01 Addendum §A.8), argparse outcomes (~170 argv × env) and
help at the 12 §A.16 widths, pre-commit-hooks per-epoch hook goldens, EAW over all code points, MT19937 permutations.
insta inline snapshots ≤ 30 lines, file snapshots otherwise, `INSTA_UPDATE=no` in CI. `xtask catalog-status` (WP-35)
reports ported-vs-total catalog IDs from `tests/catalog/index.tsv` by scanning test names (§13.8) — 100 % of hermetic
rows before 1.0.

### 13.4 Property tests (selection)

shuffle == committed CPython vectors for all n ≤ 2000 step 37; `partition` never drops/duplicates a file and never
exceeds the budget; `shlex.split(quote(s)) == [s]`; Python-re acceptance and match vs committed truth tables (corpus +
grammar-generated); tiered tags filter outcome == full identification; DiffOracle == real `git diff` (10 000
randomized iterations); `render(events)` deterministic and `cols − 1` wide; YAML 1.1 resolver vs PyYAML table;
`revline::rewrite` idempotent and EOL-preserving; `env.patched` == `envcontext`; JobPool never exceeds permits; argparse
prefix stability; `--jobs 1` vs `--jobs 32` byte equality.

### 13.5 Fuzz targets (no panic, no hang > 5 s, ≤ 512 MB, 256 KiB thread stacks)

Every target is `fuzz/fuzz_targets/<crate-or-group>_<name>.rs` (WP-00 creates the stub file and the `[[bin]]` entry;
the package owning the prefix fills it): `pycompat_shlex_split`, `pycompat_decode_utf8`, `pycompat_rsplit4`,
`pycompat_ntpath`, `pycompat_rev_line` (WP-11); `argparse_parse` (both specs, WP-12); `pyre_compile_match` (str +
bytes, WP-13); `yaml_compose_construct`, `yaml_load` (WP-14); `proc_hookcmd_split`, `proc_shebang_resolve` (WP-15);
`net_archive_extract` (WP-16); `pyparsers_json`, `pyparsers_yamlcheck`, `pyparsers_toml`, `pyparsers_xml` (WP-17);
`git_z_split`, `git_pre_push`, `git_merge_msg`, `git_hook_script_classify` (WP-18); `config_load_validate`, `config_manifest_load`,
`config_migrate`, `config_revline` (WP-21); `files_classify`, `store_env_dir_name`, `store_marker_parse` (WP-22);
`stash_journal_parse` (WP-23); `lang_python_pyvenv_cfg` (WP-24); `lang_docker_mountinfo` (WP-25); `pygrep_match` and
`pch_p0_<hook>` (WP-26); `pch_p1_<hook>`, `pch_p2_<hook>`, `pch_caveat_<hook>` (WP-27; every native hook on arbitrary
bytes); `cmd_script_args` (era parsers of 11 §A.5.3, WP-33); `migrate_lefthook` (WP-34).

### 13.6 Fault-injection and crash matrices

* **FI-01…FI-24** (§4.7) via `ScriptedBackend`.
* **Kill-9 matrix:** SIGKILL snp after the k-th syscall-visible step (clone, marker write, env install, state commit,
  patch write, journal write, checkout, staging-fixed add) for k = 1…n; assert no store entry is ever *believed*
  complete when it is not, the next run self-heals, and the worktree is recoverable from patch + journal + pointer with
  the printed hint.
* **Signal matrix** = the §10.4 table: every (signal × origin × child group) row, during stash, during a hook (pipe and
  pty mode, with and without `timeout`), during restore, during install; assert restore, exit status, that a Critical
  `git apply` was never signalled (including a hook that exits at once followed by a slow apply across the 2 s grace),
  that kernel-origin signals are not re-sent, that a sent SIGINT reaches Inherit hooks, that a daemonised descendant
  holding stdout cannot delay the restore beyond grace + drain, and (Linux) that orphaned descendants are dead before
  the restore. Windows: CTRL_C/CTRL_BREAK rows and the console-close recovery row.
* **Panic injection:** `snp_core::fault` (`SNP_FAULT=PrinterWrite`, `WorkerPartition`, `StashState@n`, …) panics inside
  a worker that holds the Printer lock → stash restored, exit 3, banner printed after the restore messages, no hang (S14).
* **fd hygiene:** N concurrent partitions where one child sleeps and daemonises; the others complete promptly — pipe
  mode and 16 colour-mode (pty) partitions on Linux and macOS (S15); a run under `ulimit -n 64` completes (pool clamp).
* **Disk-full:** tmpfs with 1 MiB headroom for store and patches; fail closed before mutation.
* **Concurrency:** N parallel `snp install-hooks` on one store; `gc` concurrent with `run`; two `git commit`s in one
  worktree; nested `snp run` inside a hook; `check-hooks-apply` with `PRE_COMMIT_NO_CONCURRENCY=1` (pool re-entrancy).
* **Windows filesystem:** a test holds an open handle (no `FILE_SHARE_DELETE`) on a file inside a materialising repo,
  an install marker and a hook script for 300 ms; `materialize`, `commit_install_marker`, `retire_dir` and `snp install`
  succeed through `rename_with_retry`.
* **SEC-01…SEC-14:** `language_version` traversal (`../../x`, `/abs`, `C:\x`, NUL), hostile manifest ids/entries,
  archive entries (absolute, `..`, symlink escape, setuid), file names starting with `-` / containing newlines / `$()`
  through every argv path (incl. pygrep and native hooks: a file named `-i` / `-x.py`, ADR-072), hook-script rendering
  of hostile config paths, world-writable/foreign store (root + foreign owner refused), symlinked store sub-dirs, a
  directory swapped for a symlink during `remove_tree_within`, crash-log symlink, overlay path tricks, `SNP_HOME`
  pointing at pre-commit's store, archive chains (`x -> .`, `y -> x/..`), hardlink and device entries, an `http://`
  mirror with a tampered SHASUMS file, pre-push stdin over 64 MiB, spill files absent after kill -9, env secrets absent
  from `SNP_LOG=trace` output.

### 13.7 CI matrix

Toolchain 1.99.0 (pinned) + MSRV job 1.98.0 (`cargo check --workspace --all-targets --locked`); OS: `ubuntu-24.04`,
`ubuntu-24.04-arm`, `macos-15` (arm64), `macos-15-intel`, `windows-2025`, `windows-11-arm`; musl static build smoke;
git versions per §13.1 L6 via containers; nextest profiles `default` (hermetic), `ci` (retries 2, JUnit), `nightly`
(all). Coverage (llvm-cov) reported; mutation testing (`cargo mutants`) nightly on snp-stash, snp-proc::xargs,
snp-argparse, snp-pyre. Every workflow first checks out the user crates as siblings at the `deps.lock` SHAs (ADR-061);
a scheduled drift job compares those SHAs with the `snp-support` branch heads and opens an issue when they differ.
`perf-smoke` (release-mode `#[ignore = "perf"]` tests) runs on push.

### 13.8 Test-catalog IDs (ADR-062)

The research catalogs reuse prefixes (`CFG-01` is a clientlib type-tag row in 10 §5.1 but the
`minimum_pre_commit_version` crash row in 13 X.4; `MIG-01…09` are migrate-config rows in 10 §5.12 while `MIG-01…24`
are install-interop scenarios in 11 §A.20). In snp documents, test names and the index, every catalog ID carries its
source: **`T10-<FAM>-nn`** (10 §5: CFG, VAL, RUN, SFO, XRG, INS, ITD, HKI, GIT, STO, REP, AUP, MIG, TRY, GC, MET, OUT,
ERR, CLI, HAZ, UTL), **`X13-<FAM>-nn`** (13 X.4: CFG, ST, GIT, CLS, EXE, STASH, ENV, HI, HZ, CMD, EH, SM) and
**`M11-MIG-nn`** (11 §A.20). Tests are named `t10_cfg_01_<slug>` / `x13_eh_03_<slug>` / `m11_mig_07_<slug>`.
`tests/catalog/index.tsv` (generated once by WP-00 from the research documents; columns: ID, source section, title,
owner WP, hermetic flag, test file) lists every row with the owner assigned in WBS.md ("Catalog ownership");
`xtask catalog-status` (WP-35) joins it with the test names found in the tree. An unprefixed ID in a research document
keeps its meaning inside that document.
