# 07 — Modern Rust (1.80 → 1.99, edition 2024) and crate ecosystem for `snp`

Status: research spec, 2026-10-07. Audience: engineers implementing **snp** ("You Shell Not Pass"), a Rust drop-in replacement for pre-commit 4.6.2 that also takes ideas from lefthook. Readers are not expected to re-read the Python/Go sources.

Conventions:
- `✔ verified` means a snippet was compiled and run locally with `rustup run 1.98.0 rustc --edition 2024` / `cargo` 1.98.0 (rustc `88d9e12ae 2026-08-18`, cargo `797e8a9bc 2026-08-05`, clippy `0.1.98`, rustfmt `1.9.0-stable`). Evidence lives under `scratchpad/tmp-rust-ecosystem/` (subdirectories `snippets/`, `yamlcmp/`, `emitcmp/`, `recmp/`, `shlexcmp/`, `ptytest/`, `tokiotest/`, `allocbench/`, `skeleton/`).
- Source citations are relative to `scratchpad/refs/` (for example `pre-commit/pre_commit/yaml.py:8`).
- Crate data (version, release date, downloads, MSRV, license) comes from the crates.io API on 2026-10-07. "dl" means all-time downloads.

---

## 0. TL;DR: decisions

| # | Decision | Why (evidence) |
|---|---|---|
| D1 | `edition = "2024"`, `rust-version = "1.98"` (MSRV). `resolver = "3"` is the edition-2024 default; set it explicitly at the root of a virtual workspace. Pin the toolchain to **1.99.0**, the latest stable (released 2026-10-01). 1.99 is a later release than the 1.98.1 vtable-miscompilation fix, but its notes do not restate that fix, so confirm it is included. Otherwise pin 1.98.1. CI verifies the MSRV with 1.98.0. | RELEASES.md: 1.98.0 (2026-08-20), 1.98.1 (2026-09-03, "fix miscompilation in generating vtables"), 1.99.0 (2026-10-01). |
| D2 | Every snp crate uses `unsafe_code = "forbid"`. Unsafe code exists only inside audited dependencies. | Verified: under `forbid`, `std::env::set_var` gives E0133 in edition 2024, and `#[unsafe(no_mangle)]` is flagged. |
| D3 | **YAML: `libyaml-safer 0.3.0`**, a safe Rust port of libyaml with parser and emitter. On top of it, snp's own `snp-yaml` layer reimplements PyYAML's SafeLoader composer, resolver and constructor (YAML 1.1) and SafeDumper's representer. **Do not use** serde_yaml, serde_yml, serde_yaml_ng, yaml_serde, serde-saphyr, saphyr or yaml-rust2 to load pre-commit configs. | Measured: libyaml-safer reproduced PyYAML CSafeLoader's scanner/parser error text **byte-for-byte on 14/14** syntax-error cases, and agreed on all 20 valid files. The only 6 mismatches were composer/constructor errors, which snp implements itself. Combined with a PyYAML resolver port, its emitter reproduced `yaml.dump(..., Dumper=CSafeDumper, indent=4, default_flow_style=False, sort_keys=False)` **byte-for-byte on 112/112 outputs**. Every serde YAML crate tested diverged on YAML 1.1 booleans, octal numbers or duplicate keys (§3.1). |
| D4 | **Regex: two tiers behind one Python-`re` translator (`snp-pyre`)** → 01 Addendum §A.1/§A.6, which supersedes the earlier "fancy-regex as the single engine" decision. **Tier 1** is `regex 1.13.1`: `regex::Regex` for config patterns, `regex::bytes` (`unicode(false)`) for pygrep, and the relaxed prefilters. **Tier 2** is `fancy-regex 0.19.2`, only for look-around, backrefs, conditionals, atomic groups and possessive quantifiers. `backtrack_limit = 100_000_000`. | Corpus: 4,858 real patterns. Tier 1 covers 99.1 % of config patterns. Both designs give 100 % match parity after translation, but fancy-only is 29 % slower on tier-1 patterns (9.38 vs 7.27 ms per pattern per 100k paths). fancy cannot express bytes ≥ 0x80 (pygrep). It caps nesting at 63. The sound tier-2 prefilter must be a `regex` automaton. The raw-engine gaps (look-around, `{,n}`, literal `a{`, `(?x)[a b]`) are closed by the translator, not by the choice of engine. |
| D5 | **shlex: own ~80-line port** of Python's `shlex.split` (posix=True, whitespace_split=True, no comments) and `shlex.quote`. | The `shlex 2.0.1` and `shell-words 1.1.1` crates differ from Python on `#`, and on `\$` and `` \` `` inside double quotes (§3.3). |
| D6 | **Async: `tokio 1.53.2`** (rt-multi-thread, process, signal, io-util, sync, time, fs, macros). **PTY and process groups: `rustix 1.1.5`**, which has safe APIs. **Windows job objects: `process-wrap 10.0.1`**. | Verified: a safe PTY capture equivalent to pre-commit's `cmd_output_p`, and process-group kill, both under `forbid(unsafe_code)`. |
| D7 | **File locking: `std::fs::File::lock`** (stable since 1.89). No fs2, fs4 or fd-lock. | Verified with strace: it uses `flock(fd, LOCK_EX)`, the same primitive as pre-commit (`pre-commit/pre_commit/file_lock.py:58`). On Windows it uses `LockFileEx` over the full range, which overlaps pre-commit's `msvcrt.locking` region. |
| D8 | **HTTP: `ureq 3.4.2`** (rustls with ring, platform verifier, gzip), called from `spawn_blocking`. Not reqwest. | The dependency closure is 37 crates for ureq vs 109–124 for reqwest 0.13.5. Downloads are rare. ureq reads `ALL_PROXY`/`HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` by default (`ureq-3.4.2/src/config.rs:948`). |
| D9 | **CLI: `clap 4.6.7`** (derive) with `infer_long_args = true`, which mirrors argparse prefix abbreviations. Never set `infer_subcommands`. Use custom error rendering where argparse parity matters. | Verified against real pre-commit: `run --all`, `--fi`, `--colo` and `--all-f` are accepted by both. `ru` is rejected by both. The ambiguous `--f` gives exit 2 in both, but the error text differs. |
| D10 | Release profile: `lto="fat"`, `codegen-units=1`, `opt-level=3`, **`panic="unwind"`**, **`overflow-checks=true`**, strip symbols for distribution. Use the **system allocator** (no mimalloc by default). | Measured: unwind adds 4.9% binary size and overflow checks add 1.4%; runtime differences were within noise. With unwind, Drop guards can restore stashed changes and a top-level `catch_unwind` can exit with code 3, like pre-commit. mimalloc gave no speedup on glibc and about 2× peak RSS (32 MB → 60–64 MB). |

---

## 1. Toolchain baseline

