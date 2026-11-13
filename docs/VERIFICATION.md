# Formal verification (ADR-095)

*Status (2026-11-13): ADR-095 accepted. M0 (pipeline, guards, CI, docs) is in place; M1 has started: K1 is verified, K2–K4 are in progress (§9.1). M2 and M3 are planned, not started. Owners: WP-38 "Formal verification" owns `proofs/` and the specs; WP-00 owns the root manifest, xtask (`xtask/src/tasks/proofs.rs`) and CI. Package owners review the kernel files in their crates.*

Where this document and the code disagree, the code and `proofs/registry.toml` are what CI checks; fix whichever is wrong in the same PR.

This document fixes how snp uses machine-checked proofs:

- **Verus** proves properties of the shipped Rust source.
- **Lean 4** proves properties of models: transliterations of pre-commit's Python, and protocol models.
- **Bridges** keep each model honestly linked to the code it describes.

It is the reference for contributors who edit a kernel file and for reviewers of proof PRs.

---

## 0. What is claimed, and what is not

**What a kernel is.** A kernel is a pure, std-only function in a production module that the shipped `snp` binary calls directly. For each kernel listed in §9, Verus proves the following for **every** input:

- **Totality.** The function terminates and never panics, overflows or indexes out of bounds.
- **Functional correctness.** Its result equals a functional specification (the *spec*) written over mathematical sequences and integers.
- **Named properties.** The spec has the properties listed for it, such as uniqueness, no data loss, idempotence and containment.

**What Lean proves.** Lean proves theorems about *models*. A model is either a line-by-line transliteration of the upstream Python, proved equal to a declarative definition, or a protocol model. The link from a Lean model to shipped code is **tested** (oracle differential tests). It is never claimed as proved.

**Wording rule** (README, CHANGELOG, docs, PRs):
- Write "kernel X is proved to satisfy spec Y (ADR-095)" or "model theorem Z".
- Never write "snp is formally verified".
- Every kernel row in §9 lists what it guarantees (G) next to what it does not (¬G).

**Not claimed:**
- whole-program equivalence with pre-commit;
- I/O, filesystem or git semantics, except as named hypotheses of a model;
- concurrency;
- anything about the regex engines, YAML, argparse, the Unicode tables or ntpath;
- the behaviour of the trusted items in §10.

---

## 1. Architecture at a glance

```
 CPython / pre-commit-hooks ──(nightly differential, py_vs_lean)──────────────► Lean transliteration pyX
                                                                                   │  theorem pyX = X
 shipped kernel (crates/…/kernel file) ═══ the same file ═══► Verus root            │  (model level, proved)
        │                                       │ proves exec == Verus spec          ▼
        │                                       ▼ (code level, proved)            Lean spec X
        └──(per PR: bounded-exhaustive + seeded random)──► Lean oracle executable (runs the proved pyX)
```

The design has three layers:

1. **Code level (Verus).** Contracts sit in place in production files and add zero dependencies (§2). One standalone `verus` run over `proofs/verus/kernels.rs` verifies every kernel.
2. **Model level (Lean 4, core library only).** The Lake package lives at `proofs/lean`. Models compile into oracle executables (§4).
3. **Bridges.** These are:
   - oracle tests in the kernel's home crate;
   - fingerprinted anchors that make a model stale when the code it describes changes;
   - the registry, which ties kernels, specs, callers, mutants, models and trusted items together (§3).

---

## 2. Code level: the in-place, zero-dependency kernel pattern

### 2.1 How it works

- **The kernel file is the production module.** Its contract is written as attributes and items that exist only when `cfg(verus_keep_ghost)` is set, and only Verus sets it. rustc strips them by its own cfg processing in every normal build, test, clippy, rustdoc, fuzz or `cargo install`, so no proc-macro or stub runs.
- **No dependency is added.** No manifest names `vstd` or any Verus crate. `Cargo.lock`, `deny.toml` (which uses `all-features = true`), the published closure (ADR-088), `check-dag` and ADR-056 are all untouched.
- **One manifest change.** The single change is in the root `Cargo.toml`:

```toml
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(coverage_nightly)", "cfg(verus_keep_ghost)"] }
```

### 2.2 A kernel file (abridged from the verified prototype of K1)

```rust
//! Verified kernel (ADR-095): the core of `xargs.partition` (`xargs.py:76-117`), called by `super::partition`.
//! Spec and lemmas: proofs/verus/spec/xargs_partition.rs. Guarantees and non-guarantees: docs/VERIFICATION.md §9.
use std::ops::Range;

#[cfg(verus_keep_ghost)]
use crate::spec::xargs_partition::*;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// Greedy partition of `lens` (`cmdlen(file) + 1` each) under `base`, `max_len` and `max_args`;
/// `Err(i)`: the first argument that fits no command (Python's `ArgumentTooLongError`).
#[cfg_attr(verus_keep_ghost, verus_spec(res =>
    ensures partition_post(res, lens@, base as int, max_len as int, max_args as int),
))]
pub(crate) fn partition_lens(lens: &[usize], base: usize, max_len: usize, max_args: usize)
    -> Result<Vec<Range<usize>>, usize> {
    let n = lens.len();
    let (mut ret, mut start, mut count, mut total, mut i) = (Vec::new(), 0_usize, 0_usize, base, 0_usize);
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant /* ret tiles 0..start, budgets hold, total == base + Σ lens[start..i], … */,
        decreases n - i, count,
    ))]
    while i < n {
        let Some(&arg_len) = lens.get(i) else { return Err(i) };
        let fits = match total.checked_add(arg_len) { Some(t) => t <= max_len, None => false };
        #[cfg(verus_keep_ghost)]
        proof! { lemma_sum_range_step(lens@, start as int, i as int); }
        /* … the same three branches as today … */
    }
    ret.push(start..i);
    Ok(ret)
}
```

### 2.3 Rules for kernel files (enforced by §3.2)

- **K1. Marking and registration.** The first line is `//! Verified kernel (ADR-095): …`. The file has a `[[kernel]]` entry in `proofs/registry.toml` and exactly one mount in `proofs/verus/kernels.rs`.
- **K2. Self-contained exec code.** Exec code names only `core`/`alloc`/`std` and items of the same file: no `crate::`, `super::` or external crate. Injected primitives such as memchr enter through a *kernel trait* with a contract (§2.6; from M2).
- **K3. No platform cfg.** Platform behaviour is a `windows: bool` parameter, and the shell passes `cfg!(windows)`. Linux CI then verifies both variants.
- **K4. Only these ghost forms are allowed:**
  - `#[cfg_attr(verus_keep_ghost, X)]` with X one of: `verus_spec(..)`, a bare `verus_verify`, `verifier::external_derive(..)`, `verifier::rlimit(..)`, `verifier::loop_isolation(..)` or `verifier::spinoff_prover`;
  - `#[cfg(verus_keep_ghost)]` on `use` items (only `vstd::prelude::*` and `crate::spec::…`);
  - `#[cfg(verus_keep_ghost)]` on `proof!{}`/`proof_decl!{}` statements.

  Kernel files contain **no** `verus!{}` block. Specs and lemmas live in `proofs/verus/spec/<kernel>.rs`, which keeps production files readable.
- **K5. Total entry points.** Entry points return `Result`/`Option`. No non-private function has a `requires`: at an unverified call site a precondition would be a silent assumption. Kernels are `pub(crate)` where possible, so `-D warnings` `dead_code` proves the shell still calls them. They are `pub` only when a fuzz target or another crate needs them (K3 end-of-file-fixer).
- **K6. Only the std surface Verus specifies.**
  - Avoid: `take_while`, `count`, `fold`, `sum`, `then_some`, `is_some_and`, `map_or*`, `rposition` without a spec, closures with destructuring patterns (`|&c|` cannot carry a contract) and closures capturing `&mut`. Use index loops instead.
  - Loops carrying an invariant are in statement position. A loop in expression position (`let x = loop {…}`) cannot take the attribute on stable (E0658).
