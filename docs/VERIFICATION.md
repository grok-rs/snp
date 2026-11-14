# Formal verification (ADR-095)

*Status (2026-11-13): ADR-095 accepted. M0 (pipeline, guards, CI, docs) is in place; M1's kernels are in: K1–K3 are verified with Verus (40 items with `--no-cheating`) and K3 and K4 have proved Lean models with oracle tests (§9.1); the M1 exit decision on blocking CI is pending (§7). M2 and M3 are planned, not started. Owners: WP-38 "Formal verification" owns `proofs/` and the specs; WP-00 owns the root manifest, xtask (`xtask/src/tasks/proofs.rs`) and CI. Package owners review the kernel files in their crates.*

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

**Specs are trusted.** Verus checks the code against its spec, and the lemmas check that the spec has the named properties. Nothing checks the spec against the upstream Python: a spec and code that are wrong in the same way (say, `eol` changed to LF-only in both) still verify. That link rests on the Lean `pyX = X` theorems, the oracle tests and, for K2, the exhaustive comparison with the former check (§9.1, §10 row 11).

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
 CPython / pre-commit-hooks ──(py_vs_lean: by hand today, nightly planned)────► Lean transliteration pyX
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
use crate::spec::lib::seq::{lemma_sum_range_nonneg, sum_range};
#[cfg(verus_keep_ghost)]
use crate::spec::xargs_partition::{in_budget, maximal, oversize, partition_post};
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
        proof! { lemma_sum_range_nonneg(lens@, start as int, i as int); }
        /* … the same three branches as today … */
    }
    ret.push(start..i);
    Ok(ret)
}
```

### 2.3 Rules for kernel files (enforced by §3.2)

- **K1. Marking and registration.** The first line is `//! Verified kernel (ADR-095): …`. The file has a `[[kernel]]` entry in `proofs/registry.toml` and exactly one mount in `proofs/verus/kernels.rs`.
- **K2. Self-contained exec code.** Exec code names only `core`/`alloc`/`std` and items of the same file: no `crate::`, `super::` or external crate. Injected primitives such as memchr enter through a *kernel trait* with a contract (§2.6; from M2). The file is the whole kernel: no out-of-line `mod x;`, no `#[path]`, no `extern crate`, and **no macro call or definition** other than the ghost `proof!`/`proof_decl!` (write `match` for `matches!`). A macro means what its scope says: a parent module's `macro_rules!` or a crate-level `#[macro_use]` would change it in production only, and `include!` pulls in text no guard reads. The kernel's crate has no `#[macro_use]` and no `extern crate`.
- **K3. No platform cfg.** Platform behaviour is a `windows: bool` parameter, and the shell passes `cfg!(windows)`. Linux CI then verifies both variants.
- **K4. Only these ghost forms are allowed:**
  - `#[cfg_attr(verus_keep_ghost, X)]` with X one of: `verus_spec(..)`, a bare `verus_verify`, `verifier::external_derive(..)`, `verifier::rlimit(..)`, `verifier::loop_isolation(..)` or `verifier::spinoff_prover`;
  - `#[cfg(verus_keep_ghost)]` on `use` items: `vstd::prelude::*`, and *named* items of `crate::spec::<kernel>` (no glob, no `self`, no name of the std prelude, a primitive type or `std`/`core`/`alloc`: a ghost import with such a name would shadow it under Verus only);
  - `#[cfg(verus_keep_ghost)]` on `proof!{}`/`proof_decl!{}` statements.

  Kernel files contain **no** `verus!{}` block. Specs and lemmas live in `proofs/verus/spec/<kernel>.rs`, which keeps production files readable.
- **K5. Total entry points.** Entry points return `Result`/`Option`. No non-private function has a `requires`: at an unverified call site a precondition would be a silent assumption. Kernels are `pub(crate)` where possible, so `-D warnings` `dead_code` proves the shell still calls them. They are `pub` only when a fuzz target or another crate needs them (K3 end-of-file-fixer).
- **K6. Only the std surface Verus specifies.**
  - Avoid: `take_while`, `count`, `fold`, `sum`, `then_some`, `is_some_and`, `map_or*`, `rposition` without a spec, closures with destructuring patterns (`|&c|` cannot carry a contract) and closures capturing `&mut`. Use index loops instead.
  - Loops carrying an invariant are in statement position. A loop in expression position (`let x = loop {…}`) cannot take the attribute on stable (E0658).
- **K7. Lint style.** Either write clippy-clean code (`get`, `checked_*`, `saturating_*` on counters proved not to saturate), or put a per-function `#[expect(clippy::indexing_slicing, clippy::arithmetic_side_effects, reason = "verus: <what is proved>")]` on a verified function. The latter is a proven site under ADR-066 as amended.
- **K8. Compiles on Verus's rustc (1.99.1 today).** Avoid 1.100-only APIs such as `trim_prefix`/`trim_suffix`. Tested: `map_or_default` compiles on 1.99.1, but it has no vstd spec. A violation shows up as a verification compile error, so no further guard is needed. (`#[clippy::msrv]` was tested and does not change clippy's suggestions, so it is not used.)
- **K9. Derived impls on data-carrying enums.** A data-carrying enum with a derived `PartialEq` (such as `EofAction`) carries `#[cfg_attr(verus_keep_ghost, verifier::external_derive)]`. Verified code uses `match`, never `==` on it.
- **K10. Documentation.** The doc comment states the property in prose and cites the upstream source as `file:line` at the pinned refs. Unit tests stay in `#[cfg(test)] mod tests;` or in the shell's tests.

### 2.4 The harness