| Item | Value | Source |
|---|---|---|
| Rust 1.98.0 | released 2026-08-20 | RELEASES.md "Version 1.98.0 (2026-08-20)" |
| Rust 1.98.1 | 2026-09-03: "rustc: fix miscompilation in generating vtables" (rust#161441) | RELEASES.md |
| Rust 1.99.0 | 2026-10-01, **latest stable** at time of writing; LLVM 23 | RELEASES.md |
| Locally installed | `1.95.0` (default), `1.98.0`, `stable` = 1.96.1 | `rustup toolchain list` |
| Tools installed locally | cargo-deny 0.19.4 (latest is 0.20.2), cargo-nextest 0.9.133 (latest 0.9.146), cargo-llvm-cov 0.6.18 (latest 0.9.1), cargo-machete 0.9.1 | `--version` |
| Linker | Rust ≥1.90 uses `rust-lld` by default on `x86_64-unknown-linux-gnu`, so mold is not needed | RELEASES 1.90 |
| Tier changes | `x86_64-apple-darwin` was demoted to Tier 2 with host tools (1.90). `aarch64-pc-windows-msvc` was promoted to Tier 1 (1.91). `*-windows-gnullvm` became Tier 2 with host tools (1.91). | RELEASES 1.90/1.91 |

MSRV policy:
- `rust-version = "1.98"` in `[workspace.package]`. Resolver 3 (the 2024-edition default) then prefers dependency versions whose `rust-version` is ≤ 1.98.
- Every crate selected in §6 has an MSRV ≤ 1.97 (the highest are `process-wrap` at 1.87, `globset`/`ignore`/`zip` at 1.88, and `file-identify` at 1.94).

---

## 2. Rust language and std features, 1.80 → 1.99, relevant to snp

### 2.1 Language features

| Ver | Feature | snp relevance | ✔ |
|---|---|---|---|
| 1.80 | Exclusive range patterns (`0..10 =>`) | byte/char classifiers (YAML resolver first-char dispatch) | |
| 1.80 | `LazyLock`, `LazyCell` (std) | static regexes and tables; **replaces lazy_static and once_cell** | ✔ |
| 1.81 | `#[expect(lint, reason = "...")]`: like `allow`, but warns if the lint is not triggered | use with `clippy::allow_attributes = "warn"` so every suppression has to stay justified | ✔ |
| 1.81 | `extern "C"` functions abort on an uncaught panic | n/a (no FFI) | |
| 1.82 | Precise capturing `impl Trait + use<'a, T>` | narrows edition-2024 RPIT captures (iterators over config) | ✔ |
| 1.82 | `&raw const` / `&raw mut`, `unsafe extern {}` blocks, unsafe attributes `#[unsafe(no_mangle)]` | forbidden in snp; the `unsafe_code` lint covers unsafe attributes consistently since 1.98 | ✔ |
| 1.82 | Empty-type patterns can be omitted | match ergonomics on `Result<T, Infallible>` | |
| 1.83 | `&mut`, `*mut`, `&Cell` in const; raw lifetimes `'r#ident` | minor | |
| 1.84 | Next-gen trait solver for coherence; `Allow #[deny] inside #[forbid]` as a no-op | `forbid(unsafe_code)` at crate root plus local `deny` no longer conflicts | |
| 1.85 | **Edition 2024** (see §2.3) | baseline | ✔ |
| 1.85 | **Async closures** `async \|x\| ...` and `AsyncFn`/`AsyncFnMut`/`AsyncFnOnce` (in the prelude in all editions) | pass hook runners or retry policies as `impl AsyncFn(&Hook) -> Result<..>` | ✔ |
| 1.85 | `#[diagnostic::do_not_recommend]` | internal trait ergonomics | |
| 1.86 | **Trait upcasting** (`Box<dyn Sub>` → `Box<dyn Super>`) | plugin traits for languages, for example `dyn Language: Debug` | ✔ |
| 1.86 | `#[target_feature]` on safe fns | n/a | |
| 1.87 | Precise capturing in traits (`-> impl Iterator + use<'a, Self>`) | trait methods returning iterators | ✔ |
| 1.88 | **let chains** `if let A = x && let B = y && cond` (edition 2024 only) | flattens config validation | ✔ |
| 1.88 | `cfg(true)` / `cfg(false)` | | |
| 1.89 | Explicit inferred const args `_`; warn-by-default `mismatched_lifetime_syntaxes`; `repr(u128)` | lints | |
| 1.90 | Split of the diagnostic-attribute lints | | |
| 1.91 | Pattern bindings lowered in written order (drop order) | Drop-sensitive guards | |
| 1.92 | `unused_must_use` no longer warns on `Result<(), Uninhabited>` | | |
| 1.93 | Warn-by-default `const_item_interior_mutations` and `function_casts_as_integer` | | |
| 1.94 | Impls inherit the `dead_code` level of their trait; Unicode 17 | | |
| 1.95 | **`if let` guards** on match arms: `Pat if let Some(x) = f() && c => ...` | stage/hook-type dispatch | ✔ |
| 1.95 | `irrefutable_let_patterns` no longer fires on let chains | | |
| 1.96 | `expr` metavariable allowed in `cfg` | | |
| 1.97 | Allow-by-default **`dead_code_pub_in_binary`** lint | **enable it (warn)**: snp is a binary, so unused `pub` items are dead | ✔ (fired in the skeleton) |
| 1.97 | `cfg(target_has_atomic_primitive_alignment)` | | |
| 1.98 | `&mut` unsize coercion can shorten lifetimes in invariant positions; deny-by-default `invalid_runtime_symbol_definitions`; warn `c_void_returns`; **the `unsafe_code` lint now fires for every unsafe attribute** | | ✔ |
| 1.99 | `#[my_macro] mod foo;`; C-variadic definitions; `unreachable_cfg_select_predicates` is now in the `unused` group | | |

Pre-1.80 essentials still assumed: `let ... else` (1.65), `OnceLock` (1.70), `std::io::IsTerminal` (1.70), `CommandExt::process_group` (1.64), inline `const {}` blocks (1.79, ✔), `NonZero<T>` (1.79), `std::path::absolute` (1.79), RPITIT/async fn in traits (1.75), C-string literals `c"..."` (1.77), `#[diagnostic::on_unimplemented]` (1.78). **CVE-2024-24576 ("BatBadBut")** was fixed in 1.77.2: `Command` now refuses `.bat`/`.cmd` arguments it cannot escape safely on Windows.

### 2.2 std APIs (stabilized 1.80 → 1.99) worth using

**Sync, lazy and once**
- `LazyLock`/`LazyCell` (1.80). `LazyLock::get`/`get_mut`/`force_mut` (1.94, ✔; `get` returns `None` until the value is forced). `From<T> for LazyLock` (1.96).
- `OnceLock::wait`, `Once::wait` (1.86). `RwLockWriteGuard::downgrade` (1.92, ✔). `Atomic*::update`/`try_update` (1.95).

**Collections**
- `Vec::extract_if(range, pred)` (1.87, ✔; **takes a range argument**: `v.extract_if(.., |x| ..)`).
- `HashMap`/`HashSet::extract_if` (1.88, ✔); `BTreeMap`/`BTreeSet::extract_if(range, pred)` (1.91, ✔).
- `Vec::pop_if` (1.86, ✔); `Vec::push_mut`/`insert_mut` (1.95, ✔); `VecDeque::pop_front_if`/`pop_back_if` (1.93); `VecDeque::retain_back` (1.99).
- `get_disjoint_mut` for slices and `HashMap` (1.86, ✔); `slice::split_off*` (1.87); `as_chunks` (1.88); `array_windows` (1.94, ✔); `element_offset` (1.94); `as_array` (1.93).
- `hash_map::Entry::insert_entry` (1.83); `btree_map::Entry::insert_entry` (1.92).
- `Vec::into_parts`/`from_parts` (1.99); `IntoIterator for Box<[T; N]>` (1.99).
- `iter::repeat_n`, `is_sorted*` (1.82, ✔); `core::iter::chain` (1.91); `Peekable::next_if_map` (1.94).

**Option, Result and bool**
- `Option::take_if` (1.80, ✔); `Option::is_none_or` (1.82, ✔); `Option::get_or_insert_default` (1.83, ✔).
- `Result::flatten` (1.89, ✔).
- **`bool::ok_or` / `bool::ok_or_else`** (1.98, ✔): `cond.ok_or(Error)?`.

**Strings and text**
- `str::split_at_checked`, `trim_ascii*` on `str`/`[u8]` (1.80, ✔).
- `String::extend_from_within` (1.87); `str::from_utf8` as an inherent method (1.87).
- `str::floor_char_boundary`/`ceil_char_boundary` (1.91, ✔): truncate output without panics.
- `fmt::from_fn` (1.93, ✔): lazy `Display` without temporary `String`s.
- `String::from_utf16le/be(_lossy)` (1.98, ✔).
- **`str::strip_circumfix`** (1.98, ✔): `"'x'".strip_circumfix('\'', '\'')`.
- **`str::substr_range`** (1.98, ✔). Gotcha: it returns `core::range::Range<usize>` (the new range type), not `std::ops::Range`. Read `.start`/`.end`.
- `core::fmt::NumBuffer` + `<int>::format_into` (1.98, ✔ via `core::fmt::NumBuffer`; the `std::fmt` re-export arrives only in 1.99): allocation-free integer formatting.
- `String::from_utf8_lossy_owned`, `FromUtf8Error::into_utf8_lossy` (1.99): convert hook output bytes without a second allocation.
- `char::MAX_LEN_UTF8` (1.93).

**Paths and OS strings**
- `OsStr::display`/`OsString::display` (1.87, ✔).
- `Path::file_prefix` (1.91, ✔; `"archive.tar.gz"` → `"archive"`); `PathBuf::add_extension`/`with_added_extension` (1.91, ✔).
- `PartialEq<str>`/`PartialEq<String>` for `Path`/`PathBuf`, and the reverse (1.91, ✔).
- `std::env::home_dir`: undeprecated in 1.87. Since 1.85 it ignores `$HOME` on Windows. Since 1.90 it falls back on Unix when `HOME=""`. Python's `expanduser` differs when `HOME=""` (see §7.20).

**Filesystem, I/O and process**
- `fs::exists` (1.81; returns `io::Result<bool>`, unlike `Path::exists`, ✔).
- New `io::ErrorKind` variants (1.83): `IsADirectory` (✔), `NotADirectory`, `DirectoryNotEmpty`, `ReadOnlyFilesystem`, **`ArgumentListTooLong`** (E2BIG, relevant to xargs partitioning), **`ExecutableFileBusy`** (ETXTBSY when executing a just-written script), `ResourceBusy`, `StorageFull`, `FileTooLarge`, `TooManyLinks`, `NotSeekable`, `StaleNetworkFileHandle`, `Deadlock`, `NetworkDown`/`NetworkUnreachable`/`HostUnreachable`. `QuotaExceeded` and `CrossesDevices` came in 1.85.
- **`io::pipe()` → (`PipeReader`, `PipeWriter`)** with `From<PipeWriter> for Stdio` (1.87, ✔). Use it to merge child stdout and stderr into one ordered stream, like Python's `stderr=subprocess.STDOUT`.
- **`File::lock`/`lock_shared`/`try_lock`/`try_lock_shared`/`unlock`** + `TryLockError::{WouldBlock, Error}` (1.89, ✔).
- `Send`/`Sync for process::CommandArgs` (1.98).
- `std::fs::set_times`/`set_times_nofollow` (1.99).
- `IoSlice::advance*` (1.81); `BufRead::skip_until` (1.83); `Seek::seek_relative` (1.80).

**Numbers**
- `strict_*` arithmetic that panics on overflow (1.91, ✔); `checked_sub_signed` (1.90); `unbounded_shl/shr`, `cast_signed/unsigned`, `is_multiple_of` (1.87); `isqrt` (1.84); `midpoint` (1.85).
- `bit_width`, `highest_one`, `lowest_one`, `isolate_*` (1.97, ✔); `NonZero::from_str_radix` (1.98).

**Errors, panics and macros**
- `core::error::Error` (1.81); `PanicHookInfo` (1.81); `PanicHookInfo::payload_as_str` (1.91).
- **`cfg_select!`** (1.95, ✔): replaces the `cfg-if` crate.
- **`std::assert_matches!`/`debug_assert_matches!`** (1.96, ✔). The macro lives at `std::assert_matches!`; `std::assert_matches::assert_matches!` does **not** resolve on 1.98.
- `hint::cold_path` (1.95); `hint::select_unpredictable` (1.88); `{core,std}::derive` (accepted 1.98, MSRV 1.96).

**Other**
- `Duration::from_mins`/`from_hours` (1.91, ✔); `Duration::from_nanos_u128` (1.93).
- `bool: TryFrom<{integer}>` (1.95); `TryFrom<char> for usize` (1.94).

### 2.3 Edition 2024: what changes for snp

| Area | Change | snp implication |
|---|---|---|
| Language | RPIT captures **all** in-scope generic params, including lifetimes | Use `+ use<'a>` to return iterators that must not borrow `&self` (✔ verified) |
| Language | `if let` scrutinee temporaries are dropped **before** `else` | `if let Some(x) = mutex.lock()?.get(..) {..} else { mutex.lock() }` no longer deadlocks. Since 1.91 this also covers `pin!`/`format_args!` temporaries. |
| Language | Tail-expression temporaries are dropped before locals | Guard-ordering changes; review `Drop` side effects |
| Language | Match ergonomics reservations (no `mut`/`ref` under default binding modes) | |
| Language | `unsafe extern` blocks and unsafe attributes required; `unsafe_op_in_unsafe_fn` warns | moot (unsafe is forbidden) |
| Language | References to `static mut` are denied | moot |
| Language | Never-type fallback is `!`, not `()` | `panic!()` in generic positions infers differently |
| Language | Macro `expr` fragment also matches `const {}` and `_`; a missing fragment specifier is an error | |
| Language | `gen` is a reserved keyword (✔ verified error); `#"..."#` and `##` are reserved syntax | do not name variables `gen` |
| std | `std::env::set_var` and `remove_var` are `unsafe` (✔ E0133 under `forbid`) | **Never mutate the process environment.** Pass env through `Command::env`/`env_remove`/`env_clear`. Enforce with clippy `disallowed-methods`. |
| std | `CommandExt::before_exec` is unsafe (`pre_exec` always was) | forbidden. `process_group(0)` covers the group use case safely. |
| std | `Future`/`IntoFuture` in the prelude; `Box<[T]>` has a by-value `IntoIterator` | |
| Cargo | `resolver = "3"` is the default, giving MSRV-aware resolution (`incompatible-rust-versions = "fallback"`) | Keep `rust-version` accurate |
| Cargo | Underscore key aliases (`dev_dependencies`, …) are removed | |
| Cargo | Unused inherited `default-features` is rejected. 1.99 adds per-member `default-features = false` override on edition 2024. | |
| Rustdoc | Combined (merged) doctests | faster doctests |
| Rustfmt | `style_edition = "2024"` (version-sorted imports, etc.) | |

### 2.4 Cargo features (1.80 → 1.99)

| Ver | Feature | Use in snp |
|---|---|---|
| 1.80 | `-Zcheck-cfg` always on; `[lints.rust.unexpected_cfgs] check-cfg = [...]` | declare custom cfgs |
| 1.82 | `cargo info` | |
| 1.83 | `CARGO_MANIFEST_PATH`; `package.autolib` | |
| 1.84 | **resolver v3** and the MSRV-aware resolver config | §1 |
| 1.85 | edition 2024; `CARGO_CFG_FEATURE` in build scripts | |
| 1.86 | Program-path config keys are replaced, not merged | |
| 1.87 | OSC 9;4 terminal progress; `cargo package --exclude-lockfile` | |
| 1.88 | Automatic cache garbage collection | |
| 1.89 | `cargo fix` default target selection; doctest cross-compilation | |
| 1.90 | Multi-package `cargo publish`; `http.proxy-cainfo` | |
| 1.91 | **`build.build-dir`**; `--target host-tuple` | CI cache layout |
| 1.93 | `cargo clean --workspace` | |
| 1.94 | **config `include`**; TOML 1.1 in manifests; `pubtime`; **`CARGO_BIN_EXE_<name>` also available at runtime** in tests | integration tests |
| 1.96 | A dependency can name both a git repo and a registry | |
| 1.97 | **`build.warnings = "deny"`** (env `CARGO_BUILD_WARNINGS=deny`); `resolver.lockfile-path`; `-m` | **Use in CI instead of `RUSTFLAGS=-Dwarnings`.** It does not invalidate the build cache. ✔ verified: it fails both rustc and clippy warnings (exit 101). |
| 1.99 | New built-in `debug` profile; per-member `default-features` override; warns on hyphenated lint names in `[lints]`; incremental compilation is disabled automatically when `CI` is set | |

### 2.5 Compatibility notes (1.80–1.99) that can bite

- 1.81: the new sort implementations may **panic** when `Ord` is not a total order. 1.98: `derive(PartialOrd)` takes a fast path when `Ord` is also derived, so a hand-written `Ord` must stay consistent with `PartialOrd`.
- 1.80: `std::io::StdoutLock` and `StderrLock` are no longer `Sync`.
- 1.92: `iter::Repeat::last/count` panic. 1.92: unwind tables are emitted even with `panic=abort` on Linux.
- 1.97: v0 symbol mangling is the default, so old profilers may fail to demangle. Linker output is warned.
- 1.97: `pin!` no longer deref-coerces.
- 1.98: `env::Vars`/`VarsOs` are `!Send`/`!Sync`. More characters are escaped in `Debug` output (affects snapshot tests). `assert_eq!` temporaries got a scope. Solaris `File::lock` now returns "unsupported" (illumos works since 1.91.1).
- 1.99: legacy integral modules (`std::i32::MAX`) are fully deprecated. Exhausted `RangeInclusive` state changed.

### 2.6 Verified feature probe (summary)

`scratchpad/tmp-rust-ecosystem/snippets/features.rs` compiles under `#![forbid(unsafe_code)] #![deny(warnings)]` with `--edition 2024 -O` on 1.98.0 and runs successfully. It exercises let-chains, if-let guards, async closures/`AsyncFn` (polled with `Waker::noop`), `LazyLock`/`OnceLock`, precise capturing (fn and trait), trait upcasting, `#[expect(.., reason)]`, `extract_if` for Vec/HashMap/BTreeMap, `pop_if`, `push_mut`, inline const, `is_none_or`, `take_if`, `Result::flatten`, `bool::ok_or`, `strip_circumfix`, `substr_range`, `floor_char_boundary`, `strict_add`, `Path::file_prefix`, `add_extension`, `OsStr::display`, `Cell::update`, `array_windows`, `fmt::from_fn`, `RwLockWriteGuard::downgrade`, `get_disjoint_mut`, `NumBuffer`, `cfg_select!`, `fs::exists`, `ErrorKind::IsADirectory`, `File::lock`/`try_lock`, and `io::pipe` merging child stdout and stderr.

Runtime output confirmed the following:
- `try_lock` on a second fd returns `WouldBlock`.
- A shell child printing to stdout and stderr through one pipe produced `out\nerr\n` in order.

Three compile errors were found and fixed during the probe. They are recorded in §2.2.

---

## 3. Crate selection

### 3.1 YAML (critical)

#### 3.1.1 What pre-commit actually does

| Use | Code | Requirement for snp |
|---|---|---|
| Load config and manifests | `Loader = getattr(yaml, 'CSafeLoader', yaml.SafeLoader)`; `yaml_load = partial(yaml.load, Loader=Loader)` (`pre-commit/pre_commit/yaml.py:8-10`). Used by `clientlib.py:89,274,549`, `languages/dart.py:52` and `migrate_config.py:26,41,121`. | PyYAML **SafeLoader semantics, YAML 1.1**, over **libyaml** (CSafeLoader exists in pip wheels: PyYAML 6.0.2 `__with_libyaml__ = True` in pre-commit's pipx venv) |
| Compose a node tree with marks | `yaml_compose = partial(yaml.compose, Loader=Loader)` (`yaml.py:9`). `migrate_config._migrate_composed` (`commands/migrate_config.py:58-112`) splices replacements using `node.start_mark.index`/`end_mark.index` and preserves `node.style` (`:49-50`). | event or node API with spans and scalar style |
| Dump | `yaml.dump(o, Dumper=CSafeDumper, default_flow_style=False, indent=4, sort_keys=False)` (`yaml.py:11-19`). Used by `try_repo.py:65`, `autoupdate.py:136` (whole-file reformat fallback) and `autoupdate.py:148` (`yaml_dump({'rev': rev}, default_style=match[3])`, which re-quotes a `rev:` line). | PyYAML representer plus **libyaml emitter**, byte-identical |
| Errors | cfgv wraps `str(exc)`: `load_from_filename` raises `ValidationError(str(e))` (`cfgv/cfgv.py:444-447`). Observed output: see below. | reproduce `MarkedYAMLError.__str__` text |

Observed with pre-commit 4.1.0 on `entry: [unclosed`:

```
==> File .pre-commit-config.yaml
=====> while parsing a flow sequence
  in "<unicode string>", line 5, column 12
did not find expected ',' or ']'
  in "<unicode string>", line 6, column 1
```

The exit code is 1. The mark name is `"<unicode string>"` because a `str` was passed. Line and column are 1-based in the text.

Also observed:
- `always_run: yes` and `pass_filenames: off` **validate OK** in pre-commit, because they are YAML 1.1 booleans.
- `language_version: 3.10` fails with `=====> Expected string got float`.
- A duplicate `id:` key is silently accepted, and the last value wins.

#### 3.1.2 Candidates (crates.io, 2026-10-07)

| Crate | Version (date) | dl | MSRV | Unsafe | Maintainer / status | Verdict |
|---|---|---|---|---|---|---|
| serde_yaml | 0.9.34+deprecated (2024-03-25) | 428M | 1.64 | uses unsafe-libyaml | dtolnay, **deprecated** | ✘ |
| unsafe-libyaml | 0.2.11 (2024-03-17) | 310M | 1.56 | c2rust, pervasive (238 `unsafe` lines) | dtolnay, dormant; RUSTSEC-2023-0075 (unsound, fixed ≥0.2.10) | ✘ |
| serde_yaml_ng | 0.10.0 (2024-05-26) | 15M | 1.64 | on unsafe-libyaml | acatton; **stale since 2024-05** | ✘ |
| serde_norway / unsafe-libyaml-norway | 0.9.42 / 0.2.15 (2024-12-21) | 13M | 1.71 | c2rust | cafkafk; stale | ✘ |
| serde_yml | 0.0.13 (2026-05-27) | 24M | 1.85 | — | sebastienrousseau. **RUSTSEC-2025-0068 "unsound and unmaintained"** (its `libyml` dep: RUSTSEC-2025-0067). 0.0.13 is a deprecation shim forwarding to `noyalib` | ✘ (ban) |
| noyalib | 0.0.54 (2026-10-07) | 0.7M | 1.86 | `forbid` in lib | same author; pre-1.0, eight releases in three weeks | ✘ (churn; YAML 1.2; resolves `0777` as 777) |
| **yaml_serde** | 0.10.7 (2026-08-18) | 2.9M | 1.82 | on **libyaml-rs 0.3.0** (c2rust of libyaml, MIT, 236 `unsafe` lines) | **The YAML Organization** (owner `ingydotnet`, YAML spec co-author); "serde_yaml maintained by The YAML Organization". **This is what cfgv-rs's optional `yaml_serde` feature uses.** | ✘ for config loading (YAML 1.2 core schema; errors on duplicates; heavy unsafe), acceptable elsewhere |
| saphyr / saphyr-parser | 0.1.0 (2026-09-19) | 2.3M / 5.9M | 1.85 | none in src | saphyr-rs (Ethiraric, davvid); YAML 1.2 test-suite compliant | ✘: `Marker::index` is char-based (docs contradict themselves); **does not strip the BOM** (key `"\u{feff}a"`); tags display as `tag:yaml.org,2002:!str` |
| yaml-rust2 | 0.13.0 (2026-09-11) | 63M | 1.85 | none | Ethiraric | ✘: YAML 1.2 core (`0777` → 777, `Null` → string); BOM kept in the first key; duplicate key is an error |
| yaml-rust | 0.4.5 (2021) | 191M | — | — | RUSTSEC-2024-0320 unmaintained | ✘ (ban) |
| granit-parser | 1.3.0 (2026-09-16) | 6.2M | 1.81 | `#![forbid(unsafe_code)]` | bourumir-wyngs (fork of saphyr-parser; used by serde-saphyr and prek) | **runner-up**: char and byte offsets, comment events, fuzzed; error texts differ from libyaml |
| serde-saphyr | 1.3.0 (2026-09-16) | 9.4M | 1.89 | `forbid` (4 lines mention `unsafe`) | bourumir-wyngs; used by prek | ✘ for configs: typed ("Rust types are the schema"). In typeless (`deserialize_any`) mode it is **over-eager**: `y`/`n`/`tRue`/`oFF` → bool, `nUll` → null, and a mapping **key** `y` became `true`. Duplicates error by default. Default `Budget` fails at 250,001 nodes. |
| **libyaml-safer** | **0.3.0 (2025-12-22)** | 81k | 1.64 | `#![deny(unsafe_code)]` with one `#[allow(unsafe_code)]`: `from_utf8_unchecked` after `valid_up_to()` (`src/reader.rs:50-86`, sound) | simonask; repo pushed 2025-12-22; 3 open issues; license field "non-standard" (license file = MIT) | **✔ chosen** |

#### 3.1.3 Empirical comparison (resolution semantics; 29 corpus files)

The corpus is in `tmp-rust-ecosystem/yamlcases/`. Reference: PyYAML 6.0.2 `CSafeLoader`. Rust crates were read through a generic serde `deserialize_any` visitor (`yamlcmp/src/main.rs`).

| Input | PyYAML (truth) | yaml_serde / serde_yaml_ng | serde-saphyr (typeless) | noyalib | yaml-rust2 |
|---|---|---|---|---|---|
| `yes`/`no`/`on`/`off`/`Yes`/`NO`/`On` | bool | **str** | bool | **str** | **str** |
| `y`/`n`/`tRue`/`oFF` | str | str | **bool** | str | str |
| `0777` | int 511 | **str "0777"** | int 511 only with `legacy_octal_numbers: true` (off by default) | **int 777** | **int 777** |
| `0o777` | **str** | int 511 | int 511 | int 511 | int 511 |
| `1_000`, `0x_1F`, `0b1_0` | int | **str** | mixed | **str** | **str** |
| `190:20:30` (sexagesimal) | int 685230 | str | str | str | str |
| `1e3`, `1.5e3`, `1.0e3`, `1e+3` | **str** (an exponent needs a dot *and* a sign) | float | float | float | float |
| `1.10` | float 1.1 | float | float | float | float |
| `Null`/`NULL` | null | null | null | null | **str** |
| `nUll` | str | str | **null** | str | str |
| `2001-12-14` | **date** | str | str | str | str |
| duplicate key `a: 1 … a: 3` | **last wins**, no error | **error** | **error** (`LastWins` option exists) | ok | **error** |
| merge `<<: *b` | supported; order `x, y` | needs `apply_merge()`; order `y, x` | key `<<` handled, but `y:` became **bool key** | ✔ | `<<` kept as a literal key |
| `!!binary aGVsbG8=` | bytes `hello` | str | "hello" | tagged | str |
| `!custom foo` | **error** "could not determine a constructor for the tag '!custom'" | tagged value | "foo" | tagged | "foo" |
| `a: =` | **error** (tag `yaml.org,2002:value`) | "=" | "=" | "=" | "=" |
| `---` second document | **error** | error | error | error | 2 docs |
| `? [a, b]` key | **error** "found unhashable key" | accepted | accepted | accepted | accepted |
| UTF-8 BOM | stripped | stripped | stripped | stripped | **kept in key** |

Conclusion: **no ready-made Rust YAML library reproduces PyYAML's YAML 1.1 SafeLoader.** A drop-in replacement must accept `pass_filenames: no` and must not turn `id: y` into a bool. snp therefore owns the resolver and constructor.

#### 3.1.4 Empirical comparison (scanner and parser errors)

`yamlcmp/src/bin/liberr.rs` formats `libyaml_safer::Error` like PyYAML's `MarkedYAMLError.__str__`, using the context, context mark, problem and problem mark, with CParser-style marks `  in "<unicode string>", line L+1, column C+1`. The context mark is omitted when it equals the problem mark.

Results across the 40 UTF-8 corpus files:
- **All 14 scanner and parser errors were byte-identical** to PyYAML CSafeLoader: files 14, 15, 19, 30, 31, 32, 34, 35, 36, 37, 38, 39, 40, 42. These cover tab indentation, unclosed flow sequences and mappings, bad indent, unterminated quoted scalar, missing block-collection `-` indicator, `@` and backtick tokens, `a: b: c`, unknown directive and bad escape.
- All 20 non-error files also agreed. This includes the `%YAML 1.2` directive file: libyaml accepts it, and PyYAML still resolves `yes` as bool.
- The only 6 differences were composer and constructor errors (files 08, 09, 10, 16, 18, 33). These must be implemented by snp; see the table below.

Composer and constructor error texts to implement (exact strings from PyYAML 6.0.2 + CParser):

| Situation | context (line 1) | context mark | problem | problem mark |
|---|---|---|---|---|
| second document | `expected a single document in the stream` | first DocumentStart | `but found another document` | second DocumentStart (line of `---`) |
| undefined alias | — | — | `found undefined alias` | alias mark |
| duplicate anchor | `found duplicate anchor; first occurrence` | first anchor node | `second occurrence` | second anchor node |
| unknown tag (also `=` → `tag:yaml.org,2002:value`) | — | — | `could not determine a constructor for the tag '!custom'` (Python `repr` of the tag) | node start |
| list or dict used as a mapping key | `while constructing a mapping` | mapping start | `found unhashable key` | key start |

These render as, for example, `found undefined alias\n  in "<unicode string>", line 1, column 4`.

#### 3.1.5 Emitter parity (verified)

`emitcmp/` contains a ~60-line PyYAML representer that computes `plain_implicit`/`quoted_implicit` with the resolver regexes below. It feeds `libyaml_safer::Emitter` with `set_indent(4)`, `set_unicode(false)`, a block mapping and sequence style, and `ScalarStyle::{Any, SingleQuoted, DoubleQuoted}` for `default_style` `None`/`"'"`/`'"'`.

The result matched Python's output **identically for all 112 lines**: 37 `rev` strings × 3 styles, plus a nested try-repo-like config. The rev strings include `1.0` → `'1.0'`, `2020-01-01` → `'2020-01-01'`, `yes` → `'yes'`, `0777`, `null`, the empty string → `''`, `é` → `"\xE9"`, `#hash`, `a #b`, `x"y'z` → `'x"y''z'`, and leading/trailing spaces.

Example output: `repos:\n-   repo: /tmp/x y\n    rev: '1.0'\n    hooks:\n    -   id: flake8\n    -   id: "\xE9"\n        args:\n        - --max=1\n        - 'yes'\n        - ''\n        always_run: true\n        n: 3\n`. Note the indentless sequences under mapping keys, and `-   ` (dash plus 3 spaces) for top-level items with indent 4.

#### 3.1.6 PyYAML YAML 1.1 implicit resolver (port exactly)

Resolution rules:
- Dispatch on the **first character** of the plain scalar. The empty string uses key `''`.
- Within a first character, try the resolvers in registration order: bool, float, int, merge, null, timestamp, value.
- The first full match wins. Otherwise the tag is `tag:yaml.org,2002:str`.
- Only **plain** scalars without an explicit tag, or with the non-specific `!` tag, are resolved. Quoted scalars are always str.

Source: `site-packages/yaml/resolver.py` (PyYAML 6.0.2).

| Tag | First chars | Regex (written **without** `(?x)`; see warning) |
|---|---|---|
| bool | `yYnNtTfFoO` | `^(?:yes\|Yes\|YES\|no\|No\|NO\|true\|True\|TRUE\|false\|False\|FALSE\|on\|On\|ON\|off\|Off\|OFF)$` |
| float | `-+0123456789.` | `^(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?\|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?\|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*\|[-+]?\.(?:inf\|Inf\|INF)\|\.(?:nan\|NaN\|NAN))$` |
| int | `-+0123456789` | `^(?:[-+]?0b[0-1_]+\|[-+]?0[0-7_]+\|[-+]?(?:0\|[1-9][0-9_]*)\|[-+]?0x[0-9a-fA-F_]+\|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)$` |
| merge | `<` | `^(?:<<)$` |
| null | `~nN` and `''` | `^(?:~\|null\|Null\|NULL\|)$` |
| timestamp | `0-9` | `^(?:[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]\|[0-9][0-9][0-9][0-9]-[0-9][0-9]?-[0-9][0-9]?(?:[Tt]\|[ \t]+)[0-9][0-9]?:[0-9][0-9]:[0-9][0-9](?:\.[0-9]*)?(?:[ \t]*(?:Z\|[-+][0-9][0-9]?(?::[0-9][0-9])?))?)$` |
| value | `=` | `^(?:=)$`; this has **no constructor**, so loading errors |

⚠ The Python sources use `re.X`. Python keeps whitespace inside character classes in verbose mode (the timestamp's `[ \t]`). Rust's `regex` `(?x)` **drops** it (verified in §3.2). Never port these regexes with `(?x)`.

⚠ Python's `$` also matches before a trailing `\n`. That is irrelevant here, because plain scalars never end in a newline.

Constructor notes (SafeConstructor):
- **bool**: case-insensitive table lookup.
- **int**: remove `_`; handle the sign; `0b`, `0x`, leading-`0` octal and base-60 forms. Ints are **arbitrary precision**, so store values beyond i64 as BigInt or text.
- **float**: remove `_`, lowercase; `.inf`/`.nan`; base 60.
- **timestamp**: `date` or `datetime` with timezone. cfgv error messages print `date`/`datetime` type names, so `rev: 2020-01-01` must give "Expected string got date".
- **binary**: base64 decode, giving bytes.
- **set, omap, pairs**: supported.
- **merge**: `flatten_mapping` puts merged keys **first**, then the node's own keys. Own keys override and keep their position, matching Python dict semantics. Errors: "expected a mapping or list of mappings for merging, but found …" and "expected a mapping for merging, but found …" (context "while constructing a mapping").

Mapping semantics:
- Duplicate keys: **last value wins and the first position is kept**. `IndexMap::insert` has exactly these semantics.
- Python key equality means `1 == 1.0 == True` collapse into one dict key (see Open questions).

#### 3.1.7 Marks: bytes vs chars (verified)

For `a: "é"\nb: x`, the scalar `b` has:
- libyaml-safer `Mark { index: 8, line: 1, column: 0 }`, where **index is a BYTE offset**.
- PyYAML `start_mark.index = 7`, a **char** offset.

Columns are char-based in both, so error messages match. For `migrate-config` splicing, use libyaml-safer byte offsets directly on the Rust `&str`, which is simpler than Python. Only convert to char offsets if a char index must ever be displayed.

#### 3.1.8 Text decoding before YAML (cfgv parity)

Python cfgv reads with `open(filename, encoding='utf-8')` (`cfgv/cfgv.py:439`), which means:
- **Universal newlines**: `\r\n` and lone `\r` become `\n` before parsing.
- A BOM is kept as U+FEFF, and libyaml then skips it.
- Invalid UTF-8 raises `ValidationError(str(UnicodeDecodeError))`, for example `'utf-8' codec can't decode byte 0xff in position 0: invalid start byte`.

To map `std::str::Utf8Error` onto those messages, use `valid_up_to()` for the position and `error_len()` for the reason:
- `None` → `unexpected end of data`
- the bad byte is a continuation byte or an invalid lead (`0x80..=0xBF`, `0xC0`, `0xC1`, `0xF5..`) → `invalid start byte`
- otherwise → `invalid continuation byte`

Other call sites handle newlines differently: `migrate_config` uses default text mode, while `autoupdate._original_lines` uses `newline=''` and keeps line endings (`commands/autoupdate.py:125`). Mirror each call site.

#### 3.1.9 Performance (5.1 MB synthetic config; 560,009 events; 3 rounds)

| Library | Time |
|---|---|
| libyaml-safer (events) | 145–222 ms |
| granit-parser | 160 ms |
| saphyr-parser | 144–151 ms |
| yaml_serde → Value | 243–368 ms |
| serde-saphyr → Value (budget disabled) | 690–722 ms |

A typical 2 KB config parses in about 60 µs, so parser speed is not a factor.

#### 3.1.10 Risk management for libyaml-safer

The crate is small (7.3k LOC), MIT-licensed, and has a single maintainer with low adoption. Mitigations:
1. Hide it behind `snp-yaml`'s own event enum, with an adapter of about 100 lines. granit-parser is the fallback.
2. If upstream stalls, vendor or fork it via `[patch.crates-io]`. Replace the one `from_utf8_unchecked` with the safe `str::from_utf8(&buf[..valid_up_to])`; the cost is negligible.
3. Keep the PyYAML differential corpus in CI (§5).

cargo-deny reports `no-license-field` as a warning but still detects MIT from `LICENSE-MIT`, and the license check passes (verified).

#### 3.1.11 Where serde fits

- Use `serde` + `serde_json`/`toml` for snp-native formats only: JSON output, an optional `snp.toml` (lefthook-style), and internal state files.
- Do **not** deserialize `.pre-commit-config.yaml` with serde. The flow is: load with snp-yaml into `cfgv::Value` (via `load_strategy`, `cfgv-rs/src/load.rs:85-112`), validate and apply defaults with cfgv-rs schemas, then convert to typed structs by hand or through `serde::Deserialize` for `cfgv::Value`.

### 3.2 Regex (Python `re` → Rust)

pre-commit compiles `files`/`exclude` with `re.compile` and uses `.search` (`pre-commit/pre_commit/commands/run.py:65-69`). Validation goes through `cfgv.check_regex`, which raises `f'{v!r} is not a valid python regex'` (`cfgv/cfgv.py:384-388`; used by `clientlib.py:24,246-247,517-518`).

Matrix (✔ = matches in a substring search; python = CPython 3.12+ `re`):

| Pattern | Haystack | Python | `regex 1.13.1` | `fancy-regex 0.19.2` | Translator action |
|---|---|---|---|---|---|
| `(?P<n>..)` | | ✔ | ✔ | ✔ | none |
| `(?P=n)` backref | `xx` | ✔ | ERR | ✔ | none (fancy) |
| `(a)\1` | `aa` | ✔ | ERR | ✔ | none |
| `^(?!tests/).*\.py$` / `(?<=src/)` | | ✔ | ERR look-around | ✔ | none |
| `(?>a+)b` atomic, `a++b`/`a{1,2}+b` possessive | | ✔ | ERR / accepted as nested repeat | ✔ | none |
| `(a)?(?(1)b\|c)` conditional | `c` | ✔ | ERR | ✔ | none |
| `a(?#comment)b` | `ab` | ✔ | ERR | ✔ | none |
| `a{,3}b` | `aab` | ✔ | ERR | ✔ | none |
| `a{` literal | `a{` | ✔ | ERR | ✔ | none |
| `\.py\Z` | `a.py\n` | **false** (absolute end) | ERR | **true** (Oniguruma `\Z`) | **rewrite `\Z` → `\z`** |
| `\.py$` | `a.py\n` | **true** (before final `\n`) | false | false | keep (documented divergence; exact form `(?=\n?\z)` forces backtracking) |
| `(?x)[a b]` | space | **true** | **false** (whitespace dropped in class) | ✔ true | none with fancy; **never** pass `(?x)` patterns to `regex` |
| `(?x) a [ ] b` | `a b` | ✔ | ERR unclosed class | ✔ | none |
| `[[]` | `[` | ✔ | ERR | ERR | escape `[` inside a class → `[\[]` |
| `[a-z&&b]` | `&` | **true** (literal `&`) | false (intersection) | false | escape `&&`, `--`, `~~` inside classes |
| `\101`, `\0` (octal) | `A`, NUL | ✔ | ERR backref | ERR | rewrite to `\x{41}`, `\x{0}` |
| `\N{EM DASH}` | `—` | ✔ | ERR | **false, silently wrong** | reject with "not a valid python regex", or resolve via a Unicode-name table |
| `(?a)\w` ASCII flag | `é` | false | ERR | ERR | reject (rare), or rewrite the classes to `(?-u:\w)` |
| `a(?i)B` (flag mid-pattern) | | ERR (3.11+) | ✔ | ✔ | accept (lenient) |
| `\p{L}`, `\k<n>` | | ERR | ✔ / ERR | ✔ | accept (lenient) |
| `\d` on `٣` (Arabic-Indic digit) | | ✔ | ✔ | ✔ | none |

Performance (100,429 paths):
- `regex` takes 2.3–2.9 ms and fancy takes 2.7–3.3 ms for non-fancy patterns; fancy delegates to regex-automata.
- A lookahead pattern on fancy takes 29.8 ms.

Recommendations (**superseded → 01 Addendum §A.6**; the earlier "fancy-regex for everything" advice is withdrawn):
- One `PyRegex` type sits over two tiers:
  - **Tier 1** (`regex::Regex` / `regex::bytes::Regex`, `nest_limit(2048)`, size limits 64 MiB) for every pattern without backtracking-only constructs;
  - **Tier 2** (`fancy_regex`, `backtrack_limit(100_000_000)`) behind a mandatory tier-1 *relaxed prefilter*.
- `$`, `\b` and `\B` use a fast tier-1 form with a lazily built exact fancy fallback that is selected per haystack (§A.6.5).
- Cache compiled patterns by `(source, mode, options)`.
- `search(&str) -> Result<bool, MatchError>`. When the backtrack limit is exceeded, raise a fatal error that names the hook and pattern.
- Validation errors must print the **Python `repr`** of the pattern (§7.10).
- Do **not** use fancy `BytesMode::Ascii`: it cannot match bytes ≥ 0x80 written as escapes. Bytes tier 2 runs on the Latin-1 lane (01 Addendum §A.6.10). prek's `BytesMode::UnicodeBytes` + raw fancy (`crates/prek/src/config/pattern.rs:193-197`) is the anti-pattern: no translation, and `unwrap_or(false)` on errors.

### 3.3 shlex parity (verified divergences)

pre-commit splits entries with `cmd = shlex.split(entry)` (`pre-commit/pre_commit/lang_base.py:174-175`, `languages/r.py:183`). It quotes with `shlex.quote` (`clientlib.py:106,355`; `languages/perl.py:25-28`).

| Input | Python `shlex.split` | `shlex 2.0.1` | `shell-words 1.1.1` |
|---|---|---|---|
| `bash -c "echo \$HOME"` | `["bash","-c","echo \\$HOME"]` (**backslash kept**) | `echo $HOME` ✘ | same ✘ |
| `` bash -c "echo \`id\`" `` | backslashes kept | dropped ✘ | dropped ✘ |
| `echo #comment here` | `["echo","#comment","here"]` | `["echo"]` ✘ | `["echo"]` ✘ |
| `echo a\` | ValueError "No escaped character" | None ✔ | `["echo","a\\"]` ✘ |
| `echo 'unterminated` | ValueError "No closing quotation" | None | Err |
| `"a\"b" 'c\'d`, `"a\\b"`, `a\ b`, `$'..'`, unicode, tabs | (all identical) | ✔ | ✔ |

Python posix-mode rules to port:
- whitespace is `' \t\r\n'`; quotes are `'"`; escape is `\`; escaped quotes are `"`.
- Outside quotes, `\x` gives `x`.
- Inside `'…'`, everything is literal.
- Inside `"…"`, `\` escapes only `"` and `\`. Any other `\c` stays as the two characters `\c`.
- Empty quoted tokens are kept (`""` → `""`).
- Errors: `No closing quotation`, `No escaped character`.

`shlex.quote(s)`:
- `''` if `s` is empty.
- `s` itself if it contains no character outside `[\w@%+=:,./-]` (ASCII `\w`).
- Otherwise `'` + `s.replace("'", "'\"'\"'")` + `'`.

Use neither crate. Note that `shlex <1.3` had RUSTSEC-2024-0006.

### 3.4 CLI: clap

| Crate | Version (date) | MSRV | Notes |
|---|---|---|---|
| clap | 4.6.7 (2026-09-14) | 1.85 | features: `derive`, `env` (`PRE_COMMIT_COLOR`), `string`, `wrap_help` |
| clap_complete | 4.6.11 (2026-09-15) | 1.85 | `snp completions <shell>` (snp extension) |
| clap_mangen | 0.3.3 (2026-08-12) | 1.85 | man pages generated in xtask |

Parity facts verified against pre-commit 4.1.0 and clap 4.6.7:

| Input | argparse (pre-commit) | clap with `infer_long_args = true` |
|---|---|---|
| `run --all` / `--fi a b` / `--colo never --all-f` | accepted (unique prefix) | accepted ✔ |
| `run --f a` (ambiguous) | exit 2, `pre-commit run: error: ambiguous option: --f could match --files, --from-ref`, preceded by the full usage block | exit 2, `error: unexpected argument '--f' found` + tip |
| `ru` (subcommand prefix) | exit 2, `pre-commit: error: argument command: invalid choice: 'ru' (choose from 'autoupdate', 'clean', 'gc', 'init-templatedir', 'install', 'install-hooks', 'migrate-config', 'run', 'sample-config', 'try-repo', 'uninstall', 'validate-config', 'validate-manifest', 'help', 'hook-impl')` | exit 2, `error: unrecognized subcommand 'ru'` |

Configuration:
- `#[command(infer_long_args = true, disable_help_subcommand = true)]`. pre-commit has its own `help` subcommand, and `hook-impl` is hidden.
- Use `num_args = 0..` for `--files`, and `conflicts_with` for `--all-files` vs `--files`.
- For byte-exact argparse errors, intercept `clap::Error` (match on `e.kind()`: `UnknownArgument`, `InvalidSubcommand`, `InvalidValue`, …) and render an argparse-style `usage:` line plus `prog: error: msg`. Scope is an open question.

### 3.5 Async runtime and process management

| Need | Crate (version, date, MSRV) | Notes |
|---|---|---|
| runtime | **tokio 1.53.2** (2026-10-03, 1.71) | `default-features=false` + `rt-multi-thread, macros, process, signal, io-util, io-std, sync, time, fs`. Use `JoinSet` + `Semaphore` for bounded partition concurrency, and reassemble results **in input order** (✔ verified, `tokiotest/`). Use `kill_on_drop(true)` for cancellation and `tokio::signal::ctrl_c()` on all OSes. Use `spawn_blocking` for PTY reads, file locking and ureq. |
| cancellation | tokio-util 0.7.19 (2026-07-21) | `CancellationToken` for fail-fast. Always available; no feature flag needed. |
| ordered stream combinators | futures-util 0.3.34 (2026-08-11), `default-features=false, features=["std"]` | optional (`buffered`) |
| PTY, termios, process groups, signals, CPU affinity (Unix) | **rustix 1.1.5** (2026-09-16, 1.65) | features `std, process, pty, termios, fs, thread, stdio`. Safe APIs: `pty::{openpt, grantpt, unlockpt, ptsname}`, `termios::{tcgetattr, tcsetattr}`, `process::kill_process_group`, `thread::sched_getaffinity`. **✔ verified** PTY capture under `forbid(unsafe_code)` (`ptytest/`). |
| `sysconf(_SC_ARG_MAX)` | nix 0.31.3 (2026-05-11), `default-features=false, features=["feature"]` (Unix only) | for xargs parity (`xargs.py:49-58`) |
| tree kill / Windows Job Objects | **process-wrap 10.0.1** (2026-09-23, 1.87; watchexec) | `default-features=false`, `["tokio1","process-group","job-object","kill-on-drop","creation-flags"]`. Use **only** for lefthook-style `timeout`. The default keeps children in snp's process group and console, so terminal Ctrl-C reaches hooks exactly as with pre-commit. |
| CPU-bound parallel work | rayon 1.12.0 (2026-04-14, 1.80) | file classification and per-hook filtering over thousands of files |

Rejected:
- `nix` as the primary syscall crate: rustix covers more with safe APIs.
- `ctrlc`, `signal-hook`: tokio::signal covers the need.
- `os_pipe`: replaced by `std::io::pipe`.
- `portable-pty` 0.9.0 (2025-02): heavy and wezterm-centric.
- `command-group` 5.0.1 (2023): superseded by process-wrap.
- `duct`: unnecessary.
- `num_cpus`: see §7.13.

Verified PTY recipe, equivalent to `pre-commit/pre_commit/util.py:127-187`:

```rust
let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)?;
grantpt(&master)?; unlockpt(&master)?;
let slave = rustix::fs::open(ptsname(&master, Vec::new())?.as_c_str(),
                             OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC, Mode::empty())?;
let mut t = tcgetattr(&slave)?;
t.output_modes.remove(OutputModes::ONLCR | OutputModes::OPOST); // util.py:138
tcsetattr(&slave, OptionalActions::Now, &t)?;
let child = Command::new(..).stdin(Stdio::null())
    .stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave)).spawn()?;
// The Command (holding slave fds) must be dropped before reading master!
// Read master until Ok(0) (macOS) or Err(EIO) (Linux).
```

The verified output was `stdout-is-tty\nto-stderr\na\nb\n`, with no `\r\n`.

### 3.6 File locking

- **Use `std::fs::File::lock()`/`try_lock()`** (1.89+). Open the file with `OpenOptions::new().create(true).append(true)`, which mirrors Python's `open(path, 'a+')` (`file_lock.py:73`).
- Algorithm: call `try_lock()`. On `TryLockError::WouldBlock`, log `[INFO] Locking pre-commit directory` (`store.py:105-106`), then call blocking `lock()` inside `spawn_blocking`. Release with `unlock()`, or by dropping the File.
- Unix: `flock(LOCK_EX)`. Verified with `strace`: `flock(3, LOCK_EX) = 0`. This is the same primitive as pre-commit (`file_lock.py:58-61`), so pre-commit and snp exclude each other on a shared store.
- Windows: `LockFileEx(.., 0, u32::MAX, u32::MAX)` locks the whole range (`library/std/src/sys/fs/windows.rs:458-468`). pre-commit locks the 0xffff-byte region at the current position (`file_lock.py:17-25`). The regions overlap, so the two are mutually exclusive.
- Not needed: `fs4 1.1.0`, `fs2 0.4.3` (2018, unmaintained), `fd-lock 4.0.4`.

### 3.7 Filesystem, paths and matching

| Need | Choice | Version (date) | Notes |
|---|---|---|---|
| temp files and dirs | **tempfile** | 3.27.0 (2026-03-11) | `tempfile::Builder::new().prefix("repo").tempdir_in(store)` mirrors `mkdtemp(prefix='repo')` (`store.py:162`). Use `NamedTempFile::persist` for atomic writes. |
| fs errors that name the path | **fs-err** | 3.3.2 (2026-10-04), feature `tokio` | Internal errors only; pre-commit-visible messages are formatted by snp. |
| PATH lookup | **which** | 8.0.6 (2026-08-26) | Handles PATHEXT on Windows. pre-commit's `parse_shebang.find_executable` semantics may still need a ~40-line port for exact `Executable \`x\` not found`. |
| same-file check | same-file | 1.0.6 (2020) | stable and complete |
| Windows canonicalize without `\\?\` | **dunce** | 1.0.5 (2024-08) | `dunce::canonicalize`. Ban `std::fs::canonicalize` via clippy. |
| glob (lefthook `glob:`) | **globset** | 0.4.20 (2026-08-04, 1.88) | `literal_separator(false)` gives gobwas-like `*` (crosses `/`); `true` gives doublestar. `{a,b}`, `**`. `default-features=false` (drops `log`). |
| glob (alternative) | wax 0.7.0 (2026-01-29) | — | rejected: 3M dl vs 253M |
| directory walk | ignore 0.4.33 (2026-08-04) | — | pre-commit never walks the filesystem; it uses `git ls-files`/`git diff`. Only for snp extensions such as a non-git mode. |
| byte strings | **bstr** | 1.13.1 (2026-08-10), `default-features=false, features=["std"]` | git `-z` output and non-UTF-8 paths |
| byte search | **memchr** | 2.8.3 (2026-07-08) | split `-z` output with `memchr_iter(0, buf)`; NUL sniffing |
| normpath / relpath | **own code** (~40 lines) | — | `path-clean` 1.0.1 (2023) and `normpath` 1.5.2 do not match `posixpath.normpath` (for example, a leading `//` is preserved in Python) |
| XDG / home dirs | **none** | — | pre-commit uses `PRE_COMMIT_HOME` or `$XDG_CACHE_HOME/pre-commit` or `~/.cache/pre-commit` **on every OS** (`store.py:32-36`). Use `std::env::home_dir()`. `dirs 7.0.0`/`etcetera 0.11.0` are unnecessary. |

### 3.8 Hashing

| Need | Choice | Notes |
|---|---|---|
| content-addressed cache keys, file fingerprints (lefthook-style skip-if-unchanged) | **blake3 1.8.7** (2026-08-20) | fast, SIMD |
| download verification (go.dev JSON publishes sha256) | **sha2 0.11.0** (2026-03-25, 1.85) | |
| docker image tag parity `pre-commit-{md5(basename(prefix_dir))}` | **md-5 0.11.0** (2026-03-27) | `languages/docker.py:69-75` |
| in-memory hash maps | **foldhash 0.2.0** (`foldhash::fast::RandomState`, randomly seeded) or `rustc-hash 2.1.3` for trusted keys | prefer `IndexMap` wherever iteration order can reach output |
| rejected | xxhash-rust 0.8.19 (BSL-1.0), twox-hash 2.1.5, seahash 4.1.0 (2021), ahash 0.8.12 | not needed with blake3 + foldhash |

### 3.9 Errors and logging

- **thiserror 2.0.21** (2026-09-23, 1.77) for typed error enums in every library crate. Use one `SnpError` with `exit_code()`:
  - `FatalError` → 1, printed as "An error has occurred".
  - Unexpected error → 3, printed as "An unexpected error has occurred".
  - `KeyboardInterrupt` → 130, printed as "Interrupted (^C)".
  - Source: `pre-commit/pre_commit/error_handler.py:75-80`. The message format is `f'{msg}: {type(exc).__name__}: '` + str(exc) (`:25`).
  - Exit-code-3 errors write `{store}/pre-commit.log` (or snp's own store) with the `### version information`/`### error information` layout (`:44-66`).
- **anyhow 1.0.104**: only in xtask and tests.
- **miette 7.6.0** (2025-04-27; maintenance slowing): rejected. Its rich diagnostics conflict with pre-commit's text parity. It could be reconsidered for snp-native `snp.toml` errors.
- **tracing 0.1.44 + tracing-subscriber 0.3.23** (`env-filter`, `fmt`, `ansi`) for **developer diagnostics only**, on stderr, enabled by `SNP_LOG`/`RUST_LOG`.
- User-facing `[INFO]`/`[WARNING]`/`[ERROR]` lines are **not** tracing. They are written by snp's output module to **stdout**: `f'{level_msg} {message}'`. WARNING uses `\033[43;30m` and ERROR uses `\033[41m` when colored (`logging_handler.py:12-31`, `output.py:9-33`).

### 3.10 Terminal: colors, width, progress

pre-commit facts:
- Escape codes: `RED='\033[41m'`, `GREEN='\033[42m'`, `YELLOW='\033[43;30m'`, `TURQUOISE='\033[46;30m'`, `SUBTLE='\033[2m'`, `NORMAL='\033[m'` (`color.py:59-64`).
- `use_color('auto')` = `sys.stderr.isatty() and terminal_supports_color and os.getenv('TERM') != 'dumb'` (`color.py:93-100`). The decision uses **stderr**, even though output goes to stdout.
- On Windows, `terminal_supports_color` = success of enabling `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on **stderr** (`color.py:46-55`).
- `--color` defaults to `os.environ.get('PRE_COMMIT_COLOR', 'auto')` (`color.py:103-108`).
- `NO_COLOR` is **not** honored.
- All output goes to stdout as bytes, flushed after each write (`output.py:9-29`).
- There is no terminal-width logic. `cols = max(name_len + 3 + len('(no files to check)') + 1 + len('Skipped'), 80)` (`commands/run.py:238-249`).
- Hook-name width is `_len_cjk`: East-Asian-Width `W`/`F` count 2 and everything else counts 1 (`commands/run.py:36-38`).

Choices:
- Hard-code the six escape constants. Do not use anstyle, owo-colors or colored to render them: `anstyle` resets with `\x1b[0m`, not `\x1b[m`, which breaks byte parity.
- Use `std::io::IsTerminal` on stderr.
- **anstyle-query 1.1.5**: `windows::enable_virtual_terminal_processing()` is safe, but it enables stdout **and** stderr and fails if stdout is redirected. That is a parity gap; see Open questions.
- Width: generate a sorted `&[(u32, u32)]` table of EAW `W`+`F` ranges from Unicode's `EastAsianWidth.txt` in xtask, and look up with binary search. Pin the Unicode version used by the reference CPython (3.12 → Unicode 15.0; 3.13 → 15.1). **Do not** use `unicode-width`, whose rules differ (combining marks count 0, emoji presentation, ambiguous width).
- `terminal_size 0.4.4`: not needed for parity.
- `indicatif 0.18.6` (with `console 0.16.6`): optional, behind a non-default feature for snp-only UX. It must never be active in pre-commit-compatible output.

### 3.11 HTTP and archives (language bootstrap only)

| Need | Choice | Notes |
|---|---|---|
| HTTP | **ureq 3.4.2** (2026-09-13, 1.85) with `default-features=false, features=["rustls","platform-verifier","gzip"]` | 37 crates including tokio, vs reqwest 0.13.5 at 109 (minimal) or 124 (default). reqwest 0.13 defaults to rustls + **aws-lc-rs**, a heavier C build. ureq's `rustls` feature means rustls + **ring** + webpki-roots. Use `platform-verifier` so corporate CAs work. Proxies come from env by default. |
| TLS | rustls 0.23.45 (transitive) | **must be ≥ 0.23.45** (RUSTSEC-2026-0285). The ring "unmaintained" advisory RUSTSEC-2025-0007 was withdrawn 2025-02-22. |
| gzip | **flate2 1.1.10**, default backend `miniz_oxide` (`#![forbid(unsafe_code)]`) | `zlib-rs` is faster but uses unsafe internally, and speed does not matter for rare downloads |
| tar | **tar 0.4.46** (`default-features=false`, which drops `xattr`) | **≥ 0.4.45** required (RUSTSEC-2026-0067 symlink chmod, RUSTSEC-2026-0068 PAX size). Unpack with `Entry::unpack_in(dst)`, which returns `false` when a path escapes; never call `unpack`. |
| zip (Go on Windows) | **zip 8.6.0** (2026-04-25, 1.88), `default-features=false, features=["deflate-flate2"]` | The default features pull aes, bzip2, lzma, zstd and ppmd; `deflate` pulls zopfli. Use `enclosed_name()` (RUSTSEC-2025-0168 class of bugs). |
| xz / zstd | liblzma 0.4.8 (`default-features=false, features=["static"]`; the default `bindgen` needs libclang), zstd 0.14.0 | **only if** a supported language downloads `.tar.xz`/`.tar.zst`. Both are C libraries. Pure-Rust lzma-rs 0.3.0 is stale (2023). |
| async archive I/O | astral-tokio-tar 0.7.0, async_zip | rejected; synchronous I/O in `spawn_blocking` is simpler |

### 3.12 Data formats

| Need | Choice | Notes |
|---|---|---|
| serde | serde 1.0.229 (`derive`) | |
| JSON | serde_json 1.0.151 (`preserve_order`) | deterministic key order for `--format json` outputs |
| TOML | toml 1.1.6+spec-1.1.0, `default-features=false, features=["std","parse","display","serde","preserve_order"]` | for a lefthook-style `snp.toml`; `toml_edit 0.25.15` only if format-preserving edits are needed |
| JSONC / JSON5 | jsonc-parser 0.34.0 / json5 1.3.1 | only if lefthook JSON configs are supported |
| schema export | schemars 1.2.2 | optional, in xtask |

### 3.13 Collections and performance helpers

| Crate | Version | Use |
|---|---|---|
| **indexmap** | 2.14.2 (2026-09-05, 1.85) | config maps (insertion order = YAML order, required for messages and dumps); deterministic iteration |
| **smallvec** | 1.16.2 (`const_generics`) | tag sets and short arg lists (file-identify already uses it) |
| **rayon** | 1.12.0 | parallel classification and filtering when files × hooks is large |
| hashbrown | 0.17.1 | only through dependencies |
| compact_str 0.10.0, smol_str 0.3.6, ecow 0.3.1, arcstr 1.2.0 | — | **not needed**. Prefer `Box<str>` (immutable config strings) and `Arc<str>` (shared). Store file lists as one buffer plus ranges. |
| camino 1.2.6 | — | rejected: paths may be non-UTF-8 |
| memmap2 0.9.11 | — | rejected: unsafe, and SIGBUS if a file is truncated. Read ≤ 1 KiB headers with `File::read`. |

### 3.14 SQLite

- pre-commit's store keeps a `db.db` SQLite database with `repos(repo, ref, path)` and `configs(path)` tables (`store.py:6,115-120,217`).
- **Only if snp shares pre-commit's store** does it need `rusqlite 0.40.2` with `bundled` (C, ~1.5 MB).
- Recommendation: use snp's own store (`SNP_HOME`, then `$XDG_CACHE_HOME/snp`, then `~/.cache/snp`) with a JSON or TOML manifest per repo plus the lock file, and no SQLite. See Open questions.

### 3.15 Allocator (measured)

Workload (`allocbench/`):
- Walk 98,620 files, filter them through 30 hook regex pairs (rayon), tag them with file-identify, sniff NUL bytes in the first 1 KiB, and glob-match.
- Build: fat LTO, `codegen-units=1`. Machine: 22 cores, glibc. 4 alternating runs.

| Allocator | Wall | Max RSS |
|---|---|---|
| glibc (system) | 0.46–0.57 s | **32.0–32.6 MB** |
| mimalloc 0.1.52 | 0.49–1.09 s | **59.7–64.0 MB** |

Decision: **system allocator** on gnu, macOS and Windows. musl's allocator is known to be slow under contention, so for `*-linux-musl` release builds mimalloc can be enabled behind a `cfg(target_env = "musl")` feature **after** measuring in CI. tikv-jemallocator 0.7.0 and snmalloc-rs 0.7.6 were not considered further (C builds, no Windows/musl benefit shown).

### 3.16 Testing and benchmarking

| Crate / tool | Version (date) | Use |
|---|---|---|
| **insta** (`filters`, `redactions`, `json`) | 1.49.0 (2026-10-03) | snapshots of CLI output; filter temp paths, durations and versions |
| **insta-cmd** | 0.7.0 (2026-06-14) | `assert_cmd_snapshot!(Command::new(env!("CARGO_BIN_EXE_snp")) ...)` |
| **assert_cmd** / **assert_fs** / **predicates** | 2.2.2 / 1.1.4 / 3.1.4 | process-level tests in temp git repos |
| **rstest** (`default-features=false`) | 0.27.0 (2026-09-06) | table-driven cases (YAML corpus, regex matrix, shlex matrix) |
| **proptest** | 1.11.0 (2026-03-24) | round-trip and property tests: `py_shlex_split(py_shlex_quote(s)) == [s]`; YAML dump→load identity; resolver vs reference table |
| pretty_assertions | 1.4.1 (2024-09) | diffs |
| cargo-fuzz 0.13.2 + libfuzzer-sys 0.4.13 | — | nightly fuzz targets: snp-yaml loader, PyRegex translator, shlex, git `-z` parser |
| **criterion** (`default-features=false, features=["cargo_bench_support"]`) | 0.8.2 (2026-02-04) | micro-benchmarks. divan 0.1.21's last release was 2025-04. |
| **hyperfine** (tool) | 1.21.0 | end-to-end comparisons: snp vs pre-commit vs prek vs lefthook |
| cargo-nextest | 0.9.146 | test runner (process per test, retries, JUnit) |
| serial_test, temp-env | — | **not needed**: never mutate the process env; nextest isolates tests in processes |

### 3.17 Release and distribution

| Tool | Version | Use |
|---|---|---|
| **cargo-dist** (binary `dist`) | 0.32.0 (2026-05-22; repo active 2026-10-05) | GitHub Releases, shell/PowerShell installers, Homebrew tap, checksums, `[profile.dist]` |
| **maturin** | 1.15.0 (2026-08-24), `bindings = "bin"` | PyPI wheels, so `pip install`/`pipx`/`uvx` work like pre-commit (ruff, uv and prek do this). **PyPI name `snp` is taken** by a third-party placeholder (0.0.0, David Bonet), so pick another distribution name. |
| cargo-auditable | 0.7.7 | embed the dependency list in binaries (`cargo auditable build`) |
| cargo-cyclonedx | 0.5.9 | SBOM (optional) |
| cargo-zigbuild | 0.23.4 | glibc-version-pinned cross builds (alternative to native runners) |
| cross | 0.2.5 (2023) | rejected (stale) |
| actions/attest-build-provenance | v4.2.2 | SLSA provenance for artifacts |

crates.io: the `snp` crate (1.0.1, 2025-07-31, repo devops247-online/snp) is **owned by `grok-rs`**, the user, so a new major version can be published under that name.

### 3.18 Ban / avoid list (with reasons)

| Crate | Instead |
|---|---|
| serde_yaml, serde_yml, libyml, yaml-rust, unsafe-libyaml, serde_yaml_ng, serde_norway | libyaml-safer + snp-yaml |
| lazy_static, once_cell (direct use) | `std::sync::{LazyLock, OnceLock}`. They stay transitive deps (tracing-core, rustls, tempfile), so ban them in code review or clippy, not in cargo-deny. |
| cfg-if | `cfg_select!` (1.95) |
| os_pipe | `std::io::pipe` (1.87) |
| fs2, fs4, fd-lock | `File::lock` (1.89) |
| atty (RUSTSEC-2021-0145, RUSTSEC-2024-0375) | `std::io::IsTerminal` |
| num_cpus | `sched_getaffinity` / `available_parallelism` (§7.13) |
| ctrlc, signal-hook | `tokio::signal` |
| dirs, etcetera, directories | `std::env::home_dir` + pre-commit's path rules |
| openssl, native-tls | rustls |
| reqwest | ureq |
| mimalloc (default) | system allocator |
| shlex, shell-words (for parsing) | own port |
| unicode-width (for `_len_cjk`) | generated EAW table |
| anstyle/owo-colors for pre-commit output | exact escape constants |
| memmap2 | bounded reads |

---

## 4. Workspace configuration

### 4.1 Suggested crate layout

```
snp/
├─ Cargo.toml                 # [workspace], [workspace.package], [workspace.dependencies], [workspace.lints], profiles
├─ rust-toolchain.toml  rustfmt.toml  clippy.toml  deny.toml  .config/nextest.toml  .cargo/config.toml
├─ crates/
│  ├─ snp/           (bin "snp": CLI, dispatch, exit codes, panic → exit 3, interrupt → 130)
│  ├─ snp-pycompat/  (Python-parity helpers: shlex split/quote, repr(), UTF-8 error text, normpath/relpath, EAW width, xargs math)
│  ├─ snp-yaml/      (PyYAML SafeLoader/SafeDumper over libyaml-safer)
│  ├─ snp-pyre/      (Python re parser + translator → regex (tier 1) / fancy-regex (tier 2) + cache; 01 Addendum)
│  ├─ snp-config/    (cfgv schemas for config/manifest, migrate-config, autoupdate rewrite)
│  ├─ snp-git/       (git CLI wrapper, -z parsing, staged-files stash guard)
│  ├─ snp-exec/      (spawn, PTY, pipe merge, xargs partition, concurrency, timeouts, signals)
│  ├─ snp-store/     (cache dir, lock, clones, envs)
│  └─ snp-lang/      (language backends)
└─ xtask/            (dist helpers, completions, man pages, EAW table codegen, differential test runner)
```

Keep crates few enough to compile fast, but separate the Python-parity layers (pycompat, yaml, pyre) so they can be fuzzed and property-tested in isolation.

### 4.2 Root `Cargo.toml` (lints and profiles)

✔ Verified on a skeleton: zero unknown-lint warnings with clippy 0.1.98, and `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets --locked` passes. A deliberately invalid lint name was correctly reported (`warning[E0602]: unknown lint`). The restriction lints were confirmed to fire.

```toml
[workspace]
members = ["crates/*", "xtask"]
resolver = "3"

[workspace.package]
version = "2.0.0"
edition = "2024"
rust-version = "1.98"
license = "MIT"
repository = "https://github.com/grok-rs/snp"
publish = false            # override to true in crates/snp when publishing

[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"
unreachable_pub = "warn"
unused_qualifications = "warn"
unused_lifetimes = "warn"
redundant_lifetimes = "warn"
let_underscore_drop = "warn"
unit_bindings = "warn"
non_ascii_idents = "deny"
trivial_numeric_casts = "warn"
meta_variable_misuse = "warn"
dead_code_pub_in_binary = "warn"          # 1.97+
rust_2018_idioms = { level = "warn", priority = -1 }
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(coverage_nightly)"] }

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
nursery = { level = "warn", priority = -1 }
cargo = { level = "warn", priority = -1 }
# noisy group members turned off
multiple_crate_versions = "allow"
cargo_common_metadata = "allow"
module_name_repetitions = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
must_use_candidate = "allow"
too_many_lines = "allow"
similar_names = "allow"
option_if_let_else = "allow"
significant_drop_tightening = "allow"
redundant_pub_crate = "allow"
future_not_send = "allow"
# restriction picks (no panics on bad input, determinism, hygiene)
unwrap_used = "deny"
expect_used = "warn"
panic = "deny"
todo = "deny"
unimplemented = "deny"
unreachable = "warn"
dbg_macro = "deny"
print_stdout = "warn"                 # all output goes through snp's output module
print_stderr = "warn"
exit = "warn"
indexing_slicing = "warn"
string_slice = "warn"                 # byte-index slicing of &str can panic on char boundaries
get_unwrap = "warn"
unwrap_in_result = "warn"
panic_in_result_fn = "warn"
allow_attributes = "warn"             # force #[expect(.., reason = "..")]
allow_attributes_without_reason = "warn"
iter_over_hash_type = "warn"          # nondeterministic output order
mem_forget = "deny"
rc_mutex = "warn"
mutex_atomic = "warn"
lossy_float_literal = "warn"
float_cmp_const = "warn"
fn_to_numeric_cast_any = "warn"
format_push_string = "warn"
if_then_some_else_none = "warn"
infinite_loop = "warn"
let_underscore_must_use = "warn"
let_underscore_untyped = "warn"
create_dir = "warn"
verbose_file_reads = "warn"
filetype_is_file = "warn"
str_to_string = "warn"
try_err = "warn"
rest_pat_in_fully_bound_structs = "warn"
same_name_method = "warn"
self_named_module_files = "warn"
empty_drop = "warn"
empty_structs_with_brackets = "warn"
empty_enum_variants_with_brackets = "warn"
error_impl_error = "warn"
tests_outside_test_module = "warn"
unused_result_ok = "warn"
renamed_function_params = "warn"
clone_on_ref_ptr = "warn"
as_conversions = "warn"               # prefer From/TryFrom
needless_raw_strings = "warn"
redundant_type_annotations = "warn"
unnecessary_self_imports = "warn"
large_include_file = "warn"
partial_pub_fields = "warn"
multiple_inherent_impl = "warn"
disallowed_methods = "deny"
disallowed_types = "deny"

[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "unwind"            # Drop guards (stash restore) + catch_unwind → exit 3 (§7.2)
overflow-checks = true      # +1.4% size, runtime within noise (§4.3)
debug = "line-tables-only"  # usable backtraces in pre-commit.log-style crash logs
strip = "debuginfo"
incremental = false

[profile.dist]              # used by cargo-dist
inherits = "release"
strip = "symbols"

[profile.profiling]
inherits = "release"
debug = "full"
strip = "none"
lto = "thin"
codegen-units = 16

[profile.dev]
debug = "line-tables-only"
split-debuginfo = "unpacked"

[profile.dev.package."*"]   # dependencies optimized in dev and test (regex, YAML, insta)
opt-level = 1
[profile.dev.package.insta]
opt-level = 3
[profile.dev.package.similar]
opt-level = 3
[profile.dev.package.regex-automata]
opt-level = 3
[profile.dev.package.libyaml-safer]
opt-level = 3

[profile.test]
debug = "line-tables-only"
```

Each member crate uses `[lints] workspace = true`. Cargo cannot mix `workspace = true` with per-crate overrides in `[lints]`. A crate that needs a different level (for example a future `snp-sys` with audited Windows FFI) must copy the whole table and set `unsafe_code = "deny"`, then use `#[expect(unsafe_code, reason = "...")]` on each item.

### 4.3 Profile measurements

Same workload as §3.15, 3 rounds each.

| Variant | Binary size | Wall time |
|---|---|---|
| fat LTO, cu1, **panic=abort**, strip symbols | 2,153,720 B | 0.51–0.52 s |
| fat LTO, cu1, **panic=unwind**, strip symbols | 2,259,232 B (+4.9%) | 0.48–0.57 s |
| unwind + **overflow-checks** | 2,289,968 B (+1.4%) | 0.52–0.53 s |
| thin LTO, cu16 | 2,742,120 B (+21%) | 0.52–0.55 s |
| unwind, not stripped, `debug=line-tables-only` | 9,908,216 B | — |

Conclusion: the workload is I/O-bound, so safety options cost nothing measurable at runtime. Choose by size and safety.

### 4.4 `rust-toolchain.toml`

```toml
[toolchain]
channel = "1.99.0"          # latest stable 2026-10-01 (released after the 1.98.1 vtable fix); MSRV 1.98 enforced in CI
components = ["rustfmt", "clippy", "llvm-tools-preview"]
profile = "minimal"
```

Do not list release `targets` here; CI adds them per job. Listing them forces every contributor to download them.

### 4.5 `rustfmt.toml` (stable-only options)

```toml
edition = "2024"
style_edition = "2024"
max_width = 100
newline_style = "Unix"
use_field_init_shorthand = true
use_try_shorthand = true
```

✔ Verified: `imports_granularity` and `group_imports` print "unstable features are only available in nightly channel" on stable. Either omit them, or run `cargo +nightly fmt` in a separate optional CI job.

### 4.6 `clippy.toml`

✔ Verified: the file is picked up from the workspace root, and the disallowed method, disallowed type and `unwrap_used` diagnostics fired as errors.

```toml
msrv = "1.98"
allow-unwrap-in-tests = true
allow-expect-in-tests = true
allow-panic-in-tests = true
allow-dbg-in-tests = true
allow-print-in-tests = true
allow-indexing-slicing-in-tests = true
avoid-breaking-exported-api = false
disallowed-methods = [
  { path = "std::env::set_var", reason = "process-global and unsafe in edition 2024; use Command::env" },
  { path = "std::env::remove_var", reason = "process-global and unsafe in edition 2024; use Command::env_remove" },
  { path = "std::process::exit", reason = "skips destructors (stash restore!); return ExitCode from main" },
  { path = "std::fs::canonicalize", reason = "returns \\\\?\\ UNC paths on Windows; use dunce::canonicalize" },
  { path = "std::thread::sleep", reason = "blocks a tokio worker; use tokio::time::sleep" },
]
# optional strict mode: force deterministic maps
# disallowed-types = [
#   { path = "std::collections::HashMap", reason = "use indexmap::IndexMap where order can reach output" },
#   { path = "std::collections::HashSet", reason = "use indexmap::IndexSet where order can reach output" },
# ]
```

### 4.7 `deny.toml`

✔ Verified with cargo-deny 0.19.4: `advisories ok, bans ok, licenses ok, sources ok` on the 272-package lock. Banning `once_cell`/`lazy_static` was tried and **failed**, because they are transitive dependencies (tracing-core, rustls, tempfile, insta). They are therefore not banned here.

```toml
[graph]
targets = [
  "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
  "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
  "x86_64-apple-darwin", "aarch64-apple-darwin",
  "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc",
]

[advisories]
db-urls = ["https://github.com/rustsec/advisory-db"]
yanked = "deny"
unmaintained = "workspace"   # direct deps only
ignore = []

[licenses]
confidence-threshold = 0.93
allow = [
  "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause", "ISC",
  "Zlib", "Unicode-3.0", "Unlicense", "CDLA-Permissive-2.0", "MPL-2.0",
]
# libyaml-safer has `license-file = LICENSE-MIT` (no SPDX field): cargo-deny warns
# `no-license-field` but detects MIT from the file and passes.

[bans]
multiple-versions = "warn"
wildcards = "deny"
highlight = "all"
deny = [
  { crate = "openssl-sys", reason = "rustls only" },
  { crate = "openssl", reason = "rustls only" },
  { crate = "serde_yaml", reason = "deprecated" },
  { crate = "serde_yml", reason = "RUSTSEC-2025-0068 unsound/unmaintained" },
  { crate = "libyml", reason = "RUSTSEC-2025-0067" },
  { crate = "yaml-rust", reason = "RUSTSEC-2024-0320 unmaintained" },
  { crate = "atty", reason = "RUSTSEC-2021-0145; use std::io::IsTerminal" },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

Duplicate versions observed in the skeleton:
- `getrandom` 0.2 (via ring) and 0.4 (via tempfile).
- `windows-sys` 0.52 and 0.61.
- `syn` 2 and 3 (proc-macros only).
- `phf` 0.12 via **file-identify 0.4.0**. A user-owned fix: bump to phf 0.14 in file-identify 0.4.1.

### 4.8 `.cargo/config.toml` and `.config/nextest.toml`

```toml
# .cargo/config.toml
[alias]
xtask = "run --package xtask --"
lint  = "clippy --workspace --all-targets --all-features --locked"
# Do NOT put -Dwarnings in rustflags: in CI set CARGO_BUILD_WARNINGS=deny (Cargo 1.97+, cache-friendly).
```

```toml
# .config/nextest.toml  (✔ validated with cargo-nextest 0.9.133; JUnit written to target/nextest/ci/junit.xml)
[profile.default]
fail-fast = false
slow-timeout = { period = "30s", terminate-after = 4 }
status-level = "fail"
final-status-level = "slow"

[profile.ci]
retries = 2
failure-output = "immediate-final"
slow-timeout = { period = "60s", terminate-after = 5 }

[profile.ci.junit]
path = "junit.xml"
```

---

## 5. CI matrix (GitHub Actions)

Latest action tags as of 2026-10-07: `actions/checkout@v7` (v7.0.1), `Swatinem/rust-cache@v2` (v2.9.2), `taiki-e/install-action@v2` (v2.87.26), `EmbarkStudios/cargo-deny-action@v2` (v2.1.1), `codecov/codecov-action@v7` (v7.1.1), `crate-ci/typos@v1` (v1.51.1), `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `actions/attest-build-provenance@v4`, `actions/setup-python@v7`, `astral-sh/setup-uv@v10`, `PyO3/maturin-action@v1` (v1.51.0). Install the toolchain with `dtolnay/rust-toolchain@master` plus `toolchain: 1.99.0`, or rely on `rust-toolchain.toml`.

Global env: `CARGO_TERM_COLOR=always`, `CARGO_BUILD_WARNINGS=deny`, `RUST_BACKTRACE=1`. `CARGO_INCREMENTAL=0` is automatic on 1.99 when `CI` is set.

| Job | Runner(s) | Steps |
|---|---|---|
| fmt | ubuntu-24.04 | `cargo fmt --all --check` |
| clippy | ubuntu-24.04, macos-15, windows-2025 | `cargo clippy --workspace --all-targets --all-features --locked` |
| test | **ubuntu-24.04, ubuntu-24.04-arm, macos-15 (arm64), macos-15-intel (x86_64; Tier 2 since 1.90), windows-2025, windows-11-arm** | `cargo nextest run --workspace --locked --profile ci`; `cargo test --doc --workspace` |
| msrv | ubuntu-24.04 | toolchain 1.98.0: `cargo check --workspace --all-targets --locked` (optionally `cargo hack check --rust-version --workspace`) |
| compat (differential) | ubuntu-24.04, macos-15, windows-2025 | `setup-python@v7`; `pip install pre-commit==4.6.2`; xtask runs fixture repos through both tools and diffs stdout, stderr, exit code and file effects. Also runs the YAML corpus (PyYAML CSafeLoader vs snp-yaml), the regex matrix, shlex and dumps. |
| deny | ubuntu-24.04 | `EmbarkStudios/cargo-deny-action@v2` (`check advisories bans licenses sources`) |
| unused deps | ubuntu-24.04 | `cargo machete` (0.9.2) or `cargo shear` (1.14.0; it needs rustc 1.95+ to build, which is fine) |
| typos | ubuntu-24.04 | `crate-ci/typos@v1` |
| coverage | ubuntu-24.04 | `taiki-e/install-action` (cargo-llvm-cov 0.9.1, nextest); `cargo llvm-cov nextest --workspace --lcov --output-path lcov.info`; `codecov/codecov-action@v7` |
| fuzz (scheduled, nightly) | ubuntu-24.04 | `cargo +nightly fuzz run <target> -- -max_total_time=300` for yaml, pyre, shlex, git-z |
| release (tags) | cargo-dist generated `release.yml` | targets `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`; `cargo auditable`; checksums; `attest-build-provenance`; Homebrew + shell/PowerShell installers; maturin wheels (`bindings="bin"`) for PyPI |

Runner labels must be re-verified when the workflow is written; GitHub rotates them.

Caching: use `Swatinem/rust-cache@v2` with `shared-key` per OS. Since 1.91 `build.build-dir` can move intermediate files outside `target/`. Keep the default unless the cache size becomes a problem.

---

## 6. Final `[workspace.dependencies]` (exact versions)

✔ Verified: `cargo metadata` resolves with cargo 1.98.0, giving 272 packages and 188 unique normal dependencies when every entry is used. `cargo clippy --workspace --all-targets --locked` compiles all of them cleanly. `cargo deny check` passes. Versions are the latest stable on crates.io as of 2026-10-07.

```toml
[workspace.dependencies]
# --- internal crates ---
snp-pycompat = { path = "crates/snp-pycompat" }
snp-yaml     = { path = "crates/snp-yaml" }
snp-pyre     = { path = "crates/snp-pyre" }
snp-config   = { path = "crates/snp-config" }
snp-git      = { path = "crates/snp-git" }
snp-exec     = { path = "crates/snp-exec" }
snp-store    = { path = "crates/snp-store" }
snp-lang     = { path = "crates/snp-lang" }

# --- user's own Rust ports (crates.io) ---
cfgv          = { version = "0.2.0", default-features = false }   # use load_strategy closure + CheckFn::custom (see §7.28)
file-identify = { version = "0.4.0", default-features = false }   # MSRV 1.94; pulls phf 0.12

# --- CLI ---
clap          = { version = "4.6.7", features = ["derive", "env", "string", "wrap_help"] }
clap_complete = "4.6.11"
clap_mangen   = "0.3.3"                                           # xtask only

# --- async runtime / processes ---
tokio        = { version = "1.53.2", default-features = false, features = ["rt-multi-thread", "macros", "process", "signal", "io-util", "io-std", "sync", "time", "fs"] }
tokio-util   = { version = "0.7.19", default-features = false }   # CancellationToken
futures-util = { version = "0.3.34", default-features = false, features = ["std"] }
process-wrap = { version = "10.0.1", default-features = false, features = ["tokio1", "process-group", "job-object", "kill-on-drop", "creation-flags"] }
rustix       = { version = "1.1.5", default-features = false, features = ["std", "process", "pty", "termios", "fs", "thread", "stdio"] }  # unix only
nix          = { version = "0.31.3", default-features = false, features = ["feature"] }   # unix only: sysconf(ARG_MAX)
rayon        = "1.12.0"

# --- config & data formats ---
serde         = { version = "1.0.229", features = ["derive"] }
serde_json    = { version = "1.0.151", features = ["preserve_order"] }
toml          = { version = "1.1.6", default-features = false, features = ["std", "parse", "display", "serde", "preserve_order"] }
libyaml-safer = "0.3.0"
indexmap      = "2.14.2"

# --- matching / text ---
regex       = "1.13.1"        # tier 1 for user patterns (via snp-pyre), regex::bytes for pygrep, tier-2 prefilters, internal fixed patterns (01 Addendum)
fancy-regex = "0.19.2"        # tier 2 only: look-around/backrefs/conditionals/atomic/possessive (via snp-pyre; 01 Addendum)
globset     = { version = "0.4.20", default-features = false }
memchr      = "2.8.3"
bstr        = { version = "1.13.1", default-features = false, features = ["std"] }
smallvec    = { version = "1.16.2", features = ["const_generics"] }

# --- filesystem / paths ---
ignore    = "0.4.33"          # only for non-git modes
tempfile  = "3.27.0"
fs-err    = { version = "3.3.2", features = ["tokio"] }
which     = "8.0.6"
same-file = "1.0.6"
dunce     = "1.0.5"

# --- hashing ---
blake3   = "1.8.7"
sha2     = "0.11.0"
md-5     = "0.11.0"
foldhash = "0.2.0"

# --- errors / diagnostics ---
thiserror          = "2.0.21"
anyhow             = "1.0.104"   # xtask/tests only
tracing            = { version = "0.1.44", default-features = false, features = ["std", "attributes"] }
tracing-subscriber = { version = "0.3.23", default-features = false, features = ["std", "fmt", "ansi", "env-filter", "smallvec"] }

# --- terminal ---
anstyle-query = "1.1.5"
indicatif     = { version = "0.18.6", default-features = false }  # optional feature only
humantime     = "2.4.0"                                           # lefthook-style `timeout: 1m30s` (verified: 15s, 1m30s, 1.5h, 2h45m, 500ms)

# --- network & archives (language bootstrap only) ---
ureq  = { version = "3.4.2", default-features = false, features = ["rustls", "platform-verifier", "gzip"] }
flate2 = "1.1.10"                                                  # default backend miniz_oxide (forbid(unsafe_code))
tar   = { version = "0.4.46", default-features = false }           # >= 0.4.45 required (RUSTSEC-2026-0067/0068)
zip   = { version = "8.6.0", default-features = false, features = ["deflate-flate2"] }

# --- dev / test / bench ---
insta             = { version = "1.49.0", features = ["filters", "redactions", "json"] }
insta-cmd         = "0.7.0"
assert_cmd        = "2.2.2"
assert_fs         = "1.1.4"
predicates        = "3.1.4"
pretty_assertions = "1.4.1"
proptest          = "1.11.0"
rstest            = { version = "0.27.0", default-features = false }
criterion         = { version = "0.8.2", default-features = false, features = ["cargo_bench_support"] }
```

Use `[target.'cfg(unix)'.dependencies] rustix.workspace = true; nix.workspace = true` in the member crates that need them.

Optional, add only when a feature needs it:
- `liblzma = { version = "0.4.8", default-features = false, features = ["static"] }`
- `zstd = "0.14.0"`
- `rusqlite = { version = "0.40.2", features = ["bundled"] }` (only to share pre-commit's store)
- `mimalloc = { version = "0.1.52", default-features = false }` (musl builds, after measuring)
- `schemars = "1.2.2"`
- `jsonc-parser = "0.34.0"`
- `toml_edit = "0.25.15"`

---

## 7. Implementation notes for snp (Rust-oriented recommendations and pitfalls)

1. **Zero unsafe in snp crates.** `unsafe_code = "forbid"` everywhere. Unsafe code exists only in audited dependencies: tokio, rustix, memchr, rayon, process-wrap, blake3, and the single block in libyaml-safer. If Windows color-detection parity ever needs FFI, add one `snp-sys` crate with `deny` + `#[expect(unsafe_code, reason)]` and `// SAFETY:` comments.

2. **`panic = "unwind"` is load-bearing.**
   - The staged-files-only stash/restore must be a RAII guard whose `Drop` restores the user's unstaged changes even during a panic.
   - `main` wraps execution in `std::panic::catch_unwind` (plus a panic hook that captures the location and payload via `PanicHookInfo::payload_as_str`) and maps panics to pre-commit's "An unexpected error has occurred: {Type}: {msg}" plus exit 3, writing the crash log (`pre-commit/pre_commit/error_handler.py:19-81`).
   - With `panic=abort`, none of this would run.

3. **Never call `std::process::exit` below `main`.** Return `std::process::ExitCode` from `main`; exits that bypass destructors skip the guards. `Interrupted (^C): KeyboardInterrupt: ` (note the trailing space) uses exit 130.

4. **Never mutate the process environment.**
   - Build child environments with `Command::env`/`env_remove`/`env_clear`.
   - Hooks see env vars that pre-commit sets, such as `PRE_COMMIT=1`, `PRE_COMMIT_FROM_REF`, …; set them per child.
   - Tests also must not use `set_var`; pass env to `assert_cmd::Command`.

5. **Output writer.**
   - One `Output` type writes **bytes** to a `StdoutLock` and flushes after every logical write, like `output.write`/`write_line_b` (`output.py:9-29`).
   - Windows console: std returns an error for non-UTF-8 bytes on a console handle ("Windows stdio in console mode does not support writing non-UTF-8 byte sequences", `library/std/src/sys/stdio/windows.rs:134`). Lossy-convert hook output only when stdout is a Windows console (`String::from_utf8_lossy`; 1.99 `from_utf8_lossy_owned`).
   - Handle `ErrorKind::BrokenPipe` (for example `snp run | head`) by exiting quietly.

6. **Colors.** Use the exact escape constants from §3.10. The decision is `stderr.is_terminal() && term_supports && env TERM != "dumb"`, overridden by `--color` (default from `PRE_COMMIT_COLOR`). Never color based on stdout.

7. **`_len_cjk` parity.** Use a generated EAW W/F table (§3.10), not `unicode-width`.

8. **Filenames as bytes.**
   - Read `git ... -z` output into one `Vec<u8>` and split with `memchr::memchr_iter(0, &buf)` into `Range<u32>`s (an arena). Do not allocate one `String` per file.
   - Convert to `&OsStr` with `OsStrExt::from_bytes` on Unix.
   - pre-commit decodes git output as UTF-8 (`.decode()`, strict), so non-UTF-8 names fail there. Decide on parity vs leniency (Open questions).

9. **Per-hook filtering.**
   - Compile each distinct `files`/`exclude` pattern once (an `IndexMap` cache).
   - Compute file tags once per file, lazily: `Vec<OnceLock<Tags>>` indexed by file id.
   - When files × hooks > ~10k, filter with `rayon` and collect `Vec<u32>` ids per hook.
   - Preserve pre-commit's ordering exactly; never iterate a `HashMap` into output.

10. **Python `repr` helper.** It is needed for cfgv messages (`'{v!r} is not a valid python regex'`, `could not determine a constructor for the tag '!x'`, …). The rules:
    - Choose `'` unless the string contains `'` and no `"`.
    - Escape `\\`, the chosen quote, `\n`, `\r` and `\t`.
    - Write other non-printables as `\xNN`, `\uNNNN` or `\UNNNNNNNN`.
    - For example, Python's `repr('^\.py(')` is `'^\\.py('`, with the backslash **doubled**.

11. **shlex.** Use the own port (§3.3), with exact `ValueError` texts `No closing quotation` and `No escaped character`.

12. **Store lock.** Follow the §3.6 algorithm. Never block a tokio worker on `File::lock`; use `spawn_blocking`.

13. **Concurrency parity.**
    - pre-commit's `cpu_count()` uses `len(os.sched_getaffinity(0))` and falls back to `multiprocessing.cpu_count()` (`xargs.py:26-38`).
    - `std::thread::available_parallelism()` **also applies cgroup v2 CPU quotas** (`library/std/src/sys/thread/unix.rs:147-172`), so it can return fewer CPUs in containers.
    - For parity, use `rustix::thread::sched_getaffinity(None)?.count()` on Linux and `available_parallelism` elsewhere.
    - `PRE_COMMIT_NO_CONCURRENCY` and `require_serial` force 1.

14. **Command-line length** (`xargs.py:41-69`).
    - POSIX: `max(min(sysconf(SC_ARG_MAX) - 2048 - environ_size, 2**17), 2**12)`, where `environ_size = 8*len(env) + Σ(len(k)+len(v)+2)`. Use `nix::unistd::sysconf(SysconfVar::ARG_MAX)`.
    - Windows: `2**15 - 2048`, with length measured in UTF-16 code units (`s.encode_utf16().count()`) of `' '.join(cmd)`.
    - Other platforms: `2**12`.
    - Also map `ErrorKind::ArgumentListTooLong` (E2BIG) to a clear error.

15. **PTY and pipes.** Use the §3.5 recipe.
    - **Drop the `Command`** (which owns the child-side fds) before reading the master or pipe. Otherwise the reader never sees EOF.
    - Without a PTY (color off, or Windows), merge stdout and stderr with `io::pipe()` and `writer.try_clone()` (✔ verified ordering).
    - Capture into a `Vec<u8>`; pre-commit prints output only after the status line.

16. **Process groups and signals.**
    - Default: children inherit snp's process group and console. Terminal Ctrl-C reaches them directly, as with pre-commit.
    - On `tokio::signal::ctrl_c()`: stop scheduling new partitions, wait for running children (with a grace period), restore the stash (guard), print the interrupt line, and exit 130.
    - Only for explicit timeouts (a lefthook-style `timeout:` extension): spawn with `process_group(0)` and kill the group with `rustix::process::kill_process_group(pid, Signal::TERM)` then `KILL`. On Windows use a process-wrap `JobObject`.
    - Avoid `pre_exec`. It is unsafe, and it disables std's `posix_spawn` fast path.

17. **Windows specifics.**
    - `.bat`/`.cmd` arguments that cannot be escaped make `Command::spawn` fail (CVE-2024-24576 mitigation). Surface the error.
    - pre-commit's shebang emulation (`parse_shebang`) must be ported.
    - Use `dunce::canonicalize` for paths.
    - `std::env::home_dir` follows `USERPROFILE`, close to Python `ntpath.expanduser`.
    - File lock interop: see §3.6.
    - `windows-2025` and `windows-11-arm` are in the CI matrix.

18. **YAML layer** (`snp-yaml`):
    - libyaml-safer events go into a composer, then a YAML 1.1 resolver (§3.1.6), then a SafeConstructor that produces `cfgv::Value` plus spans.
    - Error rendering follows §3.1.4 (marks are `line+1`/`column+1`; the context mark is omitted when it equals the problem mark).
    - Normalize universal newlines first (§3.1.8).
    - Keep scalar style and byte spans for `migrate-config`.
    - The emitter is libyaml-safer's `Emitter` with `indent=4` and `unicode=false`, plus the representer of §3.1.5.
    - Add differential tests against PyYAML in CI.
    - Fail closed on unknown tags (pre-commit errors).

19. **Regex layer** (`snp-pyre`): follow 01 Addendum §A.6, which supersedes §3.2 here. Use two tiers: `regex` for patterns without backtracking-only constructs, and `fancy-regex` (`backtrack_limit` 10⁸, behind the relaxed `regex` prefilter) for the rest. Write tests against a Python-generated truth table checked into the repo, so Python is not needed at test time.

20. **Paths.**
    - `realpath` (`store.py:36`, `os.path.realpath`) is non-strict in Python. `fs::canonicalize` fails for missing paths, so canonicalize the longest existing ancestor and append the rest lexically.
    - Python's `expanduser('~')` with `HOME=""` yields `''`, so `~/.cache` → `/.cache`. Rust's `home_dir()` falls back to the passwd entry since 1.90. Replicate Python's behavior only if parity tests require it.

21. **Determinism.** Use `IndexMap` for every config mapping; sort only where pre-commit sorts. The clippy lint `iter_over_hash_type` is on.

22. **Memory.**
    - `Box<str>` for immutable config strings; `Arc<str>`/`Arc<[..]>` for data shared across tasks.
    - A file-list arena (see note 8).
    - `SmallVec` for tags.
    - Reuse output buffers between hooks (`Vec::clear`).
    - `fmt::from_fn` for lazy `Display`.
    - Avoid `format!` in loops; use `write!` into a reused `String`.
    - Do not keep full `git diff` outputs when a 32-byte blake3 digest suffices to detect "files were modified by this hook". Verify that the reported diff text is not needed; `--show-diff-on-failure` reruns `git diff` anyway.

23. **Startup.**
    - Do not compile regexes or initialize tracing unless needed (use `LazyLock`).
    - `hook-impl` fast path: when no config is found, print pre-commit's message and exit without loading anything else.
    - Use a single tokio runtime built in `main` (`Builder::new_multi_thread().enable_all()`); keep `#[tokio::main]` out of library crates.

24. **Tests.**
    - No global env mutation.
    - Isolate git with child env: `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=<tmp>/gitconfig`, `HOME=<tmp>`, `XDG_CACHE_HOME=<tmp>/cache`, `PRE_COMMIT_HOME=<tmp>/pc`, and fixed `GIT_AUTHOR_*`/`GIT_COMMITTER_*`.
    - Use `env!("CARGO_BIN_EXE_snp")`.
    - insta filters for temp dirs, `\\` vs `/`, and durations.
    - Debug escaping changed in 1.98, so pin snapshot expectations to the toolchain.

25. **MSRV discipline.** `rust-version = "1.98"`, clippy `msrv = "1.98"`, and a CI job on 1.98.0. 1.99-only APIs (`String::from_utf8_lossy_owned`, `std::fmt::NumBuffer`, `fs::set_times`) are therefore unavailable until the MSRV is bumped. Use `core::fmt::NumBuffer` on 1.98.

26. **Edition 2024 traps.**
    - RPIT over-capture: add `use<..>` when returning iterators from `&self` methods that must outlive the borrow.
    - `if let` rescoping changes when guards drop.
    - `gen` is a keyword.
    - The never-type fallback changed.

27. **std 1.98 traps** (found while verifying):
    - `str::substr_range` returns `core::range::Range`.
    - `std::assert_matches!` is at the crate root.
    - `NumBuffer` lives only in `core::fmt` on 1.98.
    - Inconsistent `Ord`/`PartialOrd` impls can now panic in sort or misbehave with derive fast paths.

28. **cfgv-rs 0.2.0 gaps** (user-owned; fix upstream or work around):
    - (a) `CheckFn::Regex` validates with the `regex` crate (`cfgv-rs/src/check.rs:116-118`). It **rejects valid Python patterns** such as `^(?!tests/)`, which pre-commit accepts. Wrap the check in `CheckFn::custom` using snp-pyre.
    - (b) Its message uses `'{s}'` instead of Python `repr` (backslashes are not doubled).
    - (c) `Value` lacks date/datetime/bigint/bytes-from-`!!binary` distinctions needed for `Expected string got date` parity. Suggest adding `Value::Opaque { type_name, repr }` or `Date`/`DateTime` variants.
    - (d) `load_from_filename` (`cfgv-rs/src/load.rs:98-108`) uses `read_to_string`. It neither normalizes newlines nor produces Python's `'utf-8' codec can't decode byte …` text. snp can pre-read and decode itself, then call the schema check directly.

29. **file-identify 0.4.0**: bump `phf` to 0.14 to avoid a duplicate version. Keep the MSRV ≤ 1.98.

30. **Supply chain.**
    - cargo-deny in CI.
    - `rustls ≥ 0.23.45`, `tar ≥ 0.4.45`.
    - `cargo auditable` for release builds.
    - `Cargo.lock` committed; `--locked` everywhere in CI.

31. **Archive safety.**
    - tar: `unpack_in`, reject absolute paths and symlinks escaping the destination, `set_preserve_permissions(true)` only for executables, and never follow symlinks during extraction.
    - zip: `enclosed_name()`.
    - Verify sha256 before extraction when the upstream publishes it (Go does).

32. **HTTP.** ureq inside `spawn_blocking`.
    - Set explicit timeouts (connect 30 s, global per-download budget).
    - Retry idempotent GETs with backoff.
    - Use the platform certificate verifier.
    - Proxies come from env automatically. Show the URL in errors.

33. **Git.**
    - Shell out to `git`, exactly like pre-commit, for behavioral parity (hooks, worktrees, sparse checkout, submodules, `core.hooksPath`).
    - gix 0.88.0 and git2 0.21.0 could be used later only for read-only fast paths, after differential tests prove equivalence.

---

## 8. Open questions

1. **Toolchain pin.** This spec recommends `channel = "1.99.0"` with MSRV 1.98, against the orchestrator's stated 1.98.0. The alternative is to pin `1.98.1`, which carries the vtable miscompilation fix. Only 1.98.0 is installed locally.
2. **Store sharing.** Should snp reuse `~/.cache/pre-commit` (SQLite `db.db`, which needs `rusqlite` bundled and identical environment layouts) or use its own `SNP_HOME`? This spec assumes its own store, with an optional `snp migrate-store`.
3. **libyaml-safer longevity.** Accept the dependency (with a fork-ready adapter), or vendor it now with the unsafe block removed? Should the user upstream the safe `from_utf8` change?
4. **CLI error text parity.** Should argparse-style usage and `error:` messages be reproduced byte-for-byte (custom clap error renderer and usage generator), or is matching the exit code 2 enough?
5. **NO_COLOR.** pre-commit ignores it. Should snp honor `NO_COLOR` only when neither `--color` nor `PRE_COMMIT_COLOR` is set (a divergence), or ignore it for parity?
6. **Windows color detection parity.** pre-commit enables VT on stderr only. anstyle-query enables stdout and stderr and fails when stdout is redirected. Accept the divergence or add an audited `snp-sys` FFI crate?
7. **Python `$` semantics.** Should `$` match before a final `\n` (exact, but forces backtracking on most patterns) or stay strict end-of-text (fast; diverges only for filenames ending in a newline)?
8. **Python dict key coalescing.** In YAML mappings `1`, `1.0` and `true` are the same key in Python. Replicate it in `cfgv::Value` `Eq`/`Hash`?
9. **Non-UTF-8 filenames.** pre-commit crashes or fails on them (strict decode). Should snp match that, or support them (bytes regex via fancy-regex `BytesMode`, `OsStr` paths)?
10. **PyPI distribution name.** `snp` is taken on PyPI by a third-party placeholder (0.0.0). Choose an alternative (`snp-hooks`, `you-shell-not-pass` and `snp-cli` all return 404, so they are available) if wheels are desired. crates.io `snp` belongs to the user.
11. **tokio runtime flavor.** This spec recommends multi-thread. Is `current_thread` + rayon + `spawn_blocking` preferable for a smaller footprint? Benchmark with hyperfine on a real repo.
12. **Scope of language bootstrapping.** Do Go, Node, Ruby and Python toolchains get downloaded by snp itself (which needs ureq, tar, zip and maybe xz/zstd), or are they delegated to installed tools (`uv`, `nodeenv`, `virtualenv`)? This decides whether the network and archive crates ship at all.
13. **EAW Unicode version** for `_len_cjk` parity. Which CPython version is the reference (3.12 = Unicode 15.0, 3.13 = 15.1, 3.14 = 16.0)?
14. **cfgv-rs evolution.** Will the user accept upstream changes (pluggable regex check, Python repr, `Opaque`/`Date` values, Python-style UTF-8 errors), or should snp wrap and extend locally?
15. **Optional mimalloc for musl.** It must be measured on CI musl builds before enabling. Is a static musl build even a release target, or are gnu builds with an old-glibc baseline (via cargo-zigbuild) preferred?