- **K7. Lint style.** Either write clippy-clean code (`get`, `checked_*`, `saturating_*` on counters proved not to saturate), or put a per-function `#[expect(clippy::indexing_slicing, clippy::arithmetic_side_effects, reason = "verus: <what is proved>")]` on a verified function. The latter is a proven site under ADR-066 as amended.
- **K8. Compiles on Verus's rustc (1.99.1 today).** Avoid 1.100-only APIs such as `trim_prefix`/`trim_suffix`. Tested: `map_or_default` compiles on 1.99.1, but it has no vstd spec. A violation shows up as a verification compile error, so no further guard is needed. (`#[clippy::msrv]` was tested and does not change clippy's suggestions, so it is not used.)
- **K9. Derived impls on data-carrying enums.** A data-carrying enum with a derived `PartialEq` (such as `EofAction`) carries `#[cfg_attr(verus_keep_ghost, verifier::external_derive)]`. Verified code uses `matches!`, never `==` on it.
- **K10. Documentation.** The doc comment states the property in prose and cites the upstream source as `file:line` at the pinned refs. Unit tests stay in `#[cfg(test)] mod tests;` or in the shell's tests.

### 2.4 The harness

```rust
//! proofs/verus/kernels.rs: verification root. Mounts production files unchanged; every item in them is verified.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
#![forbid(unsafe_code)]               // also makes assume_specification impossible in mounted files (tested)
#[allow(unused_imports)]
use vstd::prelude::*;
pub mod spec;                         // proofs/verus/spec/{mod.rs, lib/{seq,bytes,arith}.rs, <kernel>.rs …}

#[verus_verify]                       // without it, unannotated fns are silently skipped (tested)
#[path = "../../crates/snp-proc/src/xargs/partition.rs"]
pub mod snp_proc_xargs_partition;
// … one mount per kernel file …
```

**Command:**

```
verus proofs/verus/kernels.rs --crate-type=lib --edition=2024 --no-cheating --output-json
```

- `--verify-module <mount>` checks one kernel in about 1–2 s. **Verus 0.2026.11.02 refuses `--verify-module` together with `--no-cheating`**, so a module run (`cargo xtask proofs verus --module M`) omits `--no-cheating` and is an iteration aid, never a gate. The gate is always the whole-root run; the guards of §3.2 reject cheat tokens independently of the flag.
- `--expand-errors` and `--rlimit` are for debugging.
- The run uses the prebuilt vstd inside the pinned Verus zip. There is no `cargo verus`, no vstd from crates.io and no cold vstd re-verification.
- `cargo verus` is not usable anyway: it compiles the whole dependency graph with rustc 1.99.1, and snp uses 1.100 APIs.

### 2.5 Why the verified program is the shipped program

| Difference between the two compilations | Why it does not change the proved behaviour |
|---|---|
| Crate root (harness vs home crate) | Exec code in a kernel file names only std and the same file (G9). A path that resolved differently would not resolve in the harness at all. |
| cfg set (`verus_keep_ghost` on vs off) | Only the ghost forms of K4 differ (G2–G5). Mount-level `#[verus_verify]` plus the JSON manifest (V2) ensure nothing is skipped. |
| Compiler: rustc 1.99.1 front end vs 1.100.0 + LLVM | Same stable language subset. All tests, fuzz targets and difftests run on 1.100 builds (TCB 3). |
| `vstd::prelude::*` visible only under Verus | A glob import never shadows explicit or inherent names. An ambiguity is a compile error, not a silent change (TCB 3). |
| Target | No platform cfg in kernel files. Both values of `windows` are verified, and Verus leaves `usize`'s width open (32 or 64 bit). |

### 2.6 Kernel traits: the boundary for fast primitives (M2 onward)

A kernel that needs memchr, memmem or a hash set is generic over a trait whose methods carry the contract. A verified scalar reference implementation lives in the kernel file. The production implementation (`memchr::memchr` and so on) lives in the shell.

- **Cost.** Monomorphisation makes this zero-cost, so the SIMD speed is kept.
- **Trust.** The production implementation is a trusted assumption. It is listed in `registry.toml` `[[trusted]]` together with a differential test against the reference implementation (G8).
- **Status.** Prototyped: a `FindByte` trait with a verified `Scalar` implementation verifies in attribute mode.
- **Contract precision.** A contract must state the primitive's real semantics. For example, `memmem::find_iter` reports *non-overlapping* matches (memchr 2.8.3 docs).

---

## 3. Registry and guards

### 3.1 `proofs/registry.toml` (single machine-readable source)

The committed file (abridged):

```toml
[verus]
version = "0.2026.11.02.8bf68ee"            # bundles rust_verify, Z3 4.16.0 and the prebuilt vstd
rustc = "1.99.1"                            # from the zip's version.json; never the rustup default
sha256 = { x86-linux = "f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c" }
verusfmt = { version = "0.7.4", sha256 = { x86_64-unknown-linux-gnu = "bbc5cb02…971f4f" } }

[[kernel]]
id = "K1-xargs-partition"
status = "verified"                         # verified | pending (see §8.3)
file = "crates/snp-proc/src/xargs/partition.rs"
mount = "snp_proc_xargs_partition"
spec = "proofs/verus/spec/xargs_partition.rs"
callers = ["crates/snp-proc/src/xargs.rs::partition"]
verified_min = 10
mutants = [
    { find = "Some(t) => t <= max_len", replace = "Some(t) => t < max_len" },
    { find = "match total.checked_add(arg_len) {", replace = "match Some(total.saturating_add(arg_len)) {" },
]
```

Planned shapes (no entry exists yet; K3 and K4 add the first models):

```toml
[[model]]
id = "K4-shlex-split"
lean = "proofs/lean/SnpModels/Shlex.lean"
theorems = ["SnpModels.Shlex.pySplit_eq_split", "SnpModels.Shlex.split_quote"]
oracle = "shlex-model"                      # a lean_exe of proofs/lean/lakefile.toml
oracle_test = "crates/snp-pycompat/tests/lean_oracle.rs"
anchors = [{ item = "crates/snp-pycompat/src/shlex.rs::split", fingerprint = "<16 hex digits>" }]

[[trusted]]                                 # empty in M1; from M2:
item = "Memchr: FindByte"                   # `Type: Trait`
kernel = "K5-zsplit"
test = "crates/snp-git/tests/find_byte.rs::memchr_matches_scalar"
```

**Field rules** (`cargo xtask proofs check` rejects unknown keys, so a typo cannot drop a check):

- Only the pinned platforms have a sha256. Add another platform's hash when a contributor needs it; `proofs-tools.sh` refuses a platform without one.
- `verified_min` is the number of items Verus reports verified for this kernel's mount, its spec and the lib lemmas it introduced, measured when the entry is added. A whole-root run must verify at least the sum over all verified kernels (V2).
- `mutants`: the first occurrence of `find` in `file` is replaced by `replace`. The mutant must fail *verification*; one that fails to compile is reported as invalid.
- `callers` and `test` name `path/to/file.rs::fn`; anchors name `path/to/file.rs::item`, where `item` is a top-level item, `module::item` of an inline module, or `Type::method`.
- **Anchor fingerprint**: `fnv1a64` over the item's tokens as syn lexes them, without its `#[doc]` attributes and without a trailing comma before a closing delimiter, each token rendered alone and joined by single spaces, written as 16 lowercase hex digits. Comments, doc comments and rustfmt re-wrapping keep it; any change to the code, its attributes or its signature changes it. `cargo xtask proofs fingerprint <file.rs::item>` prints the current value; a placeholder such as `TBD` fails G10 until it is filled.

### 3.2 Tool-free guards (`cargo xtask proofs check`, syn-based, in the required `xtask gates` job from M0)

