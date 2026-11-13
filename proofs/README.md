# proofs/

Machine-checked proofs for snp (ADR-095). The reference is [docs/VERIFICATION.md](../docs/VERIFICATION.md): what
is claimed and what is not (§0), the kernel rules K1-K10 (§2.3), the guards (§3) and the kernel table (§9).
Contributors who do not touch a kernel file need none of this: build, test, clippy and fmt are unaffected.

```
registry.toml          kernels, models, mutants and trusted items (the single machine-readable source, §3.1)
verus/kernels.rs       Verus root: mounts the production kernel files under crates/ unchanged
verus/spec/            specs and lemmas, one module per kernel; generic lemmas in spec/lib/
verus/run.sh           interim runner of the exact Verus command (later wrapped by `cargo xtask proofs verus`)
lean/                  Lake package SnpModels (core library only) and the oracle executables
lean/client.rs         std-only line-protocol client that home-crate oracle tests include via #[path]
```

## Install

- **Verus** `0.2026.11.02.8bf68ee` (pin and sha256 in `registry.toml`): unzip the release into a tools directory
  and put it on `PATH`. It needs the rustup toolchain `1.99.1` (`rustup toolchain install 1.99.1 --profile
  minimal`); the `verus` launcher selects it through `RUSTUP_TOOLCHAIN`, so never make it the rustup default.
  verusfmt `0.7.4` goes next to it. On the owner's machine: `. ~/.cache/snp-dev/tools/verus/env.sh`.
- **Lean** through elan; `proofs/lean/lean-toolchain` pins `leanprover/lean4:v4.34.1`, which elan fetches on the
  first `lake` call. Where `release.lean-lang.org` does not resolve, download the GitHub release tarball and run
  `elan toolchain link`.

## Run Verus

```sh
proofs/verus/run.sh                                  # whole root, --no-cheating; fails on any error or 0 items
proofs/verus/run.sh --min 10 --output-json out.json  # floor on the verified count, keep the JSON report
proofs/verus/run.sh --verify-module snp_proc_xargs_partition   # one mount, about 2 s (iteration only, see below)
proofs/verus/run.sh --mutant crates/snp-proc/src/xargs/partition.rs \
    'Some(t) => t <= max_len' 'Some(t) => t < max_len'          # succeeds only if the mutant FAILS to verify
proofs/verus/run.sh -- --expand-errors                # extra arguments go to verus
```

Verus refuses `--verify-module` together with `--no-cheating`, so a module run omits that flag and is an
iteration aid; the gate is the whole-root run. Kernel files are formatted by rustfmt (`cargo fmt`); the root and
`spec/` by `verusfmt` (`verusfmt --check proofs/verus/kernels.rs proofs/verus/spec/*.rs proofs/verus/spec/lib/*.rs`).

## Run Lean

```sh
cd proofs/lean
lake build --wfail                # --wfail is mandatory: a `sorry` is only a warning
lake env lean Axioms.lean         # every theorem: only propext, Classical.choice, Quot.sound
lake env leanchecker SnpModels    # independent re-check of the environment
```

Oracle tests in the kernels' home crates read the executables from `SNP_LEAN_ORACLES` and are skipped when it is
unset; `SNP_REQUIRE_LEAN_ORACLES=1` turns a skip into a failure:

```sh
SNP_LEAN_ORACLES=$PWD/proofs/lean/.lake/build/bin SNP_REQUIRE_LEAN_ORACLES=1 \
    cargo nextest run -p snp-pycompat --test lean_oracle
```

## Add a kernel (§8.4)

1. Fix any known bug first, with its own test and CHANGELOG entry.
2. Move the pure core into its own file next to the shell, behaviour-preserving. The first line is
   `//! Verified kernel (ADR-095): …`; follow rules K1-K10 (no `verus!` block, only the allowed
   `cfg_attr(verus_keep_ghost, …)` forms, `pub(crate)`, no `requires` on a non-private function, std-only code).
3. Write the spec and the property lemmas in `verus/spec/<kernel>.rs` (cite the upstream `file:line`) and add the
   module to `verus/spec/mod.rs`.
4. Mount the file in `verus/kernels.rs` with `#[verus_verify]` and a one-line `#[path = "../../crates/…"]`.
5. Add the `[[kernel]]` entry to `registry.toml` (callers, `verified_min`, at least one mutant) and check that
   every mutant fails with `run.sh --mutant`.
6. Add the G/¬G row to docs/VERIFICATION.md §9, and an oracle test if a Lean model exists.