```rust
//! proofs/verus/kernels.rs: verification root. Mounts production files unchanged; every item in them is verified.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
#![forbid(unsafe_code)]               // also makes assume_specification impossible in mounted files (tested)
#[allow(unused_imports)]
use vstd::prelude::*;
pub mod spec;                         // proofs/verus/spec/{mod.rs, lib/{mod.rs, seq.rs}, <kernel>.rs …}; lib/bytes.rs, lib/arith.rs planned from M2

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
| `vstd::prelude::*` visible only under Verus | A glob import never shadows explicit or inherent names. An ambiguity is a compile error, not a silent change (TCB 3). Spec items are imported by name, never by glob, and no imported name is a prelude, primitive or `std`/`core`/`alloc` name (G3), so a spec item cannot shadow what production resolves. |
| Module context (parent `macro_rules!`, `#[macro_use]`, files a macro or `mod` would load) | Kernels call and define no macro but the ghost `proof!`/`proof_decl!` (G3), load no other file (G9: no out-of-line `mod`, `#[path]`, `include!`), and the kernel's crate has no `#[macro_use]` or `extern crate` (G9). Every file under `proofs/verus/` is scanned (G5) and is the root, a spec or trusted (G1). |
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

Shapes of the other entries (`[[model]]` as registered for K4; `[[trusted]]` is still empty):

```toml
[[model]]
id = "K4-shlex-split"
lean = "proofs/lean/SnpModels/Shlex.lean"
theorems = ["SnpModels.Shlex.pySplit_eq_split", "SnpModels.Shlex.split_quote", "SnpModels.Shlex.split_join"]
oracle = "shlex-model"                      # a lean_exe of proofs/lean/lakefile.toml
oracle_test = "crates/snp-pycompat/tests/lean_oracle.rs"
driver_fingerprint = "af906ff552d63f5c"       # proofs/lean/ShlexMain.lean, the lean_exe's root (G10)
oracle_test_fingerprint = "a54309f336092389"  # the oracle test file (G10)
anchors = [
    { item = "crates/snp-pycompat/src/shlex.rs::split", fingerprint = "71f6ef5a43be5b96" },
    # … quote, join, and the helpers they call: is_ws, St, Esc, is_safe, quote_bytes
]
[model.statements]                          # L5: each theorem's statement plus the model definitions it reaches
"SnpModels.Shlex.pySplit_eq_split" = "67a279bf3ae4866e"
# … one line per entry of `theorems`

[[trusted]]                                 # empty in M1; from M2:
item = "Memchr: FindByte"                   # `Type: Trait`
kernel = "K5-zsplit"
test = "crates/snp-git/tests/find_byte.rs::memchr_matches_scalar"
```

**Field rules** (`cargo xtask proofs check` rejects unknown keys, so a typo cannot drop a check):