| ID | Check |
|---|---|
| G1 | Registry, markers and mounts agree in both directions: each verified kernel has its marker line and exactly one mount with the right `#[path]`, a pending kernel has the pending marker and no mount, every mount and every marked file under `crates/` is registered. Mounts carry a bare `#[verus_verify]` and point under `crates/`; the root carries `#![forbid(unsafe_code)]`; a verified kernel has at least one mutant; ids, files and mounts are unique. |
| G2 | The only cfg predicates in a kernel file are a bare `verus_keep_ghost`, plus `test` on a `mod` item. No `not`/`any`/`all`, no `cfg!`. |
| G3 | `#[cfg(verus_keep_ghost)]` attaches only to the allowed `use` items and `proof!`/`proof_decl!` statements. There is no `verus!` in kernel files. |
| G4 | The payload of `cfg_attr(verus_keep_ghost, …)` is on the K4 allowlist. `verus_verify(...)` with arguments is rejected, as are `external`, `external_body` and `exec_allows_no_decreases_clause` (which `--no-cheating` does not catch: tested). |
| G5 | Cheat tokens (`assume`, `admit`, `assume_specification`, `external_body`, `external`, `exec_allows_no_decreases_clause`) appear neither in kernel files nor in `proofs/verus/spec/`. Only `proofs/verus/trusted/` is exempt. |
| G6 | No `requires` in the contract of a non-private function. Trait method declarations are exempt; their impls fall under G8. |
| G7 | `reason = "verus: …"` expectations appear only in mounted kernel files, on functions, and as `#[expect]` (never `#[allow]`). |
| G8 | Every impl of a kernel trait outside kernel files is a `[[trusted]]` entry whose named test exists. |
| G9 | Exec code in kernel files has no path rooted in `crate`, `super`, `self::super` or an external crate. A multi-segment path (and every `use`) must start at `std`, `core`, `alloc`, `self`, `Self`, a std prelude name or primitive type, or a name the file itself declares; macro bodies are scanned for `crate`/`super`. `#[cfg(test)]` modules are skipped. |
| G10 | Each listed caller exists and its body calls one of the kernel's non-private functions. Each model's Lean file and oracle test exist, its oracle is a `lean_exe` of the Lake package, and it lists at least one theorem and one anchor. Each anchor fingerprint matches; a mismatch means "model link stale: re-review and update the fingerprint". |

The guards are syn-based: tokens are compared after lexing, so words in comments and string literals never fire. Their unit tests run on a fixture tree (`xtask/src/tasks/proofs/fixtures/good`) with one good case and one or more bad cases per guard. `proofs check` takes well under a second.

### 3.3 Verus checks (`cargo xtask proofs verus`)

- **V1.** The root verifies with `--no-cheating`.
- **V2.** Manifest: every `fn` and method syn finds in a kernel file (outside `#[cfg(test)]` modules) appears in `--output-json` `func-details` as `kernels::<mount>::<path>`: `f`, `module::f`, `Type::method` (Verus names an impl on a non-path type `impl&%N`) or `Trait::method`. The whole-root verified count is at least the sum of `verified_min`. The run also prints the five slowest functions (`--time`).
- **V3.** Every registered mutant, applied to a scratch copy of the root, the specs and the mounted files under `target/proofs/mutant/`, makes verification fail (Verus reports errors). A `find` string that no longer exists is an error and asks for a fresh mutant; a mutant that breaks compilation instead of verification is an error too.
- **V4.** `verusfmt --check` passes on `proofs/verus/spec` and the root. Kernel files are formatted by rustfmt, which leaves attribute contents alone (tested).

### 3.4 Lean checks (`cargo xtask proofs lean`)

- **L1.** `lake build --wfail` passes. A `sorry` is only a warning and lean exits 0, so the flag is mandatory.
- **L2.** Axiom audit (`lake env lean Axioms.lean`): every `#print axioms` line gets an answer, the result is a subset of {`propext`, `Classical.choice`, `Quot.sound`}, and every theorem a `[[model]]` registers is printed there.
- **L3.** `lake env leanchecker SnpModels` passes (skipped with a note when the toolchain has no `leanchecker`).
- **L4.** No `.lean` file of the package contains `sorry`, `admit`, `native_decide`, `implemented_by`, `extern`, `unsafe`, `partial` or `axiom` as a word outside comments and strings. `partial def` is allowed only in the `*Main.lean` I/O drivers. `lakefile.toml` has no `require` and `lake-manifest.json` lists no package.

---

## 4. Model level: Lean 4 models and oracles

### 4.1 Layout

```
proofs/lean/
  lean-toolchain            leanprover/lean4:v4.34.1   (validated 2026-11-13; bump to the latest stable in its own PR)
  lakefile.toml             package SnpModels; no `require` (core library only)
  SnpModels/{Eof,Shlex,Hex}.lean   later: Stash/Protocol.lean, …
  EofMain.lean ShlexMain.lean      oracle executables eof-model, shlex-model
  Axioms.lean
  client.rs                 std-only line-protocol client, included by oracle tests via #[path] (never packaged)
```

### 4.2 Three roles for Lean

1. **Spec validation.** `pyX` is a line-by-line transliteration of the upstream Python, and a theorem proves `pyX = X`, where X is the declarative spec matching the Verus spec.
2. **Properties SMT handles poorly.** Examples: simulation against CPython's state machines (`pySplit = split`), round trips, inductive and protocol properties.
3. **Protocol models.** Example: the stash protocol (K9). These carry named hypotheses about git and POSIX, never `axiom` declarations.

### 4.3 Oracle tests

- **Location.** Oracle tests live in the kernel's **home crate**, are written with std only, and use a seeded SplitMix64 generator (the snp-pycompat precedent in `tests/properties.rs`): `crates/snp-hooks/tests/lean_oracle.rs` and `crates/snp-pycompat/tests/lean_oracle.rs`.
- **Why not snp-difftest.** snp-difftest cannot host them: `check_dag.rs:162` gives it normal edges only to snp-testkit and snp-yaml, and dev-dependencies must be ancestors, so snp-hooks, snp-net, snp-stash and snp-store are out of reach.
- **Why not a separate harness crate.** A non-member harness crate would add a lockfile and lock-parity tooling. Home-crate tests need neither, and snp-pycompat keeps zero dev-dependencies (ADR-056).
- **Environment.** Tests read the oracle directory from `SNP_LEAN_ORACLES` and skip when it is unset. With `SNP_REQUIRE_LEAN_ORACLES=1` (CI) a skip is a failure. The oracle runs as a child process with a cleared environment.
- **Protocol.** One hex line protocol: a request goes in, `OK <hex>…` or `ERR <text>` comes out, flushed per line, at about 21–25 µs per round trip.
- **Oracles run the proved functions.** `shlex-model` also self-checks `pySplit` against `split` and reports a mismatch.
- **Nightly.** `py_vs_lean` (Python, `python3 -I`, in the existing nightly oracle job) compares the models with CPython 3.12 and with pre-commit-hooks at the pinned refs. Per-PR CI stays free of Python, as §13.3 requires.

---

## 5. Toolchains

| Tool | Pin | Where | Install |
|---|---|---|---|
| Verus | 0.2026.11.02.8bf68ee, zip sha256 per platform (x86-linux `f3a01192…3bba3c`), Z3 4.16.0 bundled | `proofs/registry.toml` | `.github/scripts/proofs-tools.sh install` (curl + sha256 check, in the style of `deps-lock.sh`) into `${SNP_PROOFS_TOOLS:-~/.cache/snp-dev/tools}/verus/<ver>/verus-<platform>/`; `proofs-tools.sh path` prints that directory. `cargo xtask proofs install` wraps it. |
| rustc for Verus | 1.99.1, minimal profile, **never** `rustup default` (the `verus` launcher sets `RUSTUP_TOOLCHAIN`) | registry | rustup (about 593 MB unpacked) |
| verusfmt | 0.7.4 + sha256 (x86_64-unknown-linux-gnu) | registry | same script, into the same directory |
| Lean | v4.34.1 at import, then the latest stable (v4.35.0 today) in its own PR; never a release candidate | `proofs/lean/lean-toolchain` | elan. Where `release.lean-lang.org` does not resolve (as on the owner's machine), download the GitHub release tarball and run `elan toolchain link`. |
| lean-action | v1.7.1, pinned by commit SHA `ba3fdfe5c4476c925da13bf11a005ddb47e47aee` | `proofs.yml` | |
| snp toolchain | 1.100.0 (unchanged) | `rust-toolchain.toml` | |

**Bump policy:**
- **Verus:** quarterly, or when a needed fix or a newer rustc lands. One PR updates version, sha256 and rustc and re-runs everything; budget 0.5–2 days. There is no rolling pre-release.
- **Lean:** follows stable releases.
- **Excluded without a new ADR:** Mathlib, Batteries, Aeneas, Charon and hax.

---

## 6. Commands (`xtask/src/tasks/proofs.rs`, owner WP-00; uses the existing `syn`, `toml`, `serde_json`; no new dependency)

| Command | Does |
|---|---|
| `cargo xtask proofs check` | G1–G10. Tool-free, well under a second. Part of the required `xtask gates` job. |
| `cargo xtask proofs verus [--module M] [--json F]` | V1–V4, plus the slowest functions from Verus's `--time` report. `--json` keeps the report. With `--module`: one mount without `--no-cheating` (§2.4), its V2 manifest and V4; no floor, no mutants; not a gate. |
| `cargo xtask proofs lean` | L1–L4 (L4 first: it needs no tool). |
| `cargo xtask proofs oracles [-k ID]` | Builds the Lean executables, then runs each model's home-crate oracle test (`cargo nextest run`, or `cargo test` without nextest, `--release -p <crate> --test <stem>`) with `SNP_LEAN_ORACLES` set and `SNP_REQUIRE_LEAN_ORACLES=1`. With no `[[model]]` registered it says so and succeeds. |
| `cargo xtask proofs mutants` | V3 alone (fast negative check). From M3, `--generate` is to add syn-based mutants as a nightly survivor report (planned). |
| `cargo xtask proofs install` / `status` | Installs the pinned tools (`proofs-tools.sh install`). Prints the pins, the installed tools, the kernel table (status, mount, `verified_min`, mutants), each model's anchors (fresh or stale) and the guard count. |
| `cargo xtask proofs fingerprint <file.rs::item>` | Prints an anchor fingerprint (§3.1), to fill or refresh a `[[model]]` anchor. |

Tools are found in the `proofs-tools.sh path` directory, else on `PATH`; `proofs verus` fails unless `verus --version` reports the registry's pin. Lean comes from elan (`lake` on `PATH`).

`check-unsafe` scans `proofs/` too, and requires `#![forbid(unsafe_code)]` in the Verus root, which no Cargo manifest covers. Only `proofs/verus/trusted/`, if it ever exists, is exempt. `.gitignore` has `/proofs/lean/.lake/` and `/proofs/verus/out/`.

`proofs/verus/run.sh` stays as a dependency-free reference runner of the same Verus command (whole root, `--min`, `--mutant`).

## 7. CI and gating

| Phase | Where | Status | Content |
|---|---|---|---|
| M0 → (done) | `ci.yml` `xtask gates` (required) | blocking | + `cargo xtask proofs check` |
| M0–M1 (done) | `proofs.yml` job `verus` | advisory | cache the tools dir and `~/.rustup/toolchains/1.99.1-*` (key: hash of the registry and `proofs-tools.sh`); `cargo xtask proofs install`, `proofs check`, `proofs verus --json` (report uploaded as an artifact) |
| M0–M2 (done) | `proofs.yml` job `lean` | advisory | lean-action pinned by SHA (`lake-package-directory: snp/proofs/lean`, `build-args: --wfail`, `axiom-audit: true`, `leanchecker: true`, `use-mathlib-cache: false`, `test`/`lint: false`); `cargo xtask proofs lean`; `cargo xtask proofs oracles` (every registered model's oracle test, `SNP_REQUIRE_LEAN_ORACLES=1`) |
| M1 exit (first production call of a verified kernel) | `xtask gates` | **blocking** (owner decision pending, recommended) | + `cargo xtask proofs install && cargo xtask proofs verus`; the advisory `verus` job is dropped. No branch-protection change is needed. **Note:** production already calls K1, so until this decision a kernel edit that breaks its proof can merge; only the advisory job turns red. |
| M1 exit (planned) | `cargo xtask release gates` | blocking for tags | new **gate 7 "proofs"**: re-verify the tagged commit; no kernel `pending` unless the release notes accept it |
| after M2 | `proofs.yml` `lean` | required after 4 green weeks on main, p95 < 5 min, owner approval | |
| nightly (planned) | `nightly.yml` | | `py_vs_lean` against CPython 3.12 and pre-commit-hooks; Z3 seed sweep (`--smt-option smt.random_seed=1..5`); half-`--rlimit` fragility report; generated mutants (M3); 10 000-case oracle runs |

**Rules:**
- Jobs always start and exit early when no relevant path changed. Do not use `on.paths`: a required check that a path filter skips stays pending forever.
- Permissions are read-only.

**Runtime (measured unless noted):**
- **Verus:** 25 items in 1.7 s wall. M1 is about 40–60 items (estimated under 10 s) plus about 2 s per mutant. Tool install takes seconds from cache; cold, the 36 MB zip plus the minimal toolchain take about 40–60 s (estimated). Net cost to `xtask gates`: about 30 s warm, about 1.5 min cold.
- **Lean:** build 3.5 s, leanchecker 1.5 s; toolchain fetch 20–40 s (estimated). The oracle tests themselves take about 10–20 s, dominated by building snp-hooks (rust-cache). Expected Lean job: 3–5 min.

---

## 8. Developer workflow

### 8.1 Everyone else
Contributors who do not touch a kernel file see no change. Build, test, clippy, fmt and MSRV are unaffected, and no Verus or Lean is needed.

### 8.2 Editing a kernel file
1. Run `cargo xtask proofs install` once (or `source ~/.cache/snp-dev/tools/verus/env.sh` on the owner's machine).
2. Edit the exec code and its contract. Iterate with `cargo xtask proofs verus --module <mount>` (about 1–2 s), adding `--expand-errors` when needed.
3. Run `cargo xtask proofs check`, then nextest, clippy and fmt as usual.
4. Spec changes go into `proofs/verus/spec/<kernel>.rs` and cite the upstream `file:line`. Generic lemmas go into `proofs/verus/spec/lib/`.
5. Before merging, `cargo xtask proofs mutants` must kill every registered mutant.

### 8.3 Escape hatch
An urgent fix to a kernel whose proof is not ready may land as follows:
- the kernel is set to `status = "pending"` with an issue link;
- its mount is removed;
- its marker line becomes `//! Kernel (ADR-095), proof pending: #NNN`.

The PR diff shows all of this. Release gate 7 fails while any kernel is pending, unless the release notes accept it explicitly.

### 8.4 Adding a kernel
1. Fix any known bug in the code first (§11).
2. Extract the pure core into its own file next to the shell. The extraction must be behaviour-preserving, with existing tests unchanged.
3. Write the spec and the uniqueness or property lemmas.
4. Add the mount, the registry entry (callers, mutant) and a G/¬G row in §9.
5. Add the oracle test if a Lean model exists.

---

## 9. Kernels

**Notation:**
- `@` is the ghost sequence view, `++` concatenation and `ε` the empty sequence.
- All spec arithmetic is in ℤ.
- `run(s)` is the length of the longest suffix of `s` consisting only of CR/LF bytes.

The properties below were checked against the current code. Where a property holds only after a listed fix, it is marked as such.

### 9.1 M1: four kernels that prove the pipeline end to end

| ID | Status (2026-11-13) |
|---|---|
| K1 | **verified**: registered, mounted, 10 items with `--no-cheating`, both mutants killed, called by `xargs::partition` |
| K2, K3, K4 | in progress (M1); not registered yet, so nothing below is claimed for them until their registry entries land |


| ID | Kernel | Tool | Crate | Exercises | Production change |
|---|---|---|---|---|---|
| K1 | xargs partition core | Verus | snp-proc | integer kernel; the "Verus finds a divergence → fix → CHANGELOG" loop; clippy-clean style; `pub(crate)` + dead_code | extract ~40 lines; checked budget |
| K2 | archive entry-name check | Verus | snp-net | security-class property; `windows: bool`; monotone shell | extract ~50 lines |
| K3 | end-of-file-fixer | Verus + Lean | snp-hooks | moving a kernel out of a file that imports memchr; `pub` re-exports for fuzz targets; `external_derive`; relational lemmas; the full Lean pipeline and home-crate oracle test | move ~70 lines, loops for iterator chains |
| K4 | shlex split model | Lean | snp-pycompat | model level; std-only oracle test in snp-pycompat (ADR-056); anchors; nightly CPython check | none |

**Why these four:**
- All are prototyped.
- None depends on open PRs.
- M1 needs **zero** trusted items, so the trust register stays empty while the pipeline is proven.

**K1: xargs partition core.**
- **Code.** `crates/snp-proc/src/xargs.rs:152-198` → new `xargs/partition.rs::partition_lens(lens, base, max_len, max_args) -> Result<Vec<Range<usize>>, usize>`.
- **Properties.** Let `n = |lens|`, `B = base`, `L = max_len`, `A = max_args`, `cost(s,e) = Σ lens[s..e]`, and `over(j) := A = 0 ∨ B + lens[j] > L`.
  - **P0** total, panic-free and overflow-free.
  - **P1** tiling: `Ok(R) ⇒ |R| ≥ 1`, `R[0].start = 0`, `R.last.end = n` and `R[k].end = R[k+1].start`. So concatenating `files[R[k]]` gives `files`: nothing is lost, duplicated or reordered.
  - **P2** `n = 0 ⇒ R = [0..0]`; `n > 0 ⇒` no range is empty.
  - **P3** `|R[k]| ≤ A` and, for a non-empty range, `B + cost(R[k]) ≤ L`.
  - **P4** greedy: every non-final range is full, or `B + cost(R[k]) + lens[R[k].end] > L`.
  - **P5** `Ok ⇔ ∀j ¬over(j)`; `Err(i) ⇒ over(i) ∧ ∀j<i ¬over(j)`.
  - **P6** uniqueness lemma: at most one `R` satisfies P1–P4. Hence `partition_lens` equals Python's `xargs.partition`, and the proptest `partition_matches_the_python_model` becomes a theorem for the kernel.
- **Truth today.** The properties hold for today's saturating loop whenever `max_len < usize::MAX`. At `max_len = usize::MAX`, today's code returns `Ok([0..1])` for `lens=[usize::MAX], base=1`, where Python raises `ArgumentTooLongError`; Verus found this. The kernel uses `checked_add`, which is Python's semantics.
- **Shell.** `partition` keeps: the `lens[i] = cmdlen(f)+1` computation, the `max_len == 0` platform default, `max(4, ⌈n/jobs⌉)`, the Windows `resplit_batch`, and the mapping of `Err(i)` to `ArgumentTooLong(files[i])`.
- **G:** no file is lost, duplicated or reordered; budgets hold; the result is Python's. **¬G:** per-OS `cmdlen`, `platform_max_length` and the Windows resplit (K13).
- **Evidence.** In the repository the root verifies 10 items for K1 in about 1.5 s (`cargo xtask proofs verus`); the prototype's count of 12 covered a different set of items.

**K2: archive entry-name check.**
- **Code.** `crates/snp-net/src/archive/path.rs:19-91` → new `archive/entry.rs::entry_components(raw, windows) -> Result<Vec<(usize, usize)>, Refusal>`.
- **Definitions.** `sep_w = {/}`, or `{/, \}` when `windows`. `comps_w(raw) = [c ∈ split(raw, sep_w) | c ≠ ε ∧ c ≠ "."]`. `rooted(raw)` means the first byte is `/` or `\`, or `raw` starts with `[A-Za-z]:`.
- **Properties.**
  - **P0** total for every byte string and both values of `windows`.
  - **P1** `Err ⇔ raw = ε ∨ rooted(raw) ∨ ".." ∈ comps_w(raw) ∨ 0 ∈ raw`. A NUL byte always lies in a kept component.
  - **P2** `Ok(spans) ⇒ map(raw[s..e], spans) = comps_w(raw)`, with spans in bounds and strictly increasing.
  - **P3** every returned component is plain: non-empty, not `.` or `..`, no NUL, no separator.
  - **P4** validation precedes stripping, so a `..` inside the stripped prefix still refuses.
  - **P5** lemma: `∀D. normpath(D ++ v) = D ++ v` for plain `v`.
- **Truth today.** On Unix today's `entry_path` equals the kernel followed by the `OsStr` conversion. On Windows the shell's `component()` (UTF-8, NTFS-illegal characters, reserved names) only adds refusals. The shell is monotone: it may turn `Ok` into `Err`, never the reverse.
- **G:** no accepted name can leave `dest` lexically, for every byte string. **¬G:** the tar/zip decoders, the O_NOFOLLOW walk, symlinks (backlog), case-insensitive aliasing, TOCTOU, and the Windows-only refusals in the shell.
- **Evidence.** The zero-dependency prototype verified the safety half (P0, P3) with 6 items, and its `..`-accepting mutant fails. The functional half (P1, P2) is the M1 work.

**K3: end-of-file-fixer.**
- **Code.** `crates/snp-hooks/src/pch/p0/kernels.rs:84-157` (`EofAction`, `end_of_file`, `is_eol`, `end_of_file_run`, `end_of_file_bytes`) → new `kernels/eof.rs`, re-exported from `kernels`. The re-export is needed because `kernels.rs` imports memchr, which a kernel file may not. The shipped path is `end_of_file_fixer.rs:76-121` (`tail_action` → `end_of_file_run`).
- **Spec F (the same as Lean `Eof.fix`).**
  - `F(ε) = ε`.
  - `F(s) = s ++ [LF]` if `run(s) = 0`.
  - `F(s) = ε` if `run(s) = |s|`.
  - Otherwise, with `p = |s| − run(s)`: `F(s) = s[..p+2]` if `s[p] = CR ∧ s[p+1] = LF`, else `s[..p+1]`.
- **Properties.**
  - **E0** total, panic-free and overflow-free (u64 lengths).
  - **E1** `end_of_file_bytes(s) = None ⇔ F(s) = s`, otherwise `Some(F(s))`.
  - **E2** the shipped path: for `(head, run) = (first min(run(s), 2) bytes of the trailing run, run(s))` of any `s` with `|s| = len`, `end_of_file_run(head, run, len)` is the action whose application to `s` gives `F(s)`.
  - **E3** no data loss: `Truncate(n) ⇒ n < |s|` and every byte of `s[n..]` is CR or LF. `Append ⇔ s ≠ ε ∧ last(s) ∉ {CR, LF}`.
  - **E4** tail locality (the fuzz invariant as a lemma): `end_of_file(s[k..], |s|) = end_of_file(s, |s|)` when `s[k..]` contains a non-CR/LF byte or `k = 0`.
  - **E5** `F(F(s)) = F(s)`, and `Some(v) ⇒ v ≠ s`.
- **Pattern.** Exec functions ensure `r == spec(inputs@)`. The relational properties E2, E4 and E5 are spec lemmas, so no ghost parameters are needed.
- **Lean (proved, no `sorry`).** `pyFixFile_eq` (the Python transliteration equals F, including the return code), `kernel_apply`, `fix_idem`, `fix_body`, `pyFixFile_twice`.
- **Bridge.** `eof-model` against `end_of_file_bytes` on all of `{a, CR, LF}^≤10` (88,573 inputs) plus seeded random inputs.
- **G:** byte-exact parity with `fix_file` for the decision and the bytes; only trailing line endings are ever removed. **¬G:** the I/O shell (`tail_action`'s block reads until K6; the TOCTOU window before `set_len`).

**K4: shlex split model.**
- **Code.** `crates/snp-pycompat/src/shlex.rs:110-195` (`is_ws`, `St`, `Esc`, `split`). Model: `proofs/lean/SnpModels/Shlex.lean`.
- **Properties.**
  - **S1 (Lean)** `pySplit_eq_split: ∀s. pySplit s = split s`. The one-pass state machine snp implements equals the transliteration of CPython's `read_token`/`__next__` (posix, `whitespace_split`, `commenters=''`), with the same tokens and both `ValueError`s.
  - **S2 (Lean)** `split_quote: ∀s. split (quote s) = .ok [s]`.
  - **S3 (tested per PR)** `snp_pycompat::shlex::split` agrees with `shlex-model` on all 335,923 strings over `{' ', '\t', '\'', '"', '\\', 'a'}` up to length 7, plus seeded random inputs.
  - **S4 (tested nightly)** the model agrees with CPython 3.12 (the prototype checked 157,257 split and 20,000 quote cases).
  - The anchor pins `split` and its state types.
  - Next theorem: `split (join ts) = .ok ts`.
- **G:** argv parity with CPython for every string, at model level; the Rust code equals the model on the tested domain. **¬G:** a proof about the Rust `split`; Python lone surrogates, which cannot reach `&str`.

### 9.2 M2 (planned, not started)

- **K5 zsplit / ZList::split** (Verus; snp-pycompat `precommit.rs:13-46`, snp-git `zlist.rs:64-112`).
  - `strip0` strips leading and trailing NULs. `zsplit_spec(b) = []` if `strip0(b) = ε`, else `split_nul(strip0(b))`.
  - The pycompat `zsplit` yields `zsplit_spec(b@)`, through a trim kernel plus a one-record step lemma.
  - `ZList::split`: `Ok ⇔ |buf| ≤ u32::MAX`. On `Ok` the spans view equals `zsplit_spec(buf@)`, or `dedup_adjacent` of it for `from_z_dedup`. Spans are in bounds, starts strictly increase and records are NUL-free.
  - This introduces the first kernel trait: `Memchr: FindByte`, trusted, with a differential test against the verified `Scalar`.
  - Iterator glue stays unverified. The frozen ADR-056 signatures are unchanged.
- **K6 end-of-file tail fold** (Verus; snp-hooks `end_of_file_fixer.rs:76-121`).
  - A pure `fold_block(state, block) -> (state, done)` is extracted from `tail_action`.
  - For every `s` and every block size `B ≥ 1`, folding from the end yields `(run(s), head(s))`, which closes K3's E2 gap.
  - Reads stop at the first block containing a non-CR/LF byte.
- **K7 D-DATA-19 pathspec lists** (Verus; snp-git `git.rs:79-87, 92-117, 124-135, 620, 682`; **after PR #17 merges**).
  - **N1 (true today)** `parse_numstat_z(concat(aᵢ TAB dᵢ TAB nameᵢ NUL)) = Ok([nameᵢ])` for tab-free `aᵢ`, `dᵢ` and non-empty NUL-free names. Tabs and newlines are allowed in names.
  - **N2 (true today)** `Ok(L) ⇒` every path is non-empty and NUL-free. A record with fewer than two tabs gives `Err`. No panic.
  - **N3 (true today)** for NUL-free paths, splitting `literal_pathspecs(L)` on NUL (without its final NUL) gives `map(":(literal)" ++ ·, L)`, and the encoding is injective.
  - **N4 (after the hardening PR)** the checkout sink refuses a list containing an empty or NUL-bearing path (an empty `:(literal)` selects the whole tree, the very loss D-DATA-19 removed). The rollback list is the first-seen dedup of `T ++ (C minus empty records)`.
  - Dedup goes through a `SeenSet` kernel trait (HashSet implementation trusted).
- **K8 gc patch protection** (Verus; snp-store `maintenance.rs:76-114, 167-182, 296-345`; snp-stash `journal.rs:160-200`).
  - **G1 (true today)** `mentioned(name, J) ⇔ ∃j∈J, k: j[k..k+|name|] = name ∧ (k+|name| = |j| ∨ j[k+|name|] ∉ [A-Za-z0-9._-])`. This needs `name` to be border-free, and every `patch<digits>-<digits>` name is. The reason is that `memmem::find_iter` reports non-overlapping matches; the trusted `Memmem` contract says so.
  - **G2 (true today)** deletable ⇒ the name starts with `patch`, its age is at least the threshold, and it is not mentioned.
  - **G3** adding journals never makes a kept patch deletable.
  - **G4 (after F1; false today)** an unrestored journal of `dir/n` protects `n` whatever the journal's size and path encoding (UTF-8 string or `{"hex": …}`). The proof uses the lemma `hex(a ++ b) = hex(a) ++ hex(b)`, or decodes journals with the shared codec (preferred).
- **K9 stash protocol model** (Lean; snp-stash `guard.rs`, snp-git checkout and rollback, ADR-093; **after PR #17**).
  - **Model.** Index `I`, worktree `W : Path → Option Content`, clean and smudge filters `c` and `s`, an assume-unchanged set, `Patch(W)` and `D = dom Patch(W₀)`.
  - **Hypotheses (named, not axioms).** A1 `c∘s = id` on the image of `I`; A2 apply is all-or-nothing; A3 checkout writes exactly the given set; A4 there is no concurrent writer; A5 `numstat(Patch) = D` (linked to K7).
  - **Theorems.**
    - T1: the round trip without hooks.
    - T2: the exact residual-loss set equals ADR-093's consequences paragraph.
    - T3: a machine-checked counterexample for pre-commit's `checkout -- .`.
    - T4: rollback completeness.
    - T5: no silent loss when the final apply fails.
    - T6: the index is invariant apart from stage_fixed.
  - **Link.** Fingerprinted anchors on the `guard.rs` steps, with the `tests/behaviour/hidden.rs` scenarios as named witnesses. **It proves nothing about the binary by itself.**

### 9.3 M3 (planned, not started)

- **K10 shlex quote and hook script** (Verus + Lean; snp-pycompat `shlex.rs:39-106`, snp-cmd `scripts.rs:268-290`).
  - **Q1** `quote_bytes(b)` is `''` for `ε`; `b` when every byte is SAFE (`[A-Za-z0-9_@%+=:,./-]`); otherwise `'` ++ `b` with each `'` replaced by `'"'"'` ++ `'`.
  - **Q2** for NUL-free `b`, POSIX sh word lexing of `quote_bytes(b)` yields exactly `[b]`. The lexer spec is restricted to unquoted SAFE runs and single-quoted segments, and it is cross-checked against dash and bash in a test.
  - **Q3** `render`'s `set --` line yields `args.len() + 3` words before git's arguments, matching `shift args.len()+3`.
  - **Lean** `split (join ts) = .ok ts`.
  - **Change** `all(|&c| is_safe(c))` becomes an index loop (K6), and `quote(&str)` delegates to the byte loop.
- **K11 deletion containment** (Verus; snp-os `fs.rs:150-212`, snp-lang `rmtree.rs:11-36`, snp-pycompat `posixpath.rs:26`). It works over a byte-level component enum, with `ci = cfg!(windows)` passed by the shell.
  - `Ok(rel) ⇒ rel ≠ []`, every element is Normal, and `strip_cur(R) ++ rel ≡ strip_cur(P)`, exactly or ASCII-case-insensitively.
  - Any `..` in `P ⇒ Err`; `P ≡ R ⇒ Err`; completeness: `P = R ++ [Normal …]` with at least one component `⇒ Ok`.
  - rmtree's `relative`: `Ok(c) ⇒ c` is non-empty and all Normal.
  - `normpath` is idempotent; it has no `.` or empty interior component; `..` appears only as a leading run of a relative path; `isabs` is preserved; and Python's exactly-two-leading-slashes case is kept.
  - Trusted: `std::path::absolute` and `Components`.
- **K12 line fixers** (Verus; snp-hooks `kernels.rs:9-80, 186-262`).
  - **Shared lemma:** readlines pieces and `splitlines(keepends)` pieces (CRLF by maximal munch) concatenate to the content.
  - **trailing-whitespace:** it equals `concat(map(_process_line, readlines(c)))` and returns `Some` iff that differs from `c`. The output is a subsequence of `c` with the same LF count. It is idempotent for `chars = None`.
  - **mixed-line-ending:** counts follow `fix_filename`; auto ties prefer LF > CRLF > CR; every line becomes its body plus the target ending, including an unterminated last line. The output is uniform, bodies are preserved, and it is idempotent.
  - memchr/memchr2 enter through kernel traits. The ADR-081 bench gates and perf-smoke watch the loop rewrite.
- **K13 xargs aggregate and batch resplit** (Verus; snp-proc `xargs.rs:272-315`).
  - `aggregate([]) = 0`; otherwise it returns the first code with maximal `unsigned_abs`, which is MIN-safe. `aggregate = 0 ⇔` all codes are 0.
  - The resplit core over an injected `fits` contract tiles and refines the input ranges, every output range fits or has length ≤ 1, and it terminates.

### 9.4 Backlog (not scheduled; each needs its fix or refactor first)

- Store key injectivity and `safe_component` (after the `abs:`/`url:` and `h<hex16>` collision fixes).
- stage_fixed acceptance (after the §8.3 fix), as a Verus core plus a Lean cross-hook model.
- The install/uninstall planner (classify → plan → execute).
- Archive symlink resolution: a Verus local contract plus a Lean global theorem on a filesystem model with name equivalence.
- The stash crash state machine with fault-point trace conformance.
- Identify-tier soundness (Lean) and the file-identify TagSet (same pattern, in the owner's repo).
- `c_unquote`/`diff_git_name`; `sha256_hex`/`to_hex`.

---

## 10. Trusted computing base

| # | Trusted | Mitigation |
|---|---|---|
| 1 | Verus (VIR→SMT encoding, `rust_verify`) and Z3 4.16.0 | sha256-pinned bundle; `--no-cheating`; registered mutants must fail; nightly seed sweep |
| 2 | vstd's specs of the std pieces used (slice, `Vec`, `Option`/`Result`, integer ops, `Range`) | small std surface (K6); every test, fuzz target and difftest runs on the shipped build |
| 3 | rustc 1.99.1 front end vs 1.100.0 codegen; vstd prelude glob visible only under Verus | kernel files stay in the common subset; an ambiguity is a compile error |
| 4 | Ghost-only differences between the verified and the shipped file | G2–G5, V2 |
| 5 | Mounts are the production files; nothing is silently skipped | G1, mount-level `#[verus_verify]`, V2 manifest |
| 6 | Preconditions at unverified call sites | none allowed (G6) |
| 7 | `assume_specification`/`external_body` | none in kernel files (G4, G5, root `forbid(unsafe_code)`); none at all in M1; later only in `proofs/verus/trusted/`, listed |
| 8 | Production impls of kernel traits (memchr, memmem, HashSet; from M2) | `[[trusted]]` + G8 + differential test against the verified reference implementation |
| 9 | Compiler-derived impls (`external_derive`) | verified code does not use them (`matches!`) |
| 10 | Shells: I/O, `OsStr` and `Path` conversion, error mapping, iterator glue | a few lines each, monotone where security-relevant, existing tests, fuzz and difftest; doc comment "unverified shell of kernel X" |
| 11 | Spec ≠ intent (hand transcriptions of Python) | uniqueness lemmas; independent property lemmas; Lean `pyX = X` theorems with CPython cross-checks; oracle tests; mutants; `file:line` citations |
| 12 | Lean kernel and the three standard axioms | `--wfail`, axiom allowlist, `leanchecker` |
| 13 | Lean compiler and runtime behind the oracle executables | oracles are test oracles only and run the proved functions; L4 |
| 14 | Model ↔ Python | transliteration plus nightly `py_vs_lean`. Documented gap: Python `str` admits lone surrogates; Lean `Char` and Rust `char` do not. |
| 15 | Model ↔ Rust (no bridge exists) | bounded-exhaustive alphabets that cover every state transition, plus seeded random inputs; anchors |
| 16 | Model ↔ world (git, POSIX: K9) | named hypotheses, each with a witness test on the git floor (2.31) and the latest git |
| 17 | Allocation failure, abort, stack overflow | not modelled (ADR-066) |

---

## 11. Fix-first queue (ordinary PRs with tests and D-entries; proofs lock fixes in and never encode bugs)

- **F1. gc patch protection, D-DATA-04.** Confirmed by reading the code:
  - an unreadable journal protects nothing (`maintenance.rs:80`, `filter_map(… .ok())`);
  - `read_capped` truncates at the 1 MiB `META_CAP` (`fs.rs:262`, `marker.rs:19`), so a mention past the cap is missed;
  - a non-UTF-8 store path is journaled as `{"hex": …}` (`journal.rs:5,160-175`), so the raw patch name never appears.

  The fix: fail closed on read errors, a full read or protect-all over the cap, and match hex names or decode journals with the shared codec. The owner decides whether `<patch>.*` siblings such as `.now` are protected.
- **F2. xargs checked budget.** Lands with K1 (CHANGELOG "Fixed", regression test at `usize::MAX`).
- **F3. Checkout sink hardening.** Refuse empty or NUL pathspecs and filter empty records. Lands with K7 as a D-DATA-19 amendment.
- **Reported by the surveys, not yet confirmed.** Each needs a failing test in its own PR before any proof depends on it:
  - stage_fixed staging an input that an earlier hook dirtied;
  - the RepoKey `abs:` collision and the `safe_component` `h<hex16>` collision;
  - try-repo `--files` not rebased onto the cwd;
  - revline with `#` or whitespace in a rev (bug-compatible: needs a D decision, not a proof);
  - spill collector ordering after a failed write;
  - the `scrub_urls` credential leak;
  - `diff_git_name` accepting `a/ b/`;
  - journal deletion after a failed ITA re-add;
  - the stash patch rewrite window;
  - a stale assertion in the `stash_journal_parse` fuzz target.

---

## 12. Milestones, exit criteria, effort

**M0: pipeline, no behaviour change (6–8 engineer-days). Done:** the items below exist on the `feat/formal-verification` line. It was planned to start on `main` after PRs #14–#17 merge, because the check-cfg line touches the manifest that #16 changes; the integrator rebases accordingly. Contents:
- ADR-095; this document; ARCHITECTURE §1.3 (check-cfg), §11.7 (CI tool supply chain: sha256-pinned, never in builds), §13.1 (new layer **L9 proofs**) and §13.7; the DECISIONS preamble.
- `proofs/` skeleton: registry, root, spec lib, and the Lean package imported from the prototype (Lean models are not yet linked by a `[[model]]` entry: K3 and K4 add them).
- `xtask proofs` with guard unit tests on fixture files; `proofs-tools.sh`.
- `proofs.yml` (advisory); `proofs check` in `xtask gates`.
- `check-unsafe` scope and `.gitignore`.

**M1: K1–K4 (12–16 days).** Effort per kernel: K1 2–3, K2 3–4, K3 4–5 (including the oracle test), K4 1–2, plus slack.

M1 exit criteria:
1. `cargo xtask proofs verus` is green with `--no-cheating`, the manifest is complete, the trusted list is empty, and the run takes under 15 s.
2. Production calls K1–K3 (G10 and dead_code).
3. All existing tests, fuzz smoke, difftest, perf-smoke and bench gates pass unchanged, except for the new `usize::MAX` test.
4. `cargo deny check`, `Cargo.lock`, an empty `cargo tree -p snp-pycompat` and `cargo publish --workspace --dry-run --locked` are all unchanged.
5. Every kernel has a mutant that fails; a deliberately broken kernel edit fails CI (documented negative test).
6. Lean: `--wfail`, axiom audit, `leanchecker` and both oracle tests (with `SNP_REQUIRE_LEAN_ORACLES=1`) are green.
7. The Verus step is blocking in `xtask gates` and in release gate 7 (owner decision).
8. The §9 G/¬G rows are complete.

**M2: K5–K9, plus F1 and F3 (22–30 days). M3: K10–K13 and generated mutants (18–26 days).**

**Total:** about 58–80 engineer-days. **Upkeep:** 0.5–2 days per Verus bump, about 0.5 day per Lean bump, and 1–2 days a month of triage.

---

## 13. Risks and the kill criterion

1. **Verus churn.** It is pre-1.0 with weekly releases, and the attribute syntax is its newest surface; `verifier::verify` and `--no-external-by-default` are already deprecated. *Mitigation:* exact pins, deliberate bumps, small lemmas, and the negative CI test.
2. **Attribute-mode maturity.** Trait contracts were tested; closures with contracts and `&mut` ghost interplay were not, and K6 avoids them. *Mitigation:* a per-kernel spike before committing a shape.
3. **SMT brittleness.** *Mitigation:* explicit triggers on element-level predicates, intermediate asserts, `--rlimit` margins, the nightly seed sweep and half-rlimit report, and per-function timings from the JSON.
4. **rustc lag.** Kernel files trail snp's MSRV idioms. *Mitigation:* the constraint applies to kernel files only, and the Verus pin moves when a needed API appears.
5. **Wrong specs.** *Mitigation:* see TCB row 11. Bug-compatible quirks are explicit spec exceptions (trailing-whitespace `--chars`).
6. **Contributor friction.** *Mitigation:* about a dozen marked files, exact failure messages and the escape hatch.
7. **Performance of loop rewrites.** *Mitigation:* kernel traits keep SIMD; ADR-081 bench gates and perf-smoke.
8. **Lean footprint.** The toolchain is 3 GB unpacked, and the elan DNS fallback is needed locally. There will be a temptation to add Mathlib (6.7 GB of oleans); the ADR forbids it.
9. **Bus factor.** *Mitigation:* the work package owner, §8.4, and kernels kept to a few hundred proof lines each.

**Kill criterion.** If upkeep exceeds about 3 engineer-days a month, or two consecutive Verus bumps break more than two proofs, the Verus step returns to advisory. The Lean models and oracle tests stay.

---

## 14. Out of scope

- Whole-program equivalence; I/O and filesystem effects beyond named model hypotheses.
- Concurrency (JobPool, install scheduler, lock token).
- Regex engines, YAML, argparse, Unicode tables, ntpath, tar/zip decoders, TLS, blake3 and sha2.
- The owner's crates (deferred; same pattern later).
- Aeneas, Charon and hax (they need a pinned nightly rustc with rustc-dev, a Lean release candidate and Mathlib, and the binary could not be run here). Re-evaluate in 2027-H1 or at Charon 1.0.
- Lean FFI (it needs `unsafe`).

---

## 15. Alternatives considered and contradictions resolved

| Question | Options on the table | Decision | Why |
|---|---|---|---|
| Integration | (A) `verus!` crate with a vstd dependency; (B) stub proc-macro facade; in-place `cfg_attr`; sidecar of token-identical mirrors | in-place `cfg_attr`, zero dependencies | Identity by construction; no eraser in the TCB; no Cargo change. A adds 8 weekly-prerelease crates to the published closure (§11.7 budget 120, 109 used) and cannot serve snp-pycompat. B adds two published crates plus an optional vstd to the `all-features` deny graph. Mirrors need resynchronising on every edit. |
| Spec placement | in the kernel file / in `proofs/verus/spec` | `proofs/verus/spec` | Production files stay readable; kernel files carry no `verus!`, which simplifies G3/G5 (prototyped with `crate::speclib`). |
| Proof statements | `#[cfg(verus_keep_ghost)] proof!{}` / a no-op `macro_rules! proof` under `not(...)` | the former | Only positive cfg in kernel files (G2). |
| Lint relief | per-function `expect` / file-level `cfg_attr(not(..), expect)` | per-function, `reason = "verus: …"` | Precise; matches ADR-066. |
| Unannotated items | mount-level `#[verus_verify]` + JSON manifest / annotate every item + count | mount-level + manifest + `verified_min` | Tested: a plain mount verifies 0 items silently, while a marked mount reports the overflow. |
| Oracle harness | snp-difftest / non-member `verify/harness` | home-crate std-only tests | `check_dag.rs:162` and the ancestor rule forbid snp-difftest → snp-hooks; no new crate or lockfile. |
| Verus roots | one / one per area / three crates | one root; a separate trusted crate only if ever needed | `assume_specification` cannot be declared under `forbid(unsafe_code)` (tested). |
| Clippy MSRV for kernel files | `#[clippy::msrv]` | dropped | Tested: it does not change the suggestion, and the inner form is E0658. |
| Lean pin | 4.34.1 / 4.35.0 | 4.34.1 at import, then the latest stable | Start from the validated version. |
| Gating | required at M1 / advisory → required later / release gate first | guards blocking from M0; Verus blocking at M1 exit + release gate; Lean after M2 | Claims about shipped kernels need a gate; models do not. |
| M1 | partition+zsplit+eof / tiny Level-A items+eof B+shlex / pathspecs+entry+stash model+shlex | partition, entry path, eof (+Lean), shlex model | All prototyped, no trusted items, no dependency on open PRs. |

---

## 16. Evidence (2026-11-13; the design-time prototypes are local, not committed)

- **Verus in-place prototype.** `~/.cache/snp-dev/tools/verus/poc-inplace-2026-11-13/`: partition, zsplit, the `FindByte` trait, count. Re-run today: 25 verified, 0 errors, in 1.7 s with `--no-cheating`. Lib clippy is clean with snp's lints and `-D warnings`. Kernel files are rustfmt-clean. `cargo tree` shows no dependencies.
- **Zero-dependency prototype.** `~/.cache/snp-dev/tools/verus/poc-2026-11-13/cfgattr-zero-dep/run.sh`: 3 + 6 items verified, the mutant fails, stable clippy and `cargo package` succeed.
- **Probes run today:**
  - a mount without `#[verus_verify]` verifies 0 items, while a marked mount catches overflow in an unannotated function;
  - `--output-json` lists `root::mount::fn`;
  - `#[expect(clippy::…)]` is accepted under Verus;
  - `const fn` and `usize as u64` verify;
  - `assume_specification` under `forbid(unsafe_code)` fails with "declaration of an `unsafe` function";
  - `--no-cheating` rejects `assume_specification`;
  - rustc 1.99.1 rejects `trim_prefix` and accepts `map_or_default`.
- **Lean prototype.** `~/.cache/snp-dev/tools/proofs-poc-2026-11-13/lean-proto/`, rebuilt today: `lake build --wfail` takes 3.5 s; the axiom audit shows only {propext, Classical.choice, Quot.sound}; the models contain no `partial def`.
- **Research measurements.** Lean models agree with pre-commit-hooks `fix_file` on 49,524 cases and with CPython 3.12.3 shlex on 177,257 cases. Rust agrees with the oracles on about 425k inputs in 9 s.