- Only the pinned platforms have a sha256. Add another platform's hash when a contributor needs it; `proofs-tools.sh` refuses a platform without one.
- `verified_min` is the number of items Verus reports verified for this kernel's mount, its spec and the lib lemmas it introduced, measured when the entry is added (in practice: the whole-root count with the kernel minus the count without it). A whole-root run must verify at least the sum over all verified kernels (V2). Verus's `verified` count is coarse: it does not track the number of `func-details` entries (adding a spec fn can leave it unchanged), so the floor catches a lost kernel or spec module, not a single lost lemma; V2's manifest check covers exec functions one by one.
- `mutants`: the first occurrence of `find` in the kernel file (or in the kernel's spec, with `file = "<spec>"`) is replaced by `replace`. The mutant must fail *verification*; one that fails to compile is reported as invalid. A spec mutant shows that a lemma depends on the spec definition it mutates.
- `callers` and `test` name `path/to/file.rs::fn`; anchors name `path/to/file.rs::item`, where `item` is a top-level item, `module::item` of an inline module, or `Type::method`.
- **Anchor fingerprint**: `fnv1a64` over the item's tokens as syn lexes them, without its `#[doc]` attributes and without a trailing comma before a closing delimiter, each token rendered alone and joined by single spaces, written as 16 lowercase hex digits. Comments, doc comments and rustfmt re-wrapping keep it; any change to the code, its attributes or its signature changes it. `cargo xtask proofs fingerprint <file.rs::item>` prints the current value; a placeholder such as `TBD` fails G10 until it is filled. An anchor hashes its own item only, so list the private helpers a modelled function calls as anchors of their own.
- **Driver and oracle-test fingerprints** (`driver_fingerprint`, `oracle_test_fingerprint`): the same hash over a whole file: the oracle test by its normalised tokens, the Lean driver (the `root` of the oracle's `[[lean_exe]]`) by its text with trailing whitespace dropped. Both decide what "the oracle agrees" means and neither is proved, so any edit fails G10 until someone re-reviews it and refreshes the value with `cargo xtask proofs fingerprint <file>`.
- **`[model.statements]`** pins every entry of `theorems` (G10 checks that the two lists match). L5 computes each value after `lake build`: FNV-1a over the elaborated type of the theorem and of every package constant it reaches (definitions with their bodies, inductive types with their constructors; never a proof). A vacuous restatement under a registered name, or an edit to a definition a theorem talks about, changes it. A Lean upgrade may change every value: re-review, then refresh.

### 3.2 Tool-free guards (`cargo xtask proofs check`, syn-based, in the required `xtask gates` job from M0)

| ID | Check |
|---|---|
| G1 | Registry, markers and mounts agree in both directions: each verified kernel has its marker line and exactly one mount with the right `#[path]`, a pending kernel has the pending marker and no mount, every mount and every marked file under `crates/` is registered. No file under `crates/` or `fuzz/` outside the registry uses `verus_keep_ghost`, `verus_spec` or `verus_verify` (a de-registered kernel would keep contracts that look verified), and every module of `proofs/verus/spec/mod.rs` is some kernel's `spec` (or `lib`). Every `.rs` file under `proofs/verus/` is the root, a spec or under `trusted/`. Mounts carry a bare `#[verus_verify]` and point under `crates/`; the root carries `#![forbid(unsafe_code)]`; a verified kernel has at least one mutant, and a mutant's `file` is the kernel file or its spec; ids, files and mounts are unique. |
| G2 | The only cfg predicates in a kernel file are a bare `verus_keep_ghost`, plus `test` on a `mod` item. No `not`/`any`/`all`, no `cfg!`. |
| G3 | `#[cfg(verus_keep_ghost)]` attaches only to the allowed `use` items (named spec items, none with a prelude, primitive or `std`/`core`/`alloc` name) and `proof!`/`proof_decl!` statements. A kernel file calls and defines no other macro: no `verus!`, `matches!`, `include!`, `macro_rules!`. |
| G4 | The payload of `cfg_attr(verus_keep_ghost, …)` is on the K4 allowlist. `verus_verify(...)` with arguments is rejected, as is `verus_verify` on a `mod`, and `external`, `external_body` and `exec_allows_no_decreases_clause` (which `--no-cheating` does not catch: tested). |
| G5 | Cheat tokens (`assume`, `admit`, `assume_specification`, `external_body`, `external`, `exec_allows_no_decreases_clause`) and raw identifiers (`r#…`, which rustc resolves to the plain name) appear neither in kernel files nor in any file under `proofs/verus/`. Spec files and the root declare spec and proof functions and datatypes only: no exec `fn`, `trait`, `impl`, `extern`, `unsafe`, `union` or macro definition, no `include!`/`env!`/`cfg!`-like macro, and spec files carry no `#[path]`. Only `proofs/verus/trusted/` is exempt. |
| G6 | No `requires` in the contract of a non-private function. Trait method declarations are exempt; their impls fall under G8. |
| G7 | `reason = "verus: …"` expectations appear only in mounted kernel files, on functions, and as `#[expect]` (never `#[allow]`). |
| G8 | Every impl of a kernel trait outside kernel files is a `[[trusted]]` entry whose named test exists. |
| G9 | Exec code in kernel files has no path rooted in `crate`, `super`, `self::super` or an external crate. A multi-segment path (and every `use`) must start at `std`, `core`, `alloc`, `self`, `Self`, a std prelude name or primitive type, or a name the file itself declares; macro bodies are scanned for `crate`/`super`. A kernel file has no out-of-line `mod x;` (outside `#[cfg(test)]`), no `#[path]`, no `extern crate` and no item syn cannot parse; the kernel's crate has no `#[macro_use]` and no `extern crate`. `#[cfg(test)]` modules are skipped. |
| G10 | Each listed caller exists and its body calls one of the kernel's non-private functions. Each model's Lean file and oracle test exist, its oracle is a `lean_exe` (with a `root`) of the Lake package, it lists at least one theorem and one anchor, and `[model.statements]` pins exactly its theorems. Each anchor, driver and oracle-test fingerprint matches; a mismatch means "model link stale: re-review and update the fingerprint". |

The guards are syn-based: tokens are compared after lexing (raw identifiers unrawed), so words in comments and string literals never fire. Their unit tests run on a fixture tree (`xtask/src/tasks/proofs/fixtures/good`) with one good case and one or more bad cases per guard. `proofs check` takes well under a second.

### 3.3 Verus checks (`cargo xtask proofs verus`)

- **V1.** The root verifies with `--no-cheating`.
- **V2.** Manifest: every `fn` and method syn finds in a kernel file (outside `#[cfg(test)]` modules) appears in `--output-json` `func-details` as `kernels::<mount>::<path>`: `f`, `module::f`, `Type::method` (Verus names an impl on a non-path type `impl&%N`) or `Trait::method`. The whole-root verified count is at least the sum of `verified_min`. The run also prints the five slowest functions (`--time`).
- **V3.** Every registered mutant, applied to a scratch copy of the root, the specs and the mounted files under `target/proofs/mutant/`, makes verification fail (Verus reports errors). A `find` string that no longer exists is an error and asks for a fresh mutant; a mutant that breaks compilation instead of verification is an error too.
- **V4.** `verusfmt --check` passes on `proofs/verus/spec` and the root. Kernel files are formatted by rustfmt, which leaves attribute contents alone (tested).

### 3.4 Lean checks (`cargo xtask proofs lean`)

- **L1.** `lake build --wfail` passes. A `sorry` is only a warning and lean exits 0, so the flag is mandatory.
- **L2.** Axiom audit (`lake env lean Axioms.lean`): every `#print axioms` line gets an answer, the result is a subset of {`propext`, `Classical.choice`, `Quot.sound`}, and every theorem a `[[model]]` registers is printed there.
- **L3.** `lake env leanchecker SnpModels` passes (skipped with a note when the toolchain has no `leanchecker`).
- **L4.** No `.lean` file of the package contains `sorry`, `admit`, `native_decide`, `implemented_by`, `extern`, `unsafe`, `partial` or `axiom`, nor a metaprogram or syntax extension (`run_cmd`, `run_meta`, `run_elab`, `elab`, `elab_rules`, `macro`, `macro_rules`, `syntax`, `notation`, `infix*`, `initialize`, `builtin_initialize`, `attribute`, `#eval`, `#exit`), nor `_unsafe_rec` or `skipKernelTC`, as a word outside comments and string or char literals (the scanner knows char literals such as `'"'`, raw strings, `«…»` names, and keeps the code inside an interpolation `s!"…{e}…"`). `partial def` is allowed only in the oracle drivers, the `root` files of the `[[lean_exe]]` entries. Every file imports only `SnpModels` modules. `lakefile.toml` has no `require` and `lake-manifest.json` lists no package.
- **L5.** After the build, generated scripts (`target/proofs/lean-audit/`, one for the library and one per driver, since drivers cannot be imported together) check the *elaborated* environment, whatever the source spells: no constant of the package carries `implemented_by` or `extern`, and none is `unsafe` or `partial` except the compiler's own `…._unsafe_rec`, so the oracle executables run the proved definitions. They also compute the `[model.statements]` fingerprints; a mismatch is "statement pin stale" with the new value.

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
- **Protocol.** One hex line protocol: a request goes in, `OK <hex>…` (for `eof-model`, `OK <code> :<hex>`) or `ERR <text>` comes out, flushed per line, at about 21–25 µs per round trip.
- **Oracles run the proved functions.** `shlex-model` also self-checks `pySplit` against `split`, and `eof-model` checks `(kernel f).apply f` against `pyFixFile`; each reports a mismatch. Enforced in part: L5 rejects compiler attributes that would swap in other code, and the driver and the oracle test are fingerprinted (G10), so any edit to them is a visible re-review. That a driver calls the registered model functions, and that a test asserts every answer, is checked by that review, not by a tool.
- **CPython check (by hand; nightly planned).** `py_vs_lean` (Python, `python3 -I`) compares the models with CPython 3.12 and with pre-commit-hooks at the pinned refs (§9.1 S4; last run 413,372 / 39,524 cases). No workflow runs it yet; the planned job belongs in `nightly.yml` (§7), with its own Lean toolchain, since the existing nightly oracle job has none. Per-PR CI stays free of Python, as §13.3 requires.

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
| `cargo xtask proofs lean` | L1–L5 (L4 first: it needs no tool). |
| `cargo xtask proofs oracles [-k ID]` | Builds the Lean executables, then runs each model's home-crate oracle test (`cargo nextest run`, or `cargo test` without nextest, `--release -p <crate> --test <stem>`) with `SNP_LEAN_ORACLES` set and `SNP_REQUIRE_LEAN_ORACLES=1`. With no `[[model]]` registered it says so and succeeds. |
| `cargo xtask proofs mutants` | V3 alone (fast negative check). From M3, `--generate` is to add syn-based mutants as a nightly survivor report (planned). |
| `cargo xtask proofs install` / `status` | Installs the pinned tools (`proofs-tools.sh install`). Prints the pins, the installed tools, the kernel table (status, mount, `verified_min`, mutants), each model's anchors (fresh or stale) and the guard count. |
| `cargo xtask proofs fingerprint <file.rs::item \| file>` | Prints an anchor fingerprint (§3.1), to fill or refresh a `[[model]]` anchor, or the whole-file fingerprint of an oracle driver or oracle test. |

Tools are found in the `proofs-tools.sh path` directory, else on `PATH` (also on a platform the script has no pinned release for, such as Linux arm64 or Windows); `proofs verus` fails unless `verus --version` reports the registry's pin. Lean comes from elan (`lake` on `PATH`).

`check-unsafe` scans `proofs/` too, and requires `#![forbid(unsafe_code)]` in the Verus root, which no Cargo manifest covers. Only `proofs/verus/trusted/`, if it ever exists, is exempt. `.gitignore` has `/proofs/lean/.lake/` and `/proofs/verus/out/`.

The interim `proofs/verus/run.sh` was removed when `cargo xtask proofs` landed; proofs/README.md shows the raw `verus` command for extra flags.

## 7. CI and gating

| Phase | Where | Status | Content |
|---|---|---|---|
| M0 → (done) | `ci.yml` `xtask gates` (required) | blocking | + `cargo xtask proofs check` |
| M0–M1 (done) | `proofs.yml` job `verus` | advisory | cache the tools dir and `~/.rustup/toolchains/1.99.1-*` (key: hash of the registry's `[verus]` pins and of `proofs-tools.sh`; the downloaded archives are deleted after unpacking); `cargo xtask proofs install`, `proofs check`, `proofs verus --json` (report uploaded as an artifact) |
| M0–M2 (done) | `proofs.yml` job `lean` | advisory | lean-action pinned by SHA (`lake-package-directory: snp/proofs/lean`, `build-args: --wfail`, `axiom-audit: false`, `leanchecker: false` (they guess the module name `Snpmodels` from the package name and find no oleans; `proofs lean` runs both on `SnpModels`), `use-mathlib-cache: false`, `test`/`lint: false`); `cargo xtask proofs lean`; `cargo xtask proofs oracles` (every registered model's oracle test, `SNP_REQUIRE_LEAN_ORACLES=1`). This is the only place the oracle tests run with a model: in the required `ci.yml` test job `SNP_LEAN_ORACLES` is unset, so they skip and pass, and a Rust/model divergence turns only this advisory job red. |
| M1 exit (owner decision; production already calls K1–K3) | `xtask gates` | **blocking** (owner decision pending, recommended) | + `cargo xtask proofs install && cargo xtask proofs verus`; the advisory `verus` job is dropped. No branch-protection change is needed. **Note:** production already calls K1, so until this decision a kernel edit that breaks its proof can merge; only the advisory job turns red. |
| M1 exit (planned) | `cargo xtask release gates` | blocking for tags | new **gate 7 "proofs"** (not implemented yet): re-verify the tagged commit; no kernel `pending` unless the release notes accept it |
| after M2 | `proofs.yml` `lean` | required after 4 green weeks on main, p95 < 5 min, owner approval | |
| nightly (planned; none of this runs yet) | `nightly.yml` | | `py_vs_lean` against CPython 3.12 and pre-commit-hooks (a new job with its own Lean toolchain); Z3 seed sweep (`--smt-option smt.random_seed=1..5`); half-`--rlimit` fragility report; generated mutants (M3); 10 000-case oracle runs |

**Rules:**
- Jobs always start and exit early when no relevant path changed. Do not use `on.paths`: a required check that a path filter skips stays pending forever.
- Permissions are read-only.

**Runtime (measured unless noted):**
- **Verus:** M1's 40 items (K1–K3) in about 3–5 s wall for the root run; with the 10 mutants `cargo xtask proofs verus` takes about 40–55 s warm. Tool install takes seconds from cache; cold, the 36 MB zip plus the minimal toolchain take about 40–60 s (estimated). Net cost to `xtask gates`: about 45 s warm, about 2 min cold.
- **Lean:** build 3.5 s, leanchecker 1.5 s, the L5 audit scripts about 10 s (each imports `Lean`); toolchain fetch 20–40 s (estimated). The oracle tests themselves take about 10–20 s, dominated by building snp-hooks (rust-cache). Expected Lean job: 3–5 min.

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

The PR diff shows all of this. Release gate 7 (planned, M1 exit) will fail while any kernel is pending, unless the release notes accept it explicitly; until it exists, the release checklist has to look at `cargo xtask proofs status`.

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
| K1 | **verified**: registered, mounted, 10 items with `--no-cheating`, 2 mutants killed, called by `xargs::partition` |
| K2 | **verified**: registered, mounted, 16 items, 6 mutants killed (4 of the code, 2 of the spec's `..` branch), called by `archive/path.rs::entry_path` |
| K3 | **verified** (Verus) and **proved model** (Lean): 14 items, 2 mutants killed, called by `end_of_file_fixer.rs::tail_action`; Lean model with 5 theorems, 4 fingerprinted anchors and the `eof-model` oracle test in snp-hooks |
| K4 | **proved model** (Lean only, by design): 3 theorems, `shlex-model` oracle test in snp-pycompat, 8 fingerprinted anchors |

Whole root: 40 items verified, 0 errors, with `--no-cheating`, in about 3–5 s (the sum of the three floors, 10 + 16 + 14); 10 registered mutants, each failing verification (not compilation). Lean: `lake build --wfail`, the axiom audit (only `propext`, `Classical.choice`, `Quot.sound`), `leanchecker` and the L5 audit (no compiler attribute, every statement pin fresh) pass; both oracle tests pass with `SNP_REQUIRE_LEAN_ORACLES=1`.


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
  - **P6** uniqueness lemma: at most one result satisfies P1–P5 (`partition_post`; machine-checked). Python's greedy loop satisfies P1–P5: that step is argued in prose (each emitted command is non-empty, within budget and maximal; the pushed-back argument starts the next range; `ArgumentTooLongError` only for an argument that fails in a fresh command) and tested by `partition_matches_the_python_model`, not proved; no model of the Python loop exists. Given that step, `partition_lens` equals `xargs.partition`, and the proptest reduces to checking that its reference model satisfies P1–P5.
- **Truth today.** The properties hold for today's saturating loop whenever `max_len < usize::MAX`. At `max_len = usize::MAX`, today's code returns `Ok([0..1])` for `lens=[usize::MAX], base=1`, where Python raises `ArgumentTooLongError`; Verus found this. The kernel uses `checked_add`, which is Python's semantics.
- **Shell.** `partition` keeps: the `lens[i] = cmdlen(f)+1` computation, the `max_len == 0` platform default, `max(4, ⌈n/jobs⌉)`, the Windows `resplit_batch`, and the mapping of `Err(i)` to `ArgumentTooLong(files[i])`.
- **Cost.** The shell now builds the `lens` vector (`cmdlen(f) + 1` for every file) before calling the kernel, where the former loop computed `cmdlen` lazily and stopped at the first `ArgumentTooLong`. Output is unchanged; the `partition_50k` micro-bench is about 14% slower (about 11 µs per 50,000 files, pinned to one core), negligible next to process spawns. Accepted: a kernel taking a length callback would need a closure contract, and one taking `OsStr`s would pull `cmdlen` into the verified file.
- **G:** `partition_lens` is proved to satisfy P0–P6: no file is lost, duplicated or reordered, budgets hold, and at most one result satisfies the contract. Equality with Python's `xargs.partition` follows given the prose step of P6 (tested). **¬G:** a proof that the Python loop meets P1–P5; per-OS `cmdlen`, `platform_max_length` and the Windows resplit (K13).
- **Evidence.** In the repository the root verifies 10 items for K1 in about 1.5 s (`cargo xtask proofs verus`); the prototype's count of 12 covered a different set of items.

**K2: archive entry-name check.**
- **Code.** `crates/snp-net/src/archive/path.rs:19-91` → new `archive/entry.rs::entry_components(raw, windows) -> Result<Vec<(usize, usize)>, Refusal>` with `Refusal = Rooted | DotDot | Nul` and the private helpers `is_sep`, `is_rooted`, `classify`. `is_rooted` and `split` stay in `path.rs` too, because `resolve_symlink` uses them.
- **Definitions.** `sep_w = {/}`, or `{/, \}` when `windows`. `comps_w(raw) = [c ∈ split(raw, sep_w) | c ≠ ε ∧ c ≠ "."]`. `rooted(raw)` means the first byte is `/` or `\`, or `raw` starts with `[A-Za-z]:`.
- **Properties.**
  - **P0** total for every byte string and both values of `windows`.
  - **P1** `Err ⇔ raw = ε ∨ rooted(raw) ∨ ".." ∈ comps_w(raw) ∨ 0 ∈ raw`. A NUL byte always lies in a kept component.
  - **P2** `Ok(spans) ⇒ map(raw[s..e], spans) = comps_w(raw)`, with spans in bounds and strictly increasing.
  - **P3** every returned component is plain: non-empty, not `.` or `..`, no NUL, no separator.
  - **P4** validation precedes stripping, so a `..` inside the stripped prefix still refuses.
  - **P5** lemma: `∀D. normpath(D) = D ⇒ normpath(D ++ v) = D ++ v` for plain `v`, and `D` is a prefix of the result. It shows that normalising leaves appended plain components unchanged; it never reaches the `..` branch of the `normpath` model, so it does not check how the model treats `..`.
  - **P5b** lemma: for a normalised `D` whose last component is not `..`, `normpath(D ++ [".."]) = D` without its last component: a `..` would leave `D`, which is why P1 refuses it. This lemma does depend on the `..` branch: the spec mutants "`..` is always pushed" and "`..` clears the stack" are registered and fail it.
- **Truth today.** On Unix today's `entry_path` equals the kernel followed by the `OsStr` conversion. On Windows the shell's `component()` (UTF-8, NTFS-illegal characters, reserved names) only adds refusals. The shell is monotone: it may turn `Ok` into `Err`, never the reverse. The shell converts **every** kept span with `component()` before stripping, as the former code did, so the Windows-only refusals also cover the stripped prefix.
- **Proved (Verus).** P0–P3 are the exec contract of `entry_components` (`r is Err ⇔ refused(raw, windows)`, the `Rooted` variant exactly for `ε`/rooted names, and `spans_ok` on `Ok`); P5 is `lemma_normpath_append_plain` and P5b `lemma_normpath_dotdot`, over a component-level `normpath` fold (`norm_step`/`norm_from`). 8 lemmas in `proofs/verus/spec/archive_entry.rs`.
- **Not proved.** P4 is not a Verus theorem about the shell: the shell is unverified by design (OsString conversion, platform `component()`); P4 rests on the kernel taking no strip count plus P1 over the whole name, and is tested (`../top/x` strip 1, `a/../x/y` strip 2, `t\0p/x` strip 1). The monotonicity of the Windows shell holds by construction but is not machine-checked, and clippy and tests ran only on Linux (the kernel's `windows = true` variant is verified and tested there). P5 is about the component-level model, not Python's string-level `posixpath`/`ntpath.normpath` (drives, the real filesystem walk): out of scope. The spec (`rooted`, `sep`, `comps_w`) is trusted: a spec and kernel changed the same way (say, both forgetting drive letters) still verify. What checks the spec is the unit test in `path.rs` that compares the kernel with the former check on every short name.
- **G:** for every byte string, an accepted name has no `..` component, no separator inside a component and no NUL (P1, P3), so it cannot leave `dest` lexically; P5 adds that normalising keeps the joined path as it is. **¬G:** the tar/zip decoders, the O_NOFOLLOW walk, symlinks (backlog), case-insensitive aliasing, TOCTOU, and the Windows-only refusals in the shell.
- **Evidence.** 16 items in the whole root. Mutants: accept `..`, let a leading NUL through, forget `\` as a Windows separator, forget `\x` as rooted, and the two spec mutants of `norm_step`'s `..` branch (all fail verification). Unit tests in `path.rs` compare the kernel with the former check, rewritten with the platform as a parameter, on every name over `a . / \ NUL : C` up to length 6 for both `windows` values (274,514 cases).

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
- **Pattern.** Exec functions ensure `r == spec(inputs@)` (`run_action`, `tail_action`, `file_action`, `bytes_result` in `proofs/verus/spec/eof.rs`; `run_action` states the saturating `Truncate` faithfully, even for inconsistent inputs). The relational properties are spec lemmas, so no ghost parameters are needed: E2 `lemma_run_action_apply`/`lemma_tail_action_apply`, E3 `lemma_no_data_loss`, E4 `lemma_tail_locality`, E5 `lemma_fix_idempotent`, with `lemma_run_char`, `lemma_run_unique` and `lemma_run_append` as helpers. Rewrites for Verus, each with the same results: the trailing-run count is an index loop, the `[\n, \r\n, \r]` `starts_with` loop a match on the first two bytes, `to_vec` is `Vec::new()` + `extend_from_slice`, and `Result::unwrap_or` is `.ok().unwrap_or(..)` or a match.
- **Evidence.** 14 items in the whole root; mutants `run >= len` → `>` and "the run stops at the first `\r`" fail verification, and spec mutants that change `fix`/`run_action` (CRLF kept, all-EOL kept, truncation off by one, head length; tried, not registered) make E2 fail as well as the contract. A spec and code changed the same way in a shared base predicate (`eol`) still verify: the lemmas check the code against the spec and the spec's internal consistency, not the spec against pre-commit-hooks. Upstream fidelity comes from the Lean `pyFixFile_eq` plus the `eof-model` oracle test. Because each contract restates the code's exact result, any kernel edit fails on the contract alone; the lemmas are what carry the properties.
- **Lean (proved, no `sorry`).** `pyFixFile_eq` (the Python transliteration equals F, including the return code), `kernel_apply`, `fix_idem`, `fix_body`, `pyFixFile_twice`.
- **Bridge.** `crates/snp-hooks/tests/lean_oracle.rs`: `eof-model` against `end_of_file_bytes` on all of `{a, CR, LF}^≤10` (88,573 inputs) plus 20,000 seeded SplitMix64 inputs (seed `0x5eed000300000001`) over a wider alphabet, a quarter ending in line-ending runs of up to 300 bytes. The oracle replies `OK <code> :<hex>` and self-checks `(kernel f).apply f` against `pyFixFile` (`MISMATCH` otherwise). Anchors: `eof.rs::end_of_file_run`, `::end_of_file`, `::is_eol` and `::end_of_file_bytes`. Like every oracle test, it runs only in the advisory `lean` job (§7); in required CI it is skipped and passes.
- **Not proved.** That `tail_action` computes `(head, run)` correctly from its 4 KiB backward block reads (an I/O shell until K6; covered by the unit tests `tail_scan_matches_kernel` and `long_eol_run_is_linear`). That the Rust code equals the Lean model for all inputs: Verus proves the Rust code equals its own spec `fix`, transcribed to match Lean `fix`, but no tool checks that the two definitions are equal; the link is tested. TOCTOU between the decision and `set_len`/append.
- **G:** `end_of_file_bytes`/`end_of_file_run` are proved to satisfy spec F (E0–E5); only trailing line endings are ever removed; Lean proves F equals the `fix_file` transliteration (`pyFixFile_eq`). Parity with Python `fix_file` holds at model level, and the Rust code equals the model on the tested domain. **¬G:** a proof that the Verus `fix` equals the Lean `fix` (tested by `lean_oracle.rs`); a proof that the transliteration equals pre-commit-hooks `fix_file` (checked by hand with `py_vs_lean.py`, S4); `tail_action`'s computation of (head, run) and the rest of the I/O shell (block reads until K6; the TOCTOU window before `set_len`).

**K4: shlex split model.**
- **Code.** `crates/snp-pycompat/src/shlex.rs:108-195` (`is_ws`, `St`, `Esc`, `split`), plus `quote`/`quote_bytes`/`is_safe` and `join`. Model: `proofs/lean/SnpModels/Shlex.lean`.
- **Properties.**
  - **S1 (Lean)** `pySplit_eq_split: ∀s. pySplit s = split s`. The one-pass state machine snp implements equals the transliteration of CPython's `read_token`/`__next__` (posix, `whitespace_split`, `commenters=''`), with the same tokens and both `ValueError`s.
  - **S2 (Lean)** `split_quote: ∀s. split (quote s) = .ok [s]`.
  - **S5 (Lean)** `split_join: ∀ts. split (join ts) = .ok ts`, via the helper lemmas `quote_run` and `join_run`. `shlex-model` serves `join` too.
  - **S3 (tested per PR in the advisory `lean` job; skipped, and so passing, in required CI)** `snp_pycompat::shlex::split` agrees with `shlex-model` on all 335,923 strings over `{' ', '\t', '\'', '"', '\\', 'a'}` up to length 7, plus seeded random inputs.
  - **S3 detail.** `crates/snp-pycompat/tests/lean_oracle.rs` (std only; `cargo tree -p snp-pycompat -e normal,dev` stays empty) also runs 20,000 random `split` strings (seed `0x5eed000400000001`, 16 characters incl. CR, LF, `#`, NUL, non-ASCII and an emoji), all 37,449 `quote` inputs up to length 5 (checking `quote_bytes = quote` too) and 5,002 random `join` lists with a Rust-side `split(join(args)) == args` check.
  - **S4 (run by hand; nightly planned, §7)** `xtask/oracle/py_vs_lean.py` compares the model with CPython 3.12 `shlex` (split exhaustive to length 7, quote to length 5, random split/quote/join; 413,372 cases locally) and the eof model with pre-commit-hooks v6.0.0 `fix_file` (39,524 cases locally).
  - The anchors pin `split`, `quote`, `join` and the helpers they call: `is_ws`, `St`, `Esc`, `is_safe`, `quote_bytes`.
- **G:** argv parity with CPython for every string, at model level; the Rust code equals the model on the tested domain. **¬G:** a proof about the Rust `split`/`quote`/`join` (the Rust–model link is tested, not proved); a proof that the model equals CPython (checked by hand, S4); Python lone surrogates, which cannot reach `&str` (§10 row 14).

**Tooling gaps (M0/M1), stated so nothing above is over-read:**
- **V2 end to end.** Removing `#[verus_verify]` from the K1 mount still verified `partition_lens`, because it carries `verus_spec`, so V2 had nothing to report. V2's failure path is covered only by the `manifest_gaps` unit test and by G1, which requires `#[verus_verify]` on every mount.
- **G9 is syntactic.** It allows multi-segment paths rooted in `std`/`core`/`alloc`/`self`/`Self`, prelude or primitive names, or names the file declares. A local item shadowing an external crate name, or a glob re-export from std, is not modelled; the real backstop is that a non-std path does not resolve under the harness crate root.
- **G8 matches impls by last path segment** (`Type: Trait`), so same-named traits or types in different modules would be conflated. No kernel trait exists yet; it is tested on fixtures only.
- **CI not yet exercised.** `proofs.yml` passed actionlint and its path filter was tested locally, and first ran on GitHub in PR #18, where lean-action's own `axiom-audit`/`leanchecker` steps failed on the module name and were turned off in favour of `cargo xtask proofs lean`. `cargo xtask proofs oracles` has run end to end locally with both models.
- **Installer platforms.** Only the x86-linux Verus zip and the x86_64-unknown-linux-gnu verusfmt are sha256-pinned; `proofs-tools.sh install` supports only x86-linux and macOS and fails with "no pinned sha256" until hashes are added. Elsewhere (Linux arm64, Windows) put the pinned `verus` on `PATH`: `cargo xtask proofs verus` falls back to it and still checks the version.
- **Spec mutants.** A mutant may name the kernel's spec (`file = …`); K2 registers two (P5b). The other spec mutants of K3 were run by hand and are not registered.
- **Driver and test bodies.** Fingerprints make every edit to an oracle driver or oracle test visible, but whether a driver calls the registered model functions, and whether a test asserts every answer, is still reviewed by a person.

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
- **K8 gc patch protection** (Verus; snp-store `maintenance.rs` `protection`, `journal_patch`, `Protection::covers`, `is_patch_or_sibling`, sweep step 4 and `clean`; snp-store `marker.rs` `path_from_json`, the same byte-string codec as snp-stash `journal.rs` `enc_bytes`/`dec_bytes`). F1 landed: the raw-byte `mentioned` search is gone and these properties are stated over the decoded journals.
  - **G1** `covers(Patches(P), name) ⇔ ∃p∈P: name = p ∨ name = p ++ "." ++ s` for some `s`, and `covers(All(_), name)` always holds. `patch<s>-<pid>-<n>` is not covered by `patch<s>-<pid>`.
  - **G2** deletable ⇒ the name starts with `patch`, its age is at least the threshold (30 days for sweep, 1 hour for clean), and it is not covered.
  - **G3** adding journals never makes a kept patch deletable (monotonic: a new journal adds a name or turns the protection into `All`).
  - **G4** an unrestored journal whose `patch` is `dir/n` (a JSON string, or `{"hex": …}` of any bytes on Unix) protects `n` and its `n.*` siblings, whatever the journal's size up to snp-stash's 64 MiB journal limit; a bigger, unreadable or malformed journal (empty, no string `state`, or an unrestored state without a `patch`) protects everything. `clean`'s re-read before each removal reuses the decoded result of a journal whose `(len, mtime)` is unchanged; a stale entry is sound because a journal only starts naming its run's fresh patch (younger than both age limits) and later states only drop protection. The proof rests on `unhex(hex(b)) = Some(b)` and on the trusted serde_json decoder.
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
| 1 | Verus (VIR→SMT encoding, `rust_verify`) and Z3 4.16.0 | sha256-pinned bundle; `--no-cheating`; registered mutants must fail; a seed sweep is planned (§7), not running |
| 2 | vstd's specs of the std pieces used (slice, `Vec`, `Option`/`Result`, integer ops, `Range`) | small std surface (K6); every test, fuzz target and difftest runs on the shipped build |
| 3 | rustc 1.99.1 front end vs 1.100.0 codegen; vstd prelude glob visible only under Verus | kernel files stay in the common subset; an ambiguity is a compile error; spec items are imported by name and never under a prelude or std name (G3) |
| 4 | Ghost-only differences between the verified and the shipped file | G2–G5, G9, V2; kernels have no macros (G3) and load no other file (G9), so their meaning does not depend on the parent module, and every file under `proofs/verus/` is scanned |
| 5 | Mounts are the production files; nothing is silently skipped | G1, mount-level `#[verus_verify]`, V2 manifest |
| 6 | Preconditions at unverified call sites | none allowed (G6) |
| 7 | `assume_specification`/`external_body` | none in kernel files (G4, G5, root `forbid(unsafe_code)`); none at all in M1; later only in `proofs/verus/trusted/`, listed |
| 8 | Production impls of kernel traits (memchr, memmem, HashSet; from M2) | `[[trusted]]` + G8 + differential test against the verified reference implementation |
| 9 | Compiler-derived impls (`external_derive`) | verified code does not use them (`match`, never `==`) |
| 10 | Shells: I/O, `OsStr` and `Path` conversion, error mapping, iterator glue | a few lines each, monotone where security-relevant, existing tests, fuzz and difftest; doc comment "unverified shell of kernel X" |
| 11 | Spec ≠ intent (hand transcriptions of Python). The specs are trusted: a spec and code wrong in the same way verify | uniqueness lemmas; independent property lemmas; Lean `pyX = X` theorems with CPython cross-checks; oracle tests; K2's exhaustive comparison with the former check; mutants; `file:line` citations |
| 12 | Lean kernel and the three standard axioms | `--wfail`, axiom allowlist, `leanchecker` |
| 13 | Lean compiler and runtime behind the oracle executables; the drivers and oracle tests | oracles are test oracles only; L5 rejects `implemented_by`/`extern`/`unsafe` in the elaborated package; theorem statements are pinned (L5); driver and oracle-test fingerprints (G10) make any edit a visible re-review; that a driver runs the proved functions and a test asserts every answer is checked by review only |
| 14 | Model ↔ Python | transliteration plus `py_vs_lean`, run by hand (nightly planned, §7). Documented gap: Python `str` admits lone surrogates; Lean `Char` and Rust `char` do not. |
| 15 | Model ↔ Rust (no bridge exists) | bounded-exhaustive alphabets that cover every state transition, plus seeded random inputs; anchors |
| 16 | Model ↔ world (git, POSIX: K9) | named hypotheses, each with a witness test on the git floor (2.31) and the latest git |
| 17 | Allocation failure, abort, stack overflow | not modelled (ADR-066) |

---

## 11. Fix-first queue (ordinary PRs with tests and D-entries; proofs lock fixes in and never encode bugs)

- **F1. gc patch protection, D-DATA-04. Fixed** (CHANGELOG "Fixed"; D-DATA-04, D-DIAG-05): journals are decoded with the stash's byte-string codec and read in full, the `<patch>.*` siblings are protected, and a journal that cannot be read or understood keeps every patch with a stderr warning. What was confirmed by reading the code:
  - an unreadable journal protects nothing (`maintenance.rs:80`, `filter_map(… .ok())`);
  - `read_capped` truncates at the 1 MiB `META_CAP` (`fs.rs:262`, `marker.rs:19`), so a mention past the cap is missed;
  - a non-UTF-8 store path is journaled as `{"hex": …}` (`journal.rs:5,160-175`), so the raw patch name never appears.

  The fix: fail closed on read errors, a full read or protect-all over the cap, and decode journals with the shared codec. The owner approved protecting the `<patch>.*` siblings such as `.now`.
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
- `proofs/` skeleton: registry, root, spec lib, and the Lean package imported from the prototype (the `[[model]]` entries came with K3 and K4).
- `xtask proofs` with guard unit tests on fixture files; `proofs-tools.sh`.
- `proofs.yml` (advisory); `proofs check` in `xtask gates`.
- `check-unsafe` scope and `.gitignore`.

**M1: K1–K4 (12–16 days).** Effort per kernel: K1 2–3, K2 3–4, K3 4–5 (including the oracle test), K4 1–2, plus slack.

M1 exit criteria:
1. `cargo xtask proofs verus` is green with `--no-cheating`, the manifest is complete and the trusted list is empty; the root verification ("proofs verus: N verified … in X s", mutants excluded) takes under 15 s and the whole command, mutants included, under about 60 s.
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
