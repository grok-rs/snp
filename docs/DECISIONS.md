# snp — Decision log (ADRs)

Every contradiction of `docs/research/00-critic-findings.md` (C1–C18), every cross-cutting correction of the three
design judges, and every open question of the research documents is resolved here. Format: **Context** (why a
decision was needed, with sources), **Decision**, **Rationale**, **Consequences**. All ADRs are *Accepted* as of
2026-10-08. A later change to an accepted ADR is a new ADR that supersedes it (never an in-place edit of the decision),
approved by the integration lead and the owners of the affected packages; the amended ADR only gains a status line
naming the amending ADR, which is normative where they differ. ADR-056…ADR-080 apply the second design review
(2026-10-08); Appendix B lists every finding and its disposition. ADR-081 applies the verification of the W0 skeleton;
ADR-082 the end-to-end integration of the merged packages; ADR-083 the verification of the integrated `main`; ADR-084
the final multi-lens review; ADR-085 the `interactive` extension key; ADR-086 the first CI run on macOS; ADR-087 the
`.snp.yaml` config file name; ADR-088 the crates.io and Homebrew release channels; ADR-089 the retarget to pre-commit
4.7.0; ADR-090 what the scheduled gates can judge; ADR-091 measuring every scenario; ADR-092 the November 2026
upstream refresh; ADR-093 the path-limited stash checkout; ADR-094 Rust 1.100; ADR-095 machine-checked proofs;
ADR-096 the owner's decision to make the Verus proofs blocking;
ADR-099 the paired bench gate; ADR-100 the B8 heap budget and counters;
ADR-098 the tier-1 prefilter;
ADR-097 the stash re-verification by stat stamps.

Index of the 18 research contradictions: C1→ADR-019, C2→ADR-002, C3→ADR-003, C4→ADR-004, C5→ADR-005, C6→ADR-006,
C7→ADR-007, C8→ADR-008, C9→ADR-009, C10→ADR-012, C11→ADR-011, C12→ADR-013, C13→ADR-014, C14→ADR-017,
C15→ADR-018, C16→ADR-016, C17→ADR-021, C18→ADR-022. Open questions per research document: Appendix A.

---

### ADR-001 — Synthesis base and the corrections applied to the three proposals

**Context.** Three architecture proposals (compatibility-first, performance-first, robustness-first) were scored by
three judges. Two judges picked *robust* as the base (contracts-first skeleton, journaled stash, seams,
fault-injection matrices, RepoKey docker tags, legacy-safe kill switch); one picked *compat* (most exact byte-parity
mechanisms). All three listed fatal flaws in every proposal and cross-cutting corrections none of them made.
**Decision.** Robust's skeleton, with compat's parity layer and perf's performance layer grafted whole, and these
corrections: docker tags hashed from the repo key (ADR-035); per-deps prefixes via `git clone --local` (ADR-025, perf's
shared clone rejected); `SNP=0` never skips `.legacy` (ADR-038); restore-critical git in an isolated process group on
Unix *and* Windows (ADR-026); DiffOracle restricted to Unix with every `git diff` input covered (ADR-027); delegation
decided before installs (ADR-032); regex constants from the 01 Addendum (ADR-003); per-subsystem Python profiles
(ADR-020); global output budget (ADR-036); fd hygiene (ADR-037); poison recovery (ADR-040); parity restored for piped
ordering, Windows CRLF, Windows VT colour, `clean` output, post-rewrite legacy stdin, `\z` rejection, `x-*` warnings,
compat/snp `--version` (ADR-023/028/029/030/039/046); `--stdin-files` instead of `--files-from-stdin` (prefix
stability); store ownership warns instead of refusing (ADR-044); the real cfgv 0.3 / file-identify 0.5 APIs bound
(ARCHITECTURE §1.5); 21 languages and the native hooks split across several packages (WBS).
**Rationale.** Robust's structure is the hardest part to retrofit; compat's and perf's strengths are local mechanisms.
**Consequences.** ARCHITECTURE.md is normative; the proposals are superseded and not kept in the repository.

### ADR-002 — YAML: libyaml-safer events + own PyYAML layers; error texts target Tier A (C2)

**Context.** 01 §18.2 and 07 D3 say libyaml-safer reproduces PyYAML CSafeLoader messages byte for byte (14/14 scanner
and parser texts verified, 112/112 dump lines); 03 §13 says YAML errors cannot be byte-identical; 10 §6.7 made them
Tier B. **Decision.** `snp-yaml` = libyaml-safer 0.3 behind a small event adapter + snp-owned composer, YAML 1.1
resolver, SafeConstructor and CSafeDumper-compatible emitter. Error texts are a **Tier A target**: the verified corpus
is gated exactly; any residual mismatch the generated corpus finds is a registered gap (Tier B for that message class
only, never a blanket Tier B). **Rationale.** Evidence shows parity is achievable, and YAML errors are among the most
visible outputs. **Consequences.** libyaml-safer (single maintainer) is fuzzed and fork-ready (`[patch.crates-io]`);
serde_yaml-family crates are banned; the composer is an arena (aliases share node ids) used by migrate-config.

### ADR-003 — Regular expressions: the 01 Addendum, verbatim (C3)

*Status: Accepted; amended by ADR-079 (tier-1/2 lazy-DFA cache = the crate default 2 MiB; `size_limit` 64 MiB unchanged) and ADR-098 (the tier-1 exact fallback runs behind the relaxed prefilter). The amending ADRs are normative where they differ.*

**Context.** 01 §13/§18.1 and 04a §12.3 proposed two tiers, 07 D4 picked fancy-regex alone, 10 Q6 deferred; all three
proposals then contradicted the normative 01 Addendum (corpus of 4,858 real patterns) on constants and grammar.
**Decision.** Own CPython-**3.12** `re` parser and lowering (`snp-pyre`) is the single accept/reject authority;
**tier 1** `regex` 1.13 (`regex::bytes` with `unicode(false)` for pygrep) for patterns without backtracking constructs;
**tier 2** `fancy-regex` 0.19.2 for look-around/backrefs/conditionals/atomic/possessive, **always** behind the relaxed
tier-1 prefilter (§A.6.11), `backtrack_limit = 100_000_000`, bytes on the Latin-1 lane (never `BytesMode::Ascii`).
Nesting ≤ 484 accepted, ≥ 485 → validation error; repeat ≥ 4294967295 → validation error; `\z`, `(?<name>…)`,
non-initial global flags and non-ASCII conditional names rejected; `(?t)` with 3.12's compile-time rule. Unicode data
from `regex-syntax` (UCD 16): `\w`→`[\p{L}\p{N}_]`, `\s`→`[\s\x{1C}-\x{1F}]`, IGNORECASE → explicit simple-fold classes
plus the Turkish-i rule; non-MULTILINE `$` canonical `(?=\n?\z)` with haystack-dispatched fast forms; Unicode `\b`/`\B`
exact look-around forms with ASCII-only fast forms; `^$` = `h == "" || h == "\n"`. Validation parses only; engines
compile on first match. cfgv's bool regex hook receives `snp_pyre::cfgv_is_valid`.
**Rationale.** The corpus shows 100 % match parity for the translated two-tier pipeline, a 29 % speed advantage over
fancy-only, that pygrep needs `regex::bytes`, and that a real pygrep pattern needs > 10⁷ backtracks.
**Consequences.** Residual divergences §A.6.13 D1–D11 are registered (D-MISC-04/05/06, D-SEC-10, D-CRASH-04).

### ADR-004 — No async runtime (C4)

*Status: Accepted; amended by ADR-065 (InstallLimiter weighting, pool sizing, reader threads). The amending ADR is normative where they differ.*

**Context.** 02 §21.4 and 04b §4.1: no async needed; 06 §8: tokio current-thread; 07 D6/D8: tokio multi-thread.
**Decision.** `std::thread::scope` + one global `JobPool` (hook processes and native partitions) + `Limiter`s for
network (8) and installs (`min(N, 4)`) + blocking `ureq` + one signal thread + one watchdog thread. tokio, async-std
and rayon are banned in `deny.toml`. **Rationale.** The workload is process-bound; synchronous code makes guard/drop
ordering (stash safety) obvious; no runtime start-up (B1 ≤ 3 ms); smaller binary.
**Consequences.** Native per-file parallelism inside pre-commit-hooks uses scoped threads on the same pool.

### ADR-005 — Toolchain pin (C5)

*Status: Accepted; amended by ADR-094 (Rust 1.100.0, MSRV 1.100). The amending ADR is normative where they differ.*

**Decision.** `rust-toolchain.toml` channel `1.99.0` (components rustfmt, clippy, llvm-tools-preview), `rust-version =
"1.98"`, edition 2024, resolver 3; CI runs an MSRV `cargo check` job on 1.98.0. 10 §0/§7's "1.98.0 channel" is
superseded by the owner's FD-4. **Consequences.** Language features newer than 1.98 are forbidden (MSRV job).

### ADR-006 — hazmat: two paths, two exit-code regimes, `__hazmat` (C6)

**Context.** 03 §15 says hazmat failures exit 3; on 4.6.2, `entry: pre-commit hazmat …` is rewritten to
`python -m pre_commit.commands.hazmat` (no error handler: traceback, exit 1; usage errors exit 2 with prog `hazmat.py`).
**Decision.** In-hook path H: `hook_cmd` rewrites `pre-commit hazmat …`/`snp hazmat …` (first two shlex tokens, any
quoting) to `[self_exe, "__hazmat", …]`; `__hazmat` is dispatched on raw `argv[1]` before parsing, under both brands,
with prog `hazmat.py`, CPython-3.13 argparse texts, exec/resolution failures printing only Python's final traceback line
(exit 1), usage exit 2, SIGINT: 250 ms then SIGKILL the child and die by SIGINT. Direct CLI path C (`{prog} hazmat …`):
envelope, `ExecutableNotFoundError` exit 3, usage exit 2; `hazmat` with no tool is pre-commit's crash
(`AttributeError`, exit 3) — snp prints the usage error, exit 2 (D-CRASH-22). Other `pre-commit …` entries are not
rewritten. **Consequences.** `argv[1] == "__hazmat"` is reserved (D-MISC-02); hazmat lives in `snp-hooks`.

### ADR-007 — `PRE_COMMIT_COLOR` conversion timing (C7)

**Decision.** The env value is `--color`'s raw string default, converted (and possibly rejected with
`argument --color: invalid use_color value: 'x'`) only at the end of the owning subparser's parse and only when
`--color` was absent; never for `help`, `-h`, `-V` or `__hazmat`. **Rationale.** Verified on 4.6.2 (12 §A.9):
`PRE_COMMIT_COLOR=bogus pre-commit sample-config --color never` exits 0.

### ADR-008 — argparse error routing (C8)

**Decision.** Unrecognized arguments from any level are reported by the top-level parser (top-level usage +
`{prog}: error: unrecognized arguments: …`, exit 2); every other error by the owning parser with its usage; first error
in consumption order wins; ambiguity is raised at consumption. **Rationale.** 12 §A.6/§A.8 and 10 CLI-07 (verified);
02 §2.2 was incomplete.

### ADR-009 — Environment directory naming (C9)

**Context.** 03 §20.5 #9 proposed sanitised names; 04a §14.2 kept pre-commit's literal names (nested directories for
absolute python paths) and rejected only escaping paths; `language_version: ../../x` makes pre-commit delete arbitrary
directories (04b §5.1, verified). **Decision.** `<ENV_DIR>-<version>` iff the version matches
`^[A-Za-z0-9._+-]{1,64}$` and is not `.`/`..`, else `<ENV_DIR>-h<hex16(blake3(version))>`; deletion only through
`remove_tree_within(env_dir, <repo>/envs)`. **Rationale.** The store is private (FD-2), so literal names buy nothing;
the rule closes the traversal and Windows `C:` breakage. **Consequences.** D-SEC-01.

### ADR-010 — CLI engine: own argparse port, static tables, clap only at build time

**Context.** 07 D9 chose clap; 12 §A.12.3 proved clap cannot reproduce argparse acceptance (deferred unknown args,
env-default timing, REMAINDER `--`, negative numbers, short clusters), several of which change exit codes.
**Decision.** `snp-argparse` (port of CPython ≥ 3.13.14 argparse + HelpFormatter + textwrap, no dependencies) driven
by static `ParserSpec` tables in `snp-cli`; `xtask completions` builds a `clap::Command` from the same tables for
completions and man pages; a test asserts identical option sets; clap is banned outside `xtask`.
**Consequences.** Help/usage/error texts are byte-tested at the 12 §A.16 width matrix against `pc462-py313`.

### ADR-011 — Store location, `PRE_COMMIT_HOME`, Windows default (C11)

**Context.** 03 §20.5: own store; 10 Q5: honour `PRE_COMMIT_HOME`; 11 §A.1 #7: `$PRE_COMMIT_HOME/snp`; 04a §14.10:
possibly shared store with lock interop; robust proposed `%LOCALAPPDATA%\snp` on Windows. **Decision.** `SNP_HOME` →
`$XDG_CACHE_HOME/snp` (any OS) → `expanduser("~/.cache")/snp` (Windows `%USERPROFILE%\.cache\snp`, pre-commit's own
default shape, 14 P-65/§A408). `PRE_COMMIT_HOME` is used only to compute pre-commit's store in order to refuse it (path
equality or README signature → FatalError M7). No lock interop, no SQLite, never `~/.pre-commit`.
**Rationale.** FD-2; `%LOCALAPPDATA%` would be prek's convention, not the platform equivalent of pre-commit's.
**Consequences.** CI cache recipes must cache `~/.cache/snp` and change their cache key (docs/user migration guide).

### ADR-012 — Install-state file (C10)

**Decision.** Only `<env>/.snp-install-state.json` (versioned JSON, written last by temp+fsync+rename) marks an
environment installed; pre-commit's `.install_state_v1/v2` and the proposed `.snp_state.json` are never written or
read. Malformed state ⇒ not installed ⇒ reinstall (fixes 13 ENV-01). **Rationale.** Nothing reads pre-commit-format
state in snp's private store.

### ADR-013 — Crash log name and body (C12)

**Context.** 03 §20.5 #13: `<store>/snp.log`; 02 Q7: open; 10 Q9: keep pre-commit's skeleton; 13 R5 wanted the
`pre-commit.log` name for CI log scrapers. **Decision.** The crash log lives in snp's store and its name follows the
brand: `pre-commit.log` (compat) / `snp.log` (snp), so line 2 (`Check the log at …/pre-commit.log`) keeps its shape.
Body: pre-commit's skeleton (`### version information` block with `pre-commit version: 4.6.2` / `snp version: 2.0.0`,
`git --version`, `sys.version:` → `snp X (rustc 1.99.0) [target]`, `sys.executable`, `os.name`, `sys.platform`;
`### error information` with line 1; then the error source chain or, for panics, the backtrace and a hint naming
`https://github.com/grok-rs/snp/issues`). No environment dump. Opened `O_NOFOLLOW`, truncated. Unwritable → `Failed to
write to log at …` + body on stdout (never a double fault). **Consequences.** Body below the version block is Tier B.

### ADR-014 — Executable lookup without `which` (C13)

**Decision.** Own ports of `parse_shebang.find_executable`/`normexe`/`normalize_cmd` and the `command -v` emulation
(15 §A4.11); `which` is banned. Windows: dir-major `PATHEXT`, PATHEXT read per call, relative PATH entries kept
relative. **Rationale.** Six concrete `which` divergences (14 A.4.2, 15 D7).

### ADR-015 — Panic strategy, top-level guard, single-exit restore

*Status: Accepted; amended by ADR-066 (restore in `impl Drop for RunState`, not via field order; panic hook only records and cancels). The amending ADR is normative where they differ.*

**Decision.** `panic = "unwind"` in every profile; `snp::entry` installs a panic hook that records payload, location
and backtrace, runs `dispatch` under `catch_unwind`, and maps a panic to `PanicException` (exit 3, crash log); 101 is
never an exit code. `run()`/`hook_impl()` have a single exit (no `?`, no early `return`; `xtask lint-guards`), call
`StashGuard::restore()` explicitly, and the guard's `Drop` (restore inside `catch_unwind`) is only the panic fallback;
the guard is the last field of `RunState`. **Rationale.** FD-5: the stash must be restored on every exit path.

### ADR-016 — Stash algorithm and hardening (C16)

*Status: Accepted; amended by ADR-067 (create_new patch + re-verify before checkout, git-dir pointer, lock-based recovery, `git apply --3way` hints, StagingFixed state) and ADR-093 (both checkouts limited to the patch's files and, on the rollback, the files `git diff` shows changed: D-DATA-19). The amending ADR is normative where they differ.*

**Context.** 02 §21.10 keeps pre-commit's restore; 06 §7–8 proposed read-tree/verify restore (different git state on
conflict; false mismatches under `core.autocrlf`, 14 A.4.5). **Decision.** pre-commit's exact algorithm and messages
(ITA → write-tree → diff-index → patch → checkout; apply → autocrlf=false apply → rollback checkout → apply) plus:
streamed `O_EXCL` 0600 patch made durable (fsync file and dir) before any worktree mutation; `GIT_DIFF_OPTS` removed
for the diff (hazard a); `--no-renames` on the ITA query (hazard b); a lazily created durable journal written before
every mutation; a per-worktree lock with a nested-run token; restore under a signal shield with git children in an
isolated process group; K+ hint on stderr; next-run recovery notice; read-tree/verify only as warn-only
`SNP_STASH_VERIFY=1` / `snp doctor --stash`. **Consequences.** D-DATA-01/02/04/11/12/13, D-EXT-04; FI-01…FI-24.

### ADR-017 — CPU count (C14)

*Status: Accepted; amended by ADR-065 (the pool is `min(target_concurrency, available_parallelism)` unless `-j`; partitioning still uses `target_concurrency`). The amending ADR is normative where they differ.*

**Decision.** One `py_cpu_count()` (Linux `sched_getaffinity`, else `available_parallelism`) feeds both
`target_concurrency` and `autoupdate -j 0` (pre-commit uses `xargs.cpu_count()` for both, autoupdate.py:187).
Windows differs only above 64 logical CPUs (D-PLAT-10; `GetActiveProcessorCount` would need `unsafe`).

### ADR-018 — fsmonitor neutralisation (C15)

**Decision.** User-repo calls keep pre-commit's argv exactly, with `-c core.useBuiltinFSMonitor=false` only at its
sites (init, remote add, `tag --points-at`, autoupdate `_git`). Every snp-owned repo call additionally gets
`core.fsmonitor=` (empty) and `core.useBuiltinFSMonitor=false` through `GIT_CONFIG_COUNT` (argv unchanged; git ≥ 2.31);
a pre-existing `GIT_CONFIG_COUNT` is extended, a malformed one left untouched. **Rationale.** argv appears in error
texts; an empty value means "off" on every git version (02 G.7 row 4).

### ADR-019 — Hook stdin (C1)

**Decision.** Non-pty hooks (all OSes, incl. Windows colour mode): `Stdio::piped()` with `ChildStdin` dropped
immediately (empty pipe, EOF); pty hooks: `/dev/null`; snp's own git/tool calls: `Stdio::null()`; `use_stdin`
extension: a copy of the hook-impl stdin. 02 §21.3 and 06 §8 (`Stdio::null()` for hooks) were wrong.
**Rationale.** util.py:81-113,176-178; Windows `NUL` makes CRT `isatty(0)` true and changes Python hook behaviour.

### ADR-020 — Emulated Python profile per subsystem

**Context.** compat used one CPython-3.13 profile; perf/robust used 3.13 for regex; the 01 Addendum decided 3.12 for
regex grammar; 12 §A.2 decided 3.13.14+ for argparse texts; 03 H.11 suggested 3.12 hazmat texts.
**Decision.** regex grammar = CPython 3.12, regex Unicode = UCD 16 (regex-syntax); argparse texts (pre-commit CLI,
`pygrep.py`, `hazmat.py`, pre-commit-hooks console scripts) = CPython ≥ 3.13.14; `repr`/`isprintable`/EAW = UCD 15.1;
string/path/shlex/codec semantics = 3.12 vectors + 3.13 delta; pre-commit-hooks emulation (json messages, recursion
thresholds, tomllib/expat/ruamel messages) = CPython 3.13; identify = 2.6.20. Constants in `snp_core::profile`.
**Consequences.** The differential harness tags each byte stream with its oracle (`pc462-py312` runtime/regex,
`pc462-py313` CLI texts) and normalises the 12 §A.4.1 argparse deltas.

### ADR-021 — Native emulation granularity (C17) and pygrep compile-once

*Status: Accepted; amended by ADR-072 (natives parse `args + files` per partition). The amending ADR is normative where they differ.*

**Context.** 04a §12.2: native pygrep in one pass; 09 §0/§2.2: once per partition for parity; 02 §21.14: pygrep
partition semantics must match. **Decision.** Native hooks (pygrep, pre-commit-hooks ports) run once per xargs
partition computed exactly as the subprocess would (synthetic interpreter prefix for the length budget; pygrep
unshuffled with serial partitions; pre-commit-hooks shuffled and parallel; `fail` one call with all files). The regex
of a pygrep entry is compiled **once per hook per run**; an invalid entry fails the hook once with the exact Python
last line (Tier B vs pre-commit's per-partition tracebacks, D-MISC-05). `fast_path_coalesce: true` opts into one call.
**Rationale.** Partition-visible output (footers ×N, abort rest of partition) is observable parity (09 §2.2).

### ADR-022 — Pre-push stdin parsing (C18)

**Decision.** Valid UTF-8: Python `str.splitlines()` + `line.rsplit(maxsplit=3)` (Unicode whitespace incl.
U+001C–U+001F), lazy unpacking; a malformed line fails closed with exit 3 `ValueError: not enough values to unpack
(expected 4, got N)` (byte-identical). Fixes only: any all-zero OID of 40 or 64 hex digits is null (D-CRASH-10);
non-UTF-8 input uses the same algorithm on bytes instead of crashing (D-CRASH-11). 03 §20.4's "skip malformed lines" is
rejected (fail-open for a push guard).

### ADR-023 — Branding, `--version`, compat strings, snp-only surface

*Status: Accepted; amended by ADR-058 (argparse texts rendered with prog `snp`; `brand_map` for non-argparse texts only, plus the `--version` row) and ADR-060 (hook-time brand follows the installing brand); ADR-089 retargets the compat brand to pre-commit 4.7.0. The amending ADR is normative where they differ.*

**Context.** FD-1 (byte-identical in the compat brand, product name included; same layout with product name `snp` in
the snp brand). 11 §A.8 proposed exceptions (E1 `--version` suffix, E2 snp bug URLs, E4 a different min-version hint);
compat/perf proposed `snp X (pre-commit 4.6.2 compatible)` for the snp brand. **Decision.** Brand = `argv[0]` stem
`pre-commit` (exact on Unix; ASCII-case-insensitive with `.exe` stripped on Windows), never `current_exe()`, no env
override. Compat brand: every byte as pre-commit 4.6.2, including `--version` = `pre-commit 4.6.2`, the
`pip install --upgrade pre-commit` min-version hint and pre-commit's bug URL in the migration-mode text; exceptions are
only what the separate store forces (store paths, crash-log body) and the registered divergences; compat exposes exactly
pre-commit's 16 commands and options (snp-only commands/flags are rejected with pre-commit's own `invalid choice`
texts). snp brand: same layout; `--version` = `snp 2.0.0`; the `brand_map` substitutions of ARCHITECTURE §9.6 (tool
name, commands to type, `Try upgrading snp?`, the snp min-version sentence, snp's bug URL); snp-only commands and
flags available, guarded by the prefix-stability test (`--files-from-stdin` rejected → `--stdin-files`).
**Rationale.** The owner's decision is explicit; "which tool is this?" is answered by `snp doctor`, `snp self check`,
`snp self alias status` and the crash log instead of altered compat bytes.
**Consequences.** The difftest runs every scenario in both brands (brand_map applied to oracle output).

### ADR-024 — Store layout, keys, locks, clone prefetch

*Status: Accepted; amended by ADR-064 (prefetch: eligible URLs only, every failure is a silent cache miss redone in the foreground). The amending ADR is normative where they differ.*

**Decision.** ARCHITECTURE §5.2–§5.5: versioned buckets (`repos-v1`, `keys-v1`, `configs-v1`, `tools-v1`, `cache-v1`,
`patches`, `spill`, `scratch`, `trash`), blake3 keys with domain separation, `abs:`-canonical keys for local-path repos
(D-DATA-08), commit markers written last, scratch-then-rename materialisation, global shared/exclusive lock (gc can never
delete a repo in use), per-repo and per-tool locks, lock order stash → global → repo → tool, one `Locking … directory`
line per process on contention. Background clone prefetch with `GIT_TERMINAL_PROMPT=0` on the `NetLimiter`, foreground
retry with prompts after an auth failure (not under `CI`); messages printed at pre-commit's moment.
**Rationale.** No SQLite (FD-2), crash consistency (S10), parallelism without transcript changes.

### ADR-025 — Deps variants get their own prefix, cloned locally

**Context.** perf shared one clone per `(repo, rev)` across `additional_dependencies` variants; installers mutate the
prefix (`cargo add` edits `Cargo.toml`, dart `pubspec.lock`, dotnet/gem/pip build residue; 04a, 04b §5.5) so variants
would contaminate each other; compat used `git clone --shared` (alternates break when the base is removed).
**Decision.** One key and prefix per `(repo, rev, deps)` (pre-commit's isolation); a variant is created with
`git clone --local --no-checkout <base src>` + `checkout <rev>` (+ shallow submodules), falling back to the network
clone. `[INFO] Initializing environment for repo:dep1,dep2.` follows the keys-marker rule (ADR-032).

### ADR-026 — Signals, isolated restore, exit statuses, worktree lock

*Status: Accepted; amended by ADR-065 (roles, origin-aware forwarding table, descendants, Critical commands never signalled) and ADR-067 (journal/lock details). The amending ADR is normative where they differ.*

**Context.** compat and robust left restore-critical git children in the terminal's process group (a second ^C reaches
`git apply`); perf's ctrlc-based design cannot distinguish SIGINT/SIGTERM/SIGHUP; robust invented `Terminated:`
banners (unregistered); perf's journal lived in the lock file (not atomic); compat's nested-run exemption used a pid.
**Decision.** Unix: `signal-hook` thread for INT/TERM/HUP; Windows: `ctrlc` (→ Interrupt). First signal cancels the
token; TERM/HUP are forwarded to registered children; running children get 2 s then SIGKILL (pid-reuse safe). Every
restore-critical git command (stash checkout, both applies, rollback checkout, ITA rm/re-add, stage_fixed add) runs
with `Group::Isolated` (Unix `process_group(0)`; Windows `CREATE_NEW_PROCESS_GROUP` via `creation_flags`) under a
`RestoreShield`. SIGINT → banner + exit 130; SIGTERM/SIGHUP → restore (its messages print), no banner, then re-raise
the signal with the default disposition (`emulate_default_handler`) so the parent sees 143/129 (D-DATA-02). Worktree
lock `<git_dir>/snp-stash.lock` with a random token exported as `_SNP_STASH_LOCK` (nested runs skip the lock); journal
in `<store>/patches/` written temp+rename. **Consequences.** Signal and second-^C matrices in CI (posix + Windows).

### ADR-027 — Modification detection: Unix DiffOracle, Windows real diff, baseline elision

*Status: Accepted; amended by ADR-078 (filesystem allow-list, extra attribute inputs, warm-path-only baseline elision). The amending ADR is normative where they differ.*

**Context.** robust kept one `git diff` per executed hook (B3 ≤ 25 ms doubtful, far off on Windows where a git spawn
costs 10–30 ms); compat's stat fast path ignored non-tracked diff inputs; perf's DiffOracle was unsound on Windows and
omitted system/include config files. **Decision.** ARCHITECTURE §3.7: exact digest semantics; native read-only hooks
never diff; native fixers report modified paths (09 §6.7 caveats fall back); on Unix, local filesystems, ≤ 16 384
tracked files, a stat snapshot over tracked files **and every git-diff input** (index, all config files incl. system,
global, XDG, worktree config, include/includeIf targets, `core.attributesFile`, `info/attributes`, every
`.gitattributes` incl. existence) with blake3 hashing of racy entries replaces the per-hook `git diff` when provably
unchanged; otherwise the real `git diff`. Windows and network filesystems always use the real `git diff`. Baseline
elided when `diff-index` returned 0 (`SNP_VERIFY_BASELINE=1` asserts in CI). Escape hatch `SNP_DIFF_MODE=git`.
**Consequences.** A 10 000-iteration randomized soundness test against real `git diff` gates the oracle; Windows
budgets are ratios to pre-commit (ADR-043).

### ADR-028 — Python two-layer stdout model (piped ordering)

**Context.** perf and robust printed chronologically; pre-commit's `print()` goes through CPython's text layer (8 KiB
block buffer when piped) while `output.write` flushes the byte layer, so piped output reorders (01 §8.2: validation
blocks after later warnings) — which is what CI logs and difftests see. **Decision.** `Printer::py_print` reproduces the
text layer exactly (8192-byte pending buffer when stdout is not a TTY, line-buffered on a TTY, flushed at `finish`);
`write*`/`log` are buffer writes + flush. Every pre-commit `print()` site uses `py_print`. **Consequences.** Piped and
TTY transcripts are both Tier A.

### ADR-029 — Windows text mode, colour, `NO_COLOR`

*Status: Accepted; amended by ADR-074 (VT re-enabled after inherited-stdio children; Windows env-name case). The amending ADR is normative where they differ.*

**Decision.** `py_print`/`py_eprint`/argparse/`SystemExit` texts emit CRLF on Windows (Python text-mode stdio; 14 Q2's
LF proposal rejected); hook output and status lines stay LF (byte layer). Windows colour `auto`: enable VT on the
**stderr** console handle with `winapi-util` (safe API; exactly `color.py`); `anstyle-query` banned. `NO_COLOR`,
`FORCE_COLOR`, `CLICOLOR_FORCE`, mintty detection: ignored in both brands (parity; `PRE_COMMIT_COLOR`/`--color` exist).

### ADR-030 — `clean` and `gc` output parity

**Decision.** `clean` always prints `Cleaned {store}.` (pre-commit re-creates its store before `clean`, 03 §10), even
when snp's store did not exist; it never touches `~/.pre-commit` or pre-commit's store (D-STORE-02). `gc` prints
`{n} repo(s) removed.` counting `(db_repo_name, rev)` keys; snp-only notes (1.x cleanup) go to stderr.

### ADR-031 — pre-commit-hooks fast path: tiers, gating, default `auto`

**Decision.** ARCHITECTURE §7.2: Tier A (known tag or its full commit: embedded manifest/profile, no clone/venv/network
when all selected hooks of the repo are native) and Tier B (normal clone, blob ids of module + import closure verified);
gates G1–G6; default mode `auto` = NATIVE plus NATIVE-WITH-CAVEATS hooks whose CI parity gate passed (`check-yaml`,
`check-toml`, `check-xml`, `detect-aws-credentials` start delegated until their gate file says otherwise); `exact` =
NATIVE only; opt-outs `--no-fast-path`, `SNP_NO_FAST_PATH`, root `fast_path: off`, `fast_path_exclude`, per-hook
`language`/`entry`/non-empty `additional_dependencies`. Per-tag epochs from v3.0.0; older revs delegate.
**Rationale.** FD-8 (default-on for verified upstream revs) with byte parity as the gate.

### ADR-032 — Cold-run transcript parity: keys markers, virtual installs, delegation before installs

**Context.** No proposal fully specified the cold transcript with the fast path; perf dropped the lines (D-FAST-1);
compat installed delegated hooks lazily (lines mid-run). **Decision.** (1) `[INFO] Initializing environment for
{db_repo_name}.` prints exactly when pre-commit would insert a db row: first sight of `(db_repo_name, rev)` by this
store, tracked by `keys-v1` markers, in config order at pre-commit's moment, for every repo kind including Tier A repos
that are never cloned and locally derived deps variants. (2) For a fast-path repo, the `Installing environment` triplet
prints exactly once for the python `InstallKey` pre-commit would have built (a *virtual* install marked in `keys-v1`).
(3) Delegation is decided before `install_envs`, so delegated hooks install at pre-commit's step with the others.
**Consequences.** Cold and warm transcripts are Tier A with the fast path on; `clean` resets the markers.

### ADR-033 — Python environments via uv

*Status: Accepted; amended by ADR-073 (`SNP_PYTHON_INSTALLER`). The amending ADR is normative where they differ.*

**Decision.** ARCHITECTURE §6.2 row python: uv from `$SNP_UV` / sibling / PATH (≥ 0.8.0, flags probed) / pinned
sha256-embedded download; `--no-config` with explicit pip-config translation; pip backend fallback when untranslatable;
interpreter choice reproduces pre-commit's; managed CPython download only for an explicitly requested missing version
(`SNP_PYTHON_DOWNLOADS=never`); isolated uv cache unless `UV_CACHE_DIR`; `UV_*` strip list. **Rationale.** 15–30 ms
venvs vs 250–400 ms (04a §11); semantics preserved.

### ADR-034 — Verified toolchain downloads

*Status: Accepted; amended by ADR-077 (two-pass archive extraction; checksum manifests always over TLS). The amending ADR is normative where they differ.*

**Decision.** snp downloads uv, node, go and rustup-init itself, always over TLS and verified by sha256 before
extraction (embedded for uv and the ruby helpers; `SHASUMS256.txt`, go.dev JSON, `.sha256` sidecars otherwise); safe
extraction; `SNP_OFFLINE`, mirror overrides, `SNP_ALLOW_INSECURE_MIRROR` for `http://`. Other toolchains come from the
system, as in pre-commit. **Consequences.** D-SEC-08 (pre-commit verifies nothing).

### ADR-035 — Docker image tag from the repo key

**Context.** pre-commit tags `pre-commit-<md5(basename(prefix))>` (04a §7.1); snp's prefixes are all named `src`, so
copying the rule gives every docker repo the same tag and, with "skip rebuild if the image exists", runs another repo's
image (compat and perf flaw). **Decision.** Tag = `pre-commit-` + md5(hex of the `RepoKey`) (lowercase); label
`PRE_COMMIT` kept. The per-run `docker build` (pull=False) is skipped when `docker image inspect <tag>` succeeds
(equivalent: same immutable context; output was never shown), `SNP_DOCKER_ALWAYS_BUILD=1` restores it.

### ADR-036 — Global output memory budget

*Status: Accepted; amended by ADR-077 (spill files are unnamed). The amending ADR is normative where they differ.*

**Context.** Per-partition caps (64/32/8 MiB) make RSS O(cores × cap) (≈ 1.4 GB on 22 cores). **Decision.** One shared
`OutputBudget` (default 64 MiB, `SNP_OUTPUT_BUDGET`) across all running children; a capture exceeding the remaining
budget continues into `<store>/spill/` (or the OS temp dir); printing streams from the spill. Stash patches are always
streamed to disk. **Consequences.** Peak RSS is independent of CPU count (B12).

### ADR-037 — fd hygiene under concurrent spawning

**Context.** pre-commit spawns with `close_fds=True`; Rust relies on CLOEXEC; on macOS std's pipe creation and
`posix_openpt` are not atomically CLOEXEC, so parallel partitions can leak a pipe write end into a sibling's child and
hang daemonising hooks (eslint_d, gradle). **Decision.** Linux: atomic CLOEXEC everywhere (std `pipe2`, pty master
`openpt(CLOEXEC)`), no lock. macOS/other Unix: `snp_core::spawnlock::guard()` serialises every `spawn()` and every
pipe/pty creation snp performs (spawns are ~0.1–1 ms, so the cost is negligible). Windows: std serialises handle
inheritance. **Consequences.** S15 test (N partitions, one daemonising child).

### ADR-038 — `SNP=0` never skips the legacy hook; `allow_disable`

**Context.** 05 A6 had the shim `exit 0` on `SNP=0`, which also skips the chained `<type>.legacy` hook (git-lfs
pre-push would push pointers without objects); compat and perf copied it. **Decision.** The hook script never checks
`SNP` when a binary exists; `hook-impl` runs the legacy hook first and then, if `SNP` is `0`/`false` and the config does
not set root `allow_disable: false` (missing/invalid config ⇒ allowed), returns the legacy rc without running snp's
hooks. With no snp binary at all, the script's kill-switch branch execs the legacy hook directly (or exits 0). `snp run`
ignores `SNP`. `SNP_COMPAT_STRICT=1` ignores `SNP`. **Consequences.** D-EXT-05.

### ADR-039 — Extension key policy

*Status: Accepted; amended by ADR-080 (`--require-group`, `@ungrouped`). The amending ADR is normative where they differ.*

**Decision.** Plain snake_case keys; prek names/semantics where prek defined them (`priority`, `priorities`, `groups`,
`env`); pre-commit keys are never re-typed (prek's `files: {glob: …}` rejected); recognised extension keys produce no
warning in snp (D-EXT-01) and are added after pre-commit's cfgv items so error precedence is unchanged; **unknown keys
including `x-*` produce pre-commit's exact warnings** (prek's silent `x-*` rejected: not an allowed divergence class);
`SNP_COMPAT_STRICT=1` treats extensions as unknown keys, disables the overlay and the kill switch (differential mode).

### ADR-040 — Poison recovery on safety paths

*Status: Accepted; amended by ADR-066 (there is no process-wide restore registry). The amending ADR is normative where they differ.*

**Decision.** Every `Mutex`/`RwLock` touched on a safety path (Printer, restore registry, JobPool, child registry,
Limiters, caches read during restore) is locked through `snp_core::sync::{lock, read, write}`, which recover the guard
with `PoisonError::into_inner`. **Rationale.** With `panic = "unwind"` and scoped workers, a worker panicking while
holding the printer lock must not silence restore messages or the crash banner. **Consequences.** Panic-injection test.

### ADR-041 — Tiered lazy tags; unreadable and vanished files

**Decision.** ARCHITECTURE §2.3: name → lstat → ≤ 1 KiB head (+ shebang) tiers driven by each filter's `Needs`; git
index modes give the stat tier for free after the stash on Unix with `core.fileMode`/`core.symlinks`; vanished files are
excluded (D-CRASH-08); an unreadable file aborts (exit 3) only if a selected hook needs its content (D-CRASH-18);
FIFOs/devices never block (D-CRASH-09). **Rationale.** Exact by construction (filter outcome depends only on referenced
tag categories); typical runs read zero file contents.

### ADR-042 — Workspace (multi-project) mode deferred to v1.1

*Status: Accepted; amended by ADR-069 (literal ids first; `dir:hook` only in v1.1 workspace mode when nothing literal matches). The amending ADR is normative where they differ.*

**Decision.** v1 behaves like pre-commit (one config; nested configs never auto-run). Seams in v1: `ConfigSource::
project_root`, `Vec<Project>` planning (length 1), `project_prefix` filters, per-project `ResolveCtx`, reserved
`dir:hook` selectors (rejected in v1). v1.1: explicit opt-in only (`snp run --workspace` / root `workspace: true`).

### ADR-043 — Performance budgets per OS

*Status: Accepted; amended by ADR-086 (`reference_only` micro thresholds) ADR-090 (the regression rule's absolute floor; history from other runner VMs is reported, not gated), ADR-091 (`[[known_miss]]`), ADR-099 (the paired regression rule on interleaved base and head rounds) and ADR-100 (B8 budgeted by heap peak, not tree RSS; counter gates). The amending ADR is normative where they differ.*

**Decision.** ARCHITECTURE §12.1. Linux hard gates: B1 ≤ 3 ms, B2 ≤ 10 ms, B3 ≤ 25 ms, B12 ≤ 30 MB. macOS soft
budgets (B1 ≤ 5 ms, B3 ≤ 40 ms). Windows soft ratios to pre-commit (B1 ≤ 15 ms, B3 ≤ 0.5×) because every per-hook `git
diff` remains a 10–30 ms spawn (no sound stat oracle without `unsafe`). Never slower than pre-commit or prek at p50.

### ADR-044 — Store ownership: warn, refuse only world-writable

*Status: Accepted; amended by ADR-077 (root refuses a store owned by another uid unless `SNP_ALLOW_FOREIGN_STORE=1`). The amending ADR is normative where they differ.*

**Context.** robust refused stores not owned by euid or group-writable, breaking containerised CI with volume-mounted
caches where pre-commit works. **Decision.** Created directories are 0700; a world-writable non-sticky store is refused
(symlink-planting risk); a store owned by another uid produces one stderr warning; group-writable is accepted.

### ADR-045 — K+ recovery hint on stderr

**Decision.** The stash-restore-failure hint (`[ERROR] Your unstaged changes are saved in <patch>; restore them with:
git apply <patch>`) and the next-run recovery notice go to **stderr**, after line 2 of the envelope; stdout stays
byte-identical (13 X.9 Q1). **Consequences.** D-DATA-01.

### ADR-046 — Legacy hook stdin parity

**Decision.** The legacy hook receives the captured stdin only for pre-push and an empty stdin for every other type,
post-rewrite included (pre-commit 02 §16, 03 §6). robust's forwarding of post-rewrite stdin is rejected (not a crash,
data-loss or security class).

### ADR-047 — Speculative config load

**Decision.** During the read-only git fan-out the main thread reads and validates the config with warnings buffered;
the result is used iff the "config unstaged" query returned rc 0 (the stash cannot change the file), and the buffered
warnings are printed at pre-commit's position; otherwise the config is loaded again inside the stash. Adopted behind a
benchmark showing a measurable win on B3/B9; removable without API change. **Rationale.** Exact by construction (S13).

### ADR-048 — Hook script template and fallback chain

*Status: Accepted; amended by ADR-060 (stable `SNP_EXE` selection, M-lines on stderr, prek/snp-1.x adoption registered as D-BRAND-02). The amending ADR is normative where they differ.*

**Decision.** ARCHITECTURE §9.7 (11 §A.6 + the kill-switch branch): POSIX `sh`, LF everywhere, three identity lines
(snp ID `5bb9bd89cf6c92c58fee7677ceaf4fc3`, pre-commit `CURRENT_HASH`, prek ID) so the last installer wins and no
`.legacy` chains form; argv byte-compatible with pre-commit's `hook-impl`; fallback `$SNP_BIN` → embedded `SNP_EXE` →
`snp` on PATH → `pre-commit` on PATH → (kill switch: legacy hook / exit 0) → fail closed exit 1; written atomically,
replacing symlinks; foreign `.legacy` never clobbered without `-f`; manager-script `.legacy` removed.

### ADR-049 — Distribution and the `pre-commit` name

*Status: Accepted; amended by ADR-088 (crates.io and Homebrew channels). The amending ADR is normative where they differ.*

**Context.** 11 §A.1 rules out installing a `pre-commit` executable by default (no silent takeover); a judge suggested
cargo-dist `bin-aliases` as a cheap alias channel. **Decision.** Release with cargo-dist (Linux gnu/musl × x86-64/arm64,
macOS × 2, Windows msvc × 2; auditable binaries, checksums, attestations); PyPI `snp-cli` (maturin `bindings = "bin"`,
binary `snp` only; PEP 541 request for `snp`); the `pre-commit` name only through the opt-in `snp self alias install`
(symlink/hard link) and, in v1.1, an opt-in `snp-pre-commit-compat` distribution; `bin-aliases` is not used for the
default installers (it would silently shadow Python pre-commit).

### ADR-050 — Version number

*Status: Accepted; amended by ADR-088 (snp 2.x takes over the crates.io name `snp`) and ADR-089 (`PRE_COMMIT = "4.7.0"`, snp 2.0.1). The amending ADR is normative where they differ.*

**Decision.** Workspace version `2.0.0` (crates.io `snp` 1.x exists, 11 §A.1). `PRE_COMMIT = "4.6.2"` is the value
compared by `minimum_pre_commit_version`; `SNP_VERSION` is compared only by `minimum_snp_version`.

### ADR-051 — Git version policy

*Status: Accepted; amended by ADR-057 (`--git-config-hooks`) and ADR-063 (no blanket refusal below 2.19). The amending ADR is normative where they differ.*

**Decision.** `git --version` cached in `cache-v1` per (exe path, size, mtime, `GIT_EXEC_PATH`), trusting only passing
entries. < 2.19 → FatalError before any write; 2.19–2.30 → one stderr warning (`SNP_ALLOW_OLD_GIT` silences) and
fallbacks (adjacent dedup for `ls-files`); ≥ 2.31 silent; floor tested: 2.31. Cygwin/MSYS2 git on Windows → clear
error (D-PLAT-08). git ≥ 2.54 config-based hooks: `snp install --config-hooks` (snp brand); `rest[1:]` kept.

### ADR-052 — Non-UTF-8 paths and text

*Status: Accepted; amended by ADR-056 (MERGE_MSG conflict lines are parsed on bytes, never decoded: D-CRASH-12). The amending ADR is normative where they differ.*

**Decision.** File names are bytes end to end (argv, identify); regex matching uses the lossy UTF-8 view; messages
print bytes verbatim; where parity requires decoding (pre-push stdin, MERGE_MSG, config text) the exact Python decode
and error text are used, except pre-push (ADR-022). `SKIP` uses `strip_fsbytes`. Locale encoding is never used: UTF-8
always (D-DATA-09). **Consequences.** D-CRASH-11/12/13.

### ADR-053 — Recovery of the research base

**Context.** The scratch directory holding the research (~17 k lines), the proposals and the regex corpus was wiped by
a reboot during synthesis. **Decision.** The documents were reconstructed from the authoring transcripts and committed
under `docs/research/` with a provenance README listing the small gaps (12 §A.4 help blocks, 09 Appendix A, a few lines
of 01/10). The packages that need the lost verbatim material regenerate it from the oracles (WP-12, WP-26/27) and the
lost pycompat vectors from the committed generator (WP-11); the regex corpus is rebuilt from the Addendum's test plan
(WP-13). **Consequences.** Spec references point to `docs/research/`, which is durable.

### ADR-054 — Testing architecture: two oracles, tiers, divergence register

*Status: Accepted; amended by ADR-058 (snp-brand runs use the prog-patched oracle), ADR-062 (namespaced catalog IDs), ADR-071 (per-ID divergence-file ownership) and ADR-089 (the oracles are `pc470-py312`/`pc470-py313`, pre-commit 4.7.0). The amending ADR is normative where they differ.*

**Decision.** ARCHITECTURE §13: `xtask oracle install` builds hash-locked `pc462-py312` (CPython 3.12.3: runtime and
regex oracle, the 10 §0 baseline) and `pc462-py313` (CPython ≥ 3.13.14: CLI-text oracle); comparison tiers A/B/C; snp
runs through a `pre-commit` symlink (compat) and as `snp` (brand_map); every divergence has an ID, a
`docs/divergences/<ID>.md` and a `tests/differential/divergences/<ID>.yaml`; the harness fails on unlisted diffs and on
listed diffs that stop reproducing; vectors and goldens are committed (CI needs no Python except the difftest job).

### ADR-055 — Work plan: contracts first, waves, ownership

*Status: Accepted; amended by ADR-071 (W0 baselines, per-module stub expectations, divergence-file ownership, WP-26 M1). The amending ADR is normative where they differ.*

**Decision.** WBS.md: W0 one skeleton package (all crates, manifests, lint/profile config, complete
`[workspace.dependencies]`, every public API as compiling stubs with `tests/api.rs`, snp-core implemented, the pycompat
prototype vendored, baseline spawn, CI and xtask); W1 8 leaf packages; W2 7 dependent packages; W3 integration, binary,
differential corpus, performance gates, release and docs. Owned paths are disjoint within a wave; shared files are
pre-created by WP-00 so owners only add files; `Cargo.lock` is regenerated, never hand-merged; signature changes need an
ADR. **Rationale.** Maximum parallelism without merge conflicts; compile-time contract checking.

### ADR-056 — snp-pycompat: the frozen production contract

**Context.** ARCHITECTURE §1.4.2 pointed at 15 §A6 "verbatim", but 15 §A6 is partly pseudo-Rust (`split / dirname / …
(p, env: impl Fn…)` for ntpath, `pub mod ospath { /* facade */ }` with no signatures although every caller uses it),
names undefined types (`SplitLines`, `SplitWs`, `BytesSplitLines`, `PyChar`, `Utf8Reason`, `IntParseError`, the `Unit`
trait, the home of `UnpackError`) and conflicts with the prototype WP-00 was told to vendor (`zsplit(&str) -> Vec<&str>`,
`get_skips(&str) -> BTreeSet<&str>`, `merge_msg_conflicts -> Vec<String>`, `exe_exists_pure(Flavor, &str, &str)`; no
`random`, `eaw`, `float`, `chars`, `ospath`; a `regex` dependency at the bottom of the DAG; a `tables` module that is not
in the recovered sources). Every W1 package codes against this crate (review finding, critical).
**Decision.** ARCHITECTURE §1.4.2 now lists the complete production API as real Rust, frozen under the signature-change
rule: named iterator types (no `impl Trait` returns), `Display` + `Error` + `py_type()` on every error type, an `ospath`
facade over `OsStr`/`Path` with explicit cwd and an injected `HomeLookup`, `ntpath` generic over `Unit` (u8/u16), and the
`precommit` composites `zsplit(&[u8]) -> ZSplit`, `get_skips(Option<&OsStr>) -> Vec<OsString>` (first-seen order),
`PrePushLine` with pre-commit's field names, `pre_push_lines(_bytes)`, `merge_msg_conflicts(&[u8]) -> Vec<&[u8]>` (bytes,
never decoded — D-CRASH-12 already covers X13-GIT-07, so ADR-052's mention of MERGE_MSG decoding is superseded),
`exe_exists_pure(&OsStr, &OsStr)`. No dependencies besides std; `regex` is banned outside snp-pyre. The Unicode tables
are generated by `gen_pycompat_tables.py` under CPython 3.13 (UCD 15.1; the generated file also supplies the
prototype's missing `tables` module). WP-00 vendors the prototype **adapted** to these signatures (thin wrappers;
`rev_line_match_manual` becomes `rev_line_match`), keeping its unit tests.
**Consequences.** W1 packages rely on W0's composites; WP-11 completes behind a stable contract.

### ADR-057 — `--git-config-hooks` instead of `--config-hooks`

**Context.** `install` and `uninstall` have `-c/--config`, so `--con`, `--conf` and `--confi` are unique abbreviations
today. Adding `--config-hooks` makes them ambiguous (verified with CPython 3.13 argparse: `pre-commit install: error:
ambiguous option: --conf could match --config, --config-hooks`) — the same break that rejected `--files-from-stdin`;
WP-12's prefix-stability acceptance could not pass.
**Decision.** The snp-brand flag is `--git-config-hooks` on `install` and `uninstall` (no existing option of either
parser starts with `--g`; field `git_config_hooks`). The prefix-stability fixture plants both rejected names
(`--files-from-stdin`, `--config-hooks`) and requires `prefix_stable` to fail on each. Supersedes the flag name in
ADR-051 and in Appendix A (02 G-3, 03 Q14, 11 Q12).

### ADR-058 — snp-brand argparse texts are rendered with prog `snp`, not substituted

**Context.** The snp brand was defined as the compat bytes with `brand_map` substitutions. argparse wraps usage, help
and usage-error blocks relative to `usage: {prog} {cmd} `; with `snp` the first line is 7 characters shorter, so wrap
points and continuation indentation change (`init-templatedir`: continuation indent 35 → 28 columns). `brand_map(oracle)`
can never equal snp's output for those blocks, and a plain substitution turns `pre-commit 4.6.2` into `snp 4.6.2`.
**Decision.** snp-argparse renders every snp-brand argparse text from `SNP_SPEC` with prog `snp`. Goldens are checked
against two CPython 3.13.14 references produced by `xtask cli-goldens`: (a) the pc462-py313 oracle with argparse `prog`
patched to `snp` (a launcher installed by `xtask oracle install`) for parsers whose snp spec equals pre-commit's, and
(b) a real `argparse.ArgumentParser` built from `snp_argparse::export_json(SNP_SPEC, "snp")` for parsers with snp-only
options or commands. `brand_map` covers only non-argparse texts and gains the whole-line `--version` row
(`pre-commit 4.6.2` → `snp 2.0.0`). The difftest's snp-brand runs use the prog-patched oracle plus `brand_map`.
**Consequences.** K2 holds byte for byte in the snp brand; WP-12 acceptance rephrased; COMPATIBILITY §2–§3.3 updated.

### ADR-059 — try-repo runs against a throwaway store

**Context.** pre-commit's `try_repo` uses `Store(tempdir)` (03 §12 step 3), so every try-repo prints `[INFO]
Initializing environment for …` and the `Installing environment` triplet (13 SM-01). The design never said which store
`try_repo` uses; `RunCtx.store` would drop those lines on the second run (a Tier A transcript break) and fill the
persistent store with keys of temporary shadow repos.
**Decision.** `Store::ephemeral(tmp, Some(&main))`: `repos-v1`, `keys-v1`, `configs-v1`, `patches/` and the global lock
live in the TempDir; `tools-v1` and `cache-v1` come from the main store (invisible: toolchain downloads are snp's own
and print nothing). The temp config is never tracked; the user's `-c` is tracked in the main store as for every git
command. Acceptance: both line groups print on two consecutive runs; the main store gains no `repos-v1` entry.

### ADR-060 — Installing over other managers' scripts: messages, stable `SNP_EXE`, hook-time brand

**Context.** ADR-048 adopted 11 §A.5's in-place replacement of prek and snp-1.x scripts and its notes M1/M1b/M2/M3/M12
without deciding stream or brand; in the compat brand `pre-commit install` over Python pre-commit's own script would
print M1 on stdout (pre-commit overwrites its own scripts silently) — an unregistered byte break. How `SNP_EXE` is
chosen was undefined: Linux `current_exe()` resolves to versioned paths (Linuxbrew Cellar, mise, cargo-binstall) that
vanish on upgrade, after which GUI clients whose PATH lacks `snp` fail closed on every commit. The brand at commit time
after `pre-commit install` through the alias was undefined.
**Decision.** (1) M1, M1b, M2, M3 and M12 go to **stderr** in both brands (D-DIAG-02); pre-commit's own stdout lines are
unchanged. (2) prek and snp-1.x scripts are manager scripts replaced in place — registered as D-BRAND-02 (pre-commit
moves them to `.legacy` and prints migration-mode lines). (3) `SNP_EXE` = `snp_cmd::scripts::select_exe` = the 11
§A.19.3 algorithm with the installing brand's own name: `absolute(arg0)` if it contains a separator, then absolute PATH
entries joined with `snp[.exe]` (snp brand) or `pre-commit[.exe]` (compat brand), then `current_exe()`; first candidate
that is a file and the same file as `current_exe()`; never canonicalised. A compat-brand install therefore embeds the
alias path and commits keep the compat brand (K3); `is_snp` follows 11 §A.19.4. (4) The `run` hint stays TTY-only stderr
(D-DIAG-03). **Consequences.** WP-33 cites 11 §A.19; test: install through a symlinked bin dir, delete the versioned
target, commit.

### ADR-061 — Pinning the sibling user crates in CI and releases

**Context.** cfgv and file-identify are path dependencies outside the repository (`../cfgv-rs`, `../file-identify`,
branch `snp-support`). Nothing said how GitHub CI, the platform matrix, nightly fuzzing or the cargo-dist/maturin
release builds get those checkouts or at which commit; `cargo build --locked` fails on a clean `grok-rs/snp` checkout;
08 §8.3's `[patch.crates-io]` plan had no replacement; crates.io publication needs both crates published first.
**Decision.** WP-00 commits `deps.lock` (repository URL and commit SHA of each user crate). Every workflow checks both
repositories out as siblings at those SHAs before any cargo command; a scheduled drift job compares the SHAs with the
`snp-support` heads and opens an issue; bumping a SHA is an ordinary PR that runs every gate (including the
`user_crates.rs` API tests of ARCHITECTURE §1.5). Release gates (WP-37): cfgv 0.3.x and file-identify 0.5.x are
published on crates.io from exactly the pinned commits (the published `.cargo_vcs_info.json` commit equals `deps.lock`),
and the release builds from a clean checkout plus the pinned siblings. `[patch.crates-io]` with git revisions is not
needed: path + version requirements already resolve to the registry versions on publication.

### ADR-062 — Test-catalog IDs: namespaced by source, every row owned

**Context.** The WBS cited rows that do not exist (GIT-19, SFO-24, HKI-13, INS-42, AUP-19, CLI-10, ERR-08; 10 §5 stops
at GIT 13 (14 rows), SFO 17, HKI 09, INS 34, AUP 17, CLI 07, ERR 05), left families unowned (STO-01…13, UTL-01…07 except
UTL-05, the 29 CFG rows of 10 §5.1), and prefixes collide (CFG/GIT in 10 §5 vs 13 X.4; MIG-01…09 of 10 §5.12 vs
MIG-01…24 of 11 §A.20); `xtask catalog-status` had no owner.
**Decision.** IDs carry their source: `T10-<FAM>-nn`, `X13-<FAM>-nn`, `M11-MIG-nn`; tests are named
`t10_cfg_01_<slug>` etc.; WP-00 generates `tests/catalog/index.tsv` once from the research documents; ownership is the
WBS "Catalog ownership" table (family owner plus row exceptions); every WBS acceptance range equals the real rows;
`xtask catalog-status` is owned by WP-35 (ARCHITECTURE §13.8).

### ADR-063 — Git version policy, revised: no blanket refusal; class DIAG

**Context.** ADR-051 refused git < 2.19 (exit 1), but pre-commit 4.6.2 on git 2.18.5 runs the default staged run, the
stash, `install` and `commit` correctly (02 G.3); only an unstaged intent-to-add file breaks its stash (rollback, then
`already exists in working directory`, exit 3, work only in the patch). Below 2.31 pre-commit's `--all-files`, pre-push
root push and meta hooks crash on `ls-files --deduplicate` (exit 3) while snp falls back. Neither difference was
registered (S5/K4), and the always-on old-git warning sat in EXT, a class defined as "only when an extension is used".
**Decision.** `GitExe::version_gate` never refuses. Below 2.31: one stderr warning per process (D-DIAG-01;
`SNP_ALLOW_OLD_GIT` silences) and the capability fallbacks; the `--deduplicate` fallback is D-CRASH-26. Below 2.19 only a
stash that would be non-empty is refused, before any mutation (D-DATA-17: snp cannot detect ITA files there and
pre-commit then strands the work in the patch); every run without a stash works as in pre-commit. COMPATIBILITY gains
the class **DIAG** (stderr-only, always on, stdout/exit unchanged) holding D-DIAG-01…05; D-EXT-03 keeps only the overlay
notices. CI runs `expect_divergence` scenarios on the 2.18.5 and 2.30 containers. Config hooks still need ≥ 2.54.

### ADR-064 — Clone prefetch: every failure is a silent cache miss

**Context.** ADR-024 retried in the foreground only "after an auth failure" detected from git's (localised) stderr and
"not under CI". Under CI the reported CalledProcessError came from the background attempt (`terminal prompts disabled`,
not pre-commit's text), breaking X13-GIT-01 on every private-repo failure. `GIT_TERMINAL_PROMPT=0` does not stop ssh
passphrase or host-key prompts on /dev/tty, askpass GUIs or credential-manager dialogs, so up to 8 background clones
could prompt at once (keystrokes, passphrases included, reaching an arbitrary prompt).
**Decision.** Background prefetch only for `prefetch_eligible` URLs: https://, file:// and local paths, and ssh/scp only
when none of `GIT_SSH_COMMAND`, `GIT_SSH`, `core.sshCommand` is set (then `GIT_SSH_COMMAND='ssh -o BatchMode=yes'` is
injected); background env adds `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=` (empty: no askpass GUI) and
`GCM_INTERACTIVE=never`. Any prefetch failure is discarded (scratch dir removed, stderr never shown) and the clone is
redone in the foreground exactly as pre-commit runs it — same env, prompts, moment — under CI too. Ineligible URLs are
cloned only in the foreground, serially, in config order. `SNP_PREFETCH=0` disables prefetching. Tests (WP-31): private
repo with `LANG=de_DE.UTF-8`, with `CI=1`, and an ssh URL with a passphrase-protected key (exactly one foreground
prompt).

### ADR-065 — Process control: roles, cancellation, origin-aware signals, descendants, reads, pool sizing

**Context.** Review findings: `registry::kill_all` would SIGKILL restore-critical `git apply` when hooks die promptly
and the restore starts inside the 2 s grace (S3); `Spec.cancel` had no semantics and the stash's `Git` shared the
cancelled run token; `CancelReason::FailFast` had no owner and, on the run/pool token, would stop permits for running
bucket partitions; forwarding missed timed hooks (own process group, no terminal ^C), missed `kill -INT <snp>`, and
re-sent kernel-generated TERM/HUP (double signal = "force quit" for npm, cargo, uv); grandchildren survived
cancellation and could keep rewriting files during or after the restore; reads until EOF could block cancellation
forever when a descendant held the pipe or pty; macOS `poll` does not support pty masters; the pty slave flags were
unspecified (missing `O_NOCTTY` gives a session-leader snp a controlling terminal and SIGHUP; missing `O_CLOEXEC` leaks
the slave into sibling spawns); a restore command touching the tty stops on SIGTTIN/SIGTTOU in its background group and
hangs; Windows CTRL_CLOSE/LOGOFF/SHUTDOWN end the process when the handler returns; the pool ignored cgroup quotas; up to
four compile-heavy installs ran in parallel where pre-commit installs serially; RLIMIT_NOFILE (prek P15) was ignored.
**Decision.** (1) `spawn::Role {Hook, Tool, Critical}`: Critical commands (stash checkout, applies, rollback checkout,
ITA rm/re-add, stage_fixed add) run Isolated, are never signalled or killed, ignore `cancel`, get
`GIT_TERMINAL_PROMPT=0`, on Windows `CREATE_NO_WINDOW`, and are reaped with `WSTOPPED` (a tty stop SIGKILLs the group and
fails the command → rollback/K+ path). (2) `Spec.cancel` never prevents a spawn; for Hook/Tool children it starts the
escalation and bounds the output wait. The stash builds its own `Git` with `ProcBackend { role: Critical, cancel:
CancelToken::new() }` (never cancelled). `CancelReason::FailFast` is removed (fail_fast is a scheduler decision) and
`Panic` added. (3) ARCHITECTURE §10.4's table is normative: `SignalsInfo<WithOrigin>`; kernel-origin signals are not
re-sent to Inherit children; signals sent by a process (SIGINT included) are forwarded; Isolated hook groups always get
the signal by `-pgid`; after `SNP_KILL_GRACE` SIGKILL by pid / `-pgid`. (4) Linux: `PR_SET_CHILD_SUBREAPER` for
run/hook-impl/try-repo; on the cancel path only, `registry::quiesce` SIGKILLs and reaps every remaining non-Critical
descendant before the restore; Windows: hooks run in a per-run Job Object (no KILL_ON_JOB_CLOSE, so daemons survive
normal runs) terminated on cancel; macOS: residual risk documented in D-DATA-02. (5) Output is read by dedicated
reader threads feeding bounded channels — no `poll` anywhere: EOF on normal runs (parity), kill + 500 ms drain then
abandon on deadline/cancel. (6) pty slave `open(O_RDWR|O_NOCTTY|O_CLOEXEC)`, under the spawn lock on macOS. (7) Windows
console close/logoff/shutdown are journal-recovered events. (8) `pool_size`: an explicit `-j`/`SNP_JOBS`, else
`min(target_concurrency, available_parallelism)` (cgroup-aware; partitions still use `target_concurrency`), clamped to
`(RLIMIT_NOFILE soft − 64) / 8`; the limit is not raised (children would inherit it). InstallLimiter `min(N, 4)`, 1 under a
cgroup memory limit < 4 GiB; compile-heavy installs (rust, golang, haskell, swift, dotnet, dart, conda) take every
permit. **Consequences.** The §10.4 table is the posix signal-test matrix; FI-25…FI-30 added.

### ADR-066 — Panic-freedom hardening and panic-path ownership

*Status: Accepted; amended by ADR-095 (a `reason = "verus: …"` expectation on a verified function of a mounted kernel file is a proven site). The amending ADR is normative where they differ.*

**Context.** S2's lint set missed panics from arithmetic overflow (`overflow-checks = true`), `assert*!`, panicking std
APIs (`Duration::from_secs_f64` on a hostile `SNP_KILL_GRACE`, `split_at`, `chunks(0)`, `step_by(0)`,
`Vec::{remove, insert, drain}`, `String::truncate` off a char boundary, capacity overflow, `RefCell`) and thread-spawn
failures (`Scope::spawn` panics on EAGAIN under cgroup `pids.max`/RLIMIT_NPROC). The process-wide restore registry of
ARCHITECTURE §4.2 step 7 had no API and no owner; a panic hook that locks the Printer or a registry self-deadlocks when
the panicking thread holds it; declaring the guard as the last `RunState` field restores after every other field's
Drop, and a Drop that panics during unwinding aborts before the restore.
**Decision.** Lints of ARCHITECTURE §1.3: `arithmetic_side_effects` and `unchecked_time_subtraction` denied (checked or
saturating arithmetic; `#[expect]` with a stated bound), `disallowed-macros` for `assert`/`assert_eq`/`assert_ne`,
`disallowed-methods` extended with the panicking APIs and `thread::spawn`/`Scope::spawn` (use `Builder` and map the
error); duration variables parsed with `try_from_secs_f64` and an upper bound; `overflow-checks` stays as a backstop.
No restore registry: the explicit `restore()` on the main thread, `impl Drop for RunState` (restores inside
`catch_unwind` before any field drops) and token cancellation cover every path. The panic hook stores payload,
location and backtrace in a `try_lock` slot and calls the lock-free `CancelToken::cancel(Panic)`; it never prints,
locks or restores. `snp_core::fault` (feature `fault-injection`, owner WP-00) provides the panic-injection points.

### ADR-067 — Stash hardening, round 2

*Status: Accepted; amended by ADR-083 (dismissal of orphaned-stash notices) and ADR-097 (the re-verification by stat stamps, the patch listed before it). The amending ADRs are normative where they differ.*

**Context.** Renaming the patch to `patch<secs>-<pid>` could replace a concurrent run's live patch (same second and pid
across PID namespaces sharing a store); the fsyncs between `diff-index` and `git checkout -- .` (F_FULLFSYNC: 10–50 ms
each on macOS) widened pre-commit's ~1 ms window in which an editor autosave is discarded; pid-based liveness fails under
pid reuse, across PID namespaces and for a GUI client using another store; the `git apply <patch>` hint fails after a
partial apply; a stale `_SNP_STASH_LOCK` token matching a stale lock file could skip the lock; `stage_fixed`'s `git add`
had no journaled state, breaking the reconstruction invariant on a kill.
**Decision.** The patch is created directly under its final name with `create_new` (EEXIST → `-n`) while being hashed;
journal and the git-dir pointer `<git_dir>/snp-stash.journal` are `create_new` too; one directory fsync; then a second
`diff-index` digest must equal the streamed one before the checkout (else rewrite, ≤ 3 tries, else FatalError before
any mutation). Only data-protecting states (`stashed`, `staging-fixed`) are fsynced. Liveness is the worktree lock:
the recovery scan runs with the lock held or acquired non-blocking, never in nested runs; the pointer makes recovery
store-independent. Every hint uses `git apply --3way <patch>` (valid: `--binary` patches carry full index lines and
their preimage blobs exist); the K+ text of D-DATA-01 changes accordingly. A nested token is honoured only while the
lock is actually held and is truncated on release. New state `StagingFixed {paths}` (durable, before the add) with the
recovery `git reset -q <tree> -- <paths>` + `git apply --3way`. Recording `boot_id` and the pid-namespace inode (proposed
by the review) is not adopted: lock-based liveness makes them unnecessary; the pid stays for display only.
`SNP_STASH_BACKUP` (05 A5 item 6) is rejected: a stash entry is user-visible state pre-commit never creates (the stash
list is a compared side effect, K3), and patch + journal + pointer already make every interruption recoverable.

### ADR-068 — Windows rename retries

**Context.** On Windows, renames and replaces fail transiently with PermissionDenied or sharing violations while
antivirus or the indexer holds new files (prek's `rename_with_retry`, 06 §5.6; 11 §A.19.2 mandates 5 retries with
50·2^n ms backoff for hook scripts). The design had no retry anywhere.
**Decision.** `snp_os::fs::rename_with_retry` retries ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION and
ERROR_LOCK_VIOLATION up to 5 times with 50·2^n ms backoff (≈ 1.5 s); every rename-based primitive uses it
(`atomic_replace`, `retire_dir`, store materialisation and marker commits, hook scripts, tool installs). Unix: one
attempt. Test: an open handle held during `materialize`, an install-marker commit and `snp install`.

### ADR-069 — Hook selectors and the workspace seams

**Context.** Rejecting `dir:hook` selectors in v1 would reject valid hook ids containing `:` (`lint:js`, `mypy:strict`),
which pre-commit accepts; the `project_prefix` seam claimed for snp-files was absent from its API.
**Decision.** A positional hook is always resolved first as a literal id or alias; `dir:hook` is a project selector only
in v1.1 workspace mode and only when nothing literal matches; in v1 an unmatched selector gives pre-commit's `No hook
with id …`. `HookFilter::compile` takes `prefix: Option<RepoPath>` and `FileTable::relative_to` exists now (used by the
`root` extension in v1, the same parameter serves the v1.1 project prefix). Amends ADR-042.

### ADR-070 — Cross-crate contract corrections (review round 2)

*Status: Accepted; amended by ADR-081 (the binary depends on snp-lang to build `RunCtx.probe`/`RunCtx.runner`; doctor still needs no snp-config edge) and ADR-082 (B3's spawn bound is ≤ 18). The amending ADRs are normative where they differ.*

**Context.** Several contracts could not compile against the DAG or forced same-wave packages to invent handshakes.
**Decision.** Reflected in ARCHITECTURE §1.4: (1) doctor's fast-path table comes from `snp_run::doctor_fast_path`,
wrapped by the binary in a `DoctorCheck` (no `snp` → snp-config/snp-lang edge); the stash-recovery check lives in
snp-cmd. (2) G5 is the `snp_repo::Preflight` seam in `ResolveCtx`; `all_hooks` makes the final native/delegate decision
and clones delegated Tier A repos. (3) `LOCAL_TEMPLATE_VERSION` lives in `snp_core::profile` (re-exported by snp-lang);
tool references are the store-owned `.snp-tools.json` written by `Store::commit_install_marker(env_dir, json, tools)`,
so `sweep` never parses `InstallState`. (4) `ToolDirs` is implemented by `snp_repo::StoreTools` (WP-31) over
`Store::tool_lock`; `RunCtx` carries the net/install Limiters, `ToolProbe` and `CmdRunner`; S12 names `ToolProbe`, whose
methods are specified. (5) `JobPool` is re-entrant: permit holders run nested work inline. (6) The overlay is split:
`apply_root` + `HookPatch` merged by `merge_hook`/`finalize_local` after manifest ∪ config. (7) `repo: builtin`
validation is part of snp-config's schema (`snp_hooks::builtin::validate` removed). (8) `RepoId::from_config` and
`db_repo_name` in snp-store are the only key derivation (snp-repo and gc). (9) The hook-script classifier is
`snp_git::hookscripts` (snp-cmd and the snp-run hint use it); the binary sequences `install` → `install_hooks` and
`migrate lefthook --install`. (10) `DiffOracle::new` receives the fan-out's `IndexEntries` and config origins (query G
joins the fan-out; B3 stays ≤ 17 spawns). (11) `GitExe::resolve` precedes `Git::discover`. (12) `snp_hooks::columns`
re-exports argparse's column rule for snp-run. (13) `Debug` supertraits on every seam trait, manual `Debug` for
`Printer` and `pygrep::Compiled`, `SpawnGuard` documented `!Send`. (14) `snp::entry() -> Exit { Code(u8), Raw(i32) }`
(`ExitCodeExt::from_raw` is unstable in 1.99.0). (15) snp-testkit's API is specified (`Bins`, `RepoBuilder`, `Cmd`,
`Fixture`); library code never reads `CARGO_BIN_EXE_*`. (16) `HookCx` and `pygrep::run_partition` carry a
`CancelToken`. (17) `FileTable` keeps one cell per tier. (18) `snp_hooks::framework` is frozen in §1.4.19. (19)
`SNP_TEST_SPAWN_LOG` lives in snp-proc (`testing`), fault injection in snp-core.

### ADR-071 — Work-plan mechanics, corrected

*Status: Accepted; amended by ADR-081 (xtask edges (cli, hooks, argparse); integration-lead files; shared manifests). The amending ADR is normative where they differ.*

**Context.** W1 packages were not real leaves (snp-git needs `normalize_cmd`, xargs partitioning and the pycompat
composites; snp-lang's `ProcRunner` needs `normalize_cmd`); a crate-level stub `#![expect]` in a `lib.rs` shared by
several owners has no legal remover; `todo!()` does not compile as an `impl Iterator` body; per-divergence files were
required from W1–W3 packages under paths WP-35 owned; WP-27 needed WP-26's framework and Appendix A goldens in the same
wave and omitted WP-13; snp-lang's `Cargo.toml` had no W2 owner; `parity_gates.toml` had no parser; xtask and
dev-dependency edges were outside `check-dag`; several acceptance items needed later waves; criterion thresholds never
fail a build.
**Decision.** (1) W0 ships working baselines of `resolve::{find_executable, normexe, normalize_cmd}`,
`xargs::{cpu_count, target_concurrency, platform_max_length, partition}`, `lock::FileLock`,
`fs::{write_new_durable, atomic_replace}` and the pycompat composites with their §1.4.2 signatures. (2) Stub files have
one owner and a per-file `#![expect(clippy::todo)]`; RPIT stubs are `core::iter::from_fn(|| todo!())`. (3) WP-00
pre-creates `docs/divergences/<ID>.md` and `tests/differential/divergences/<ID>.yaml` for every register ID; each ID's
files belong to the first WP of its register row; WP-35 owns the harness, scenarios, `brand_map.yaml`, READMEs, register
tooling and `catalog-status`. (4) WP-26 delivers the native framework and the Appendix A help goldens of every console
script as milestone M1 (first merge of W2); WP-27 depends on WP-26 M1 (the only intra-wave dependency; its affected
acceptance items are marked [M1]) and on WP-13. (5) WP-00 declares in `crates/snp-lang/Cargo.toml` every dependency the
§6.2 backends need, each referenced from its owning stub file (`use roxmltree as _;`), so `cargo machete` stays green.
(6) Parity gates are a generated Rust table `crates/snp-hooks-data/src/generated/parity_gates.rs` (WP-17 creates it,
WP-27 owns it in W2). (7) `check-dag` covers dev-dependencies (ARCHITECTURE §1.2 policy) and the xtask edges (cli,
hooks); WP-00 pre-declares the dev-dependencies of `crates/snp` that WP-36 needs. (8) Cross-wave acceptance items are
rephrased to the package's own API or marked [e2e]; WP-23 owns a test-only `snp-stash-driver` binary for its signal and
kill-9 matrices. (9) Timing thresholds are release-mode `#[ignore = "perf"]` tests with a 3× factor (`perf-smoke` CI job)
plus WP-36's `xtask bench gate` for the exact targets. Amends ADR-055.

### ADR-072 — Native hooks parse file names with argparse (exact parity)

**Context.** pre-commit appends file names to the argv of `pygrep.py` and of every pre-commit-hooks console script; a
tracked file named `-x.py` gives `unrecognized arguments` (exit 2) and one named `-i` is parsed as a flag. The native
design parsed only `args`/`entry` (04a §12.2 proposed the deviation; it was never registered).
**Decision.** Exact parity: per partition a native receives `args (+ [entry]) + files` and parses it with its console
script's argparse spec (`framework::parse_args`); pygrep recompiles its pattern only for a partition whose parse
differs from the prepared one. Cost: one linear parse per partition. Tests in the SEC suite. Amends ADR-021.

### ADR-073 — `SNP_PYTHON_INSTALLER`

**Context.** 04a §11.3 G7 / §11.4 step 1 call for an escape hatch for uv-vs-pip resolver differences (pre-releases,
yanked versions, extra-index strategy); the design fell back to pip only for untranslatable pip config or unknown `-`
dependencies. **Decision.** `SNP_PYTHON_INSTALLER = auto` (default: uv unless a fallback trigger fires) `| uv` (force,
fail instead of falling back) `| pip` (pre-commit's installer: `<interp> -m venv` + `pip install`), chosen per env and
recorded in `InstallState.installer` (a different choice reinstalls). D-PERF-01 mentions it. Amends ADR-033.

### ADR-074 — Windows environment-name case and VT re-enabling: parity

**Decision.** 14 P-83 / D-WIN-05: where pre-commit passes an `env=` dict copy (the `no_git_env` family and any 04a/04b
site building `env=`), CPython's copy carries upper-cased names, so snp upper-cases those names on Windows; children that
pre-commit spawns with the inherited environment keep the original spelling — parity per call site, no register entry.
14 P-52: `snp_os::term::ensure_vt()` re-runs `SetConsoleMode` on the stderr console before coloured writes after every
inherited-stdio child (legacy hooks, show-diff, hazmat) and after hook partitions in colour mode; no decision change,
no byte change. Amends ADR-029.

### ADR-075 — gc marking: Tier A repos and relative local repos

**Context.** Tier A fast-path repos have no `repos-v1` entry, so gc could sweep their `keys-v1` markers and a later run
would print `Initializing environment …` where pre-commit (whose clone is still in use) prints nothing; with canonical
local-path keys (D-DATA-08) gc must resolve a tracked config's relative `repo: ../tools` against that config's project,
not gc's cwd. **Decision.** `configs-v1` records `(config realpath, project root)`; gc derives every key with
`RepoId::from_config(repo, recorded_root)`; a `(repo, rev)` loadable from the embedded Tier A profile counts as
"manifest loadable" and keeps its `Initialized` and virtual `Installed` markers; only markers of keys that are neither
present nor Tier-A-loadable are stale. GC acceptance rows added to WP-33.

### ADR-076 — Pre-push and `use_stdin` input cap fails closed

**Decision.** pre-commit reads pre-push stdin without a limit. snp caps it at 64 MiB and, above the cap, fails closed
before the legacy hook and the hooks run: `An error has occurred: FatalError: snp: hook stdin exceeds 64 MiB; refusing
to run hooks on a truncated ref list` (exit 1); it never truncates. Registered as D-SEC-12 (bounded resources).

### ADR-077 — Security hardening, round 2: archives, deletion, store ownership, spill files, logs

**Decision.** (1) Archive extraction runs in two passes (regular files and directories, then symlinks), resolves each
symlink target against the extracted tree and rejects targets that leave it or pass through another symlink, allows
hardlinks only to already-extracted regular files inside the tree, rejects device/FIFO/socket entries, and opens parent
directories without following symlinks (openat2 `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS` on Linux where available).
(2) Checksum manifests always come over TLS from the canonical origin or an explicit https `SNP_NODE_SHASUMS_URL` /
`SNP_GO_JSON_URL`, even when an `http://` mirror is allowed. (3) `remove_tree_within` is fd-relative on Unix
(`openat` `O_NOFOLLOW|O_DIRECTORY`, `unlinkat`, `fchmodat`), closing the CVE-2022-21658 symlink-swap race that ADR-044's
group-writable stores allow. (4) As root (euid 0), a store owned by a non-root uid is refused unless
`SNP_ALLOW_FOREIGN_STORE=1` (git `safe.directory` rationale; D-SEC-13); other foreign owners keep ADR-044's warning.
(5) Output spill files are unnamed (`O_TMPFILE`, create + unlink, `FILE_FLAG_DELETE_ON_CLOSE`): nothing with possible
secrets survives a crash and no spill sweep is needed. (6) `SNP_LOG=trace` environment dumps are allow-list based.
Amends ADR-034, ADR-036 and ADR-044.

### ADR-078 — DiffOracle soundness, round 2, and baseline timing

**Decision.** (1) Stat mode only on an allow-list of filesystems with kernel-maintained ctime and ns mtime (ext2/3/4,
xfs, btrfs, tmpfs, zfs, f2fs, bcachefs; apfs, hfs) — every FUSE (sshfs, rclone, virtiofs), 9p, overlay, vfat, exfat,
ntfs-3g and network filesystem uses Git mode (`snp_os::fsinfo::stat_oracle_safe` replaces `is_network_fs`). (2) Extra
inputs: the default attributes file when `core.attributesFile` is unset and the system `$(prefix)/etc/gitattributes`;
Git mode when `attr.tree` or `GIT_ATTR_SOURCE` is set. (3) The baseline is elided or overlapped only on the warm path
(no clone, install or download since `diff-index`); otherwise it is taken after `install_envs`, pre-commit's position,
so an IDE autosave during a long install is not blamed on the first hook. The 10 000-iteration soundness test gains
these cases. Amends ADR-027.

### ADR-079 — Resource tuning: regex DFA cache, FileTable layout, probe-fact persistence, musl allocator

**Decision.** (1) `dfa_size_limit` (tier 1) and `delegate_dfa_size_limit` (tier 2) use the crate default 2 MiB: it is
the lazy-DFA cache capacity per regex per thread, not an acceptance limit (64 MiB allowed threads × patterns × 64 MiB
RSS); a thrashing cache falls back to linear engines with identical results; `size_limit` 64 MiB stays the acceptance
bound; WP-36 adds a B12 case with 32 threads × 20 patterns on non-ASCII paths. This supersedes the 01 Addendum §A.6
`dfa_size_limit(64<<20)` constants. (2) `FileTable` stores tiers as struct-of-arrays columns allocated on first need,
`Unreadable` boxes its `PyExc`, index entries are an arena (`snp_git::IndexEntries`), and when Stat mode is impossible
fan-out query F is the staged-only `diff --staged --raw -z --no-renames --diff-filter=ACMRTUXB`. (3) Probe facts and the
git version are persisted only for native binaries that are not known proxies (pyenv/asdf/rbenv/nodenv shims are
scripts; mise, rustup and macOS `/usr/bin/*` xcrun shims are listed), keyed by the binary's realpath and stat plus a
digest of the steering environment; everything else is memoised per process. (4) The musl allocator is decided by
WP-36's measurement (system vs mimalloc behind `#[cfg(target_env = "musl")] #[global_allocator]` on B3/B4/B6/B12);
`deny.toml` allows mimalloc only through `wrappers = ["snp"]`. Supersedes Appendix A 07 Q15's unmeasured "off".

### ADR-080 — Hook groups with prek's semantics

**Context.** ADR-039 claimed prek's names and semantics for `groups`, but prek's `--require-group` (AND) and the
`@ungrouped` selector were missing, `--group`'s OR semantics was unstated, and the leading-`@` reservation had no stated
purpose. **Decision.** `--group G` (repeatable, OR), `--require-group G` (repeatable, AND), `--no-group G` (exclusion
wins over both), `@ungrouped` (hooks without groups) usable in all three; group names must not start with `@` (reserved
for selectors); `SNP_SKIP_GROUPS` and the overlay's `skip_groups` exclude; `snp list` takes the same flags. Prefix
stability: pre-commit's `run` has no option starting with `--req`. Amends ADR-039.

### ADR-081 — W0 contract corrections (WP-00 verification)

**Context.** Independent verification of the W0 skeleton found contracts that later packages could not code against
as written. The binary builds `RunCtx` (§1.4.21) once, but `RunCtx.probe: Arc<snp_lang::ToolProbe>` and
`RunCtx.runner: Arc<dyn snp_lang::CmdRunner>` (a `ProcRunner { cancel }` literal) need snp-lang, and §1.2 gave `snp` no
snp-lang edge. `xtask cli-goldens` and `xtask completions` (WP-12) call `snp_argparse::export_json` and match on
`snp_argparse::{Kind, Nargs}`, but the xtask edges were (cli, hooks). Several normative texts disagreed with the
toolchain or the listings: clippy's `exit` lint never fires inside `main`; `debug_assert*` expands to the disallowed
`assert*` macros; §1.3 called `PyRegex` cheap to clone while its listing derives only `Debug`; `HomeLookup` is
`!Send + !Sync` without a note. Ownership left `snp-hooks/src/{hook,registry}.rs`, the multi-package contract tests
and `crates/snp/tests/{api,skeleton}.rs` without a W1–W3 owner and the `user_crates.rs` drift checks under two claims;
shared manifests forced same-wave edits (`crates/snp-hooks/Cargo.toml` for WP-27; `fuzz/Cargo.toml` lacked snp-core,
which most §13.5 targets need). The fuzz jobs set `RUST_MIN_STACK=262144` for the whole job, which rustc honours for its
own main thread (it overflows), while libFuzzer runs inputs on the process main thread, which the variable does not
limit.
**Decision.** (1) New edge `snp → snp-lang`: the binary builds `ToolProbe::new(persist_dir)` and `ProcRunner { cancel }`;
crates/snp's `testing` feature enables `snp-lang/testing` directly. (2) New edge `xtask → snp-argparse`; xtask's edges
are (cli, hooks, argparse) plus the implicit snp-core edge, which applies to xtask as to every crate but snp-pycompat
and snp-probe. `xtask/Cargo.toml` pre-declares `clap` (default features off: `color` pulls the banned `anstyle-query`),
`clap_complete` and `clap_mangen` for `xtask completions`. (3) `PyRegex` is not `Clone`: compiled patterns are shared
as `Arc<PyRegex>` (`RegexCache::get`, `HookFilter`). (4) `snp_pycompat::ospath::HomeLookup` is `!Send + !Sync`
(borrowed `&dyn Fn` lookups for one `expanduser` call), noted in §1.3 next to `SpawnGuard`. (5) `main.rs` marks its
`process::exit` arm `#[expect(clippy::disallowed_methods, reason = …)]`; a non-test `debug_assert*` carries
`#[expect(clippy::disallowed_macros, reason = …)]`. (6) Ownership (WBS): WP-26 owns `snp-hooks/src/{hook,registry}.rs`
and `crates/snp-hooks/Cargo.toml` for both halves (it adds what WP-27 requests; WP-00 pre-declares `memchr` and `bstr`);
WP-33 owns `crates/snp/tests/{api,skeleton}.rs`; the WBS rule "Integration-lead files" lists the files that stay with
the integration lead after W0 (snp-core, the multi-package contract tests, the `user_crates.rs` drift checks,
`crates/snp/tests/it/main.rs`, the fuzz and xtask manifests and infrastructure, `.github/scripts/**`, the catalog).
(7) `fuzz/Cargo.toml` depends on snp-core and snp-os (machete-ignored: shared by many targets). (8) CI: a `perf` cargo
profile (release + thin LTO, 16 codegen units) for the `perf-smoke` job; fuzz targets are built without
`RUST_MIN_STACK` and run by `.github/scripts/fuzz-run.sh` under `ulimit -s 256` (main thread) with
`RUST_MIN_STACK=262144` (spawned threads); every job has `timeout-minutes`; the arm64 Linux and Windows nightly runners
run only while the repository is public; `.github/scripts/deny-fixture.sh` plants `regex`, `which` and `tokio` in
snp-pycompat and expects `cargo deny check bans` to reject each. (9) `docs/research/pycompat-proto` is excluded from the
workspace; `deps-lock.sh link` symlinks the sibling checkouts next to a linked worktree.
**Rationale.** Edges instead of new API: snp already reaches snp-lang through snp-repo and snp-run, and xtask is never
shipped, so neither edge costs build time or layering, while a constructor in snp-run/snp-repo or a re-export in snp-cli
would be new frozen API that a W1/W3 package has to implement and pin. Text follows the toolchain where the toolchain
cannot follow the text.
**Consequences.** `check-dag` enforces the two new edges; ARCHITECTURE §1.2, §1.3, §1.4.2, §1.4.21, §1.4.24 and the
WBS (rules, WP-00, WP-21, WP-22, WP-26, WP-33, crate table) are updated. Amends ADR-070 (item 1: the binary has the
snp-lang edge; doctor still needs no snp-config edge) and ADR-071 (item 7: the xtask edges).

### ADR-082 — End-to-end integration corrections

**Context.** With every work package merged, the tests ignored for a missing sibling ran for the first time. Two
normative texts could not hold as written. (a) The B3 spawn model of ARCHITECTURE §12.1 ("1 combined `rev-parse` + ≤ 6
fan-out queries + 10 hooks ⇒ ≤ 17") leaves out the stash's `write-tree` and `diff-index`, which pre-commit itself runs
(`staged_files_only.py:52-53`, G08/G09 of 02 §G.3; 02 §989 lists them in the warm sequence) and whose rc 0 is what
elides the baseline in the very same sentence: the warm B3 run is 1 + A, B, C, F, G + 2 = 8 git spawns. (b) B6 (13
pre-commit-hooks natives, partition emulation, ≤ 13 spawns with "≤ 4 per-run pre-commit-hooks facts") spawned 46: the
frozen `PchFacts::lfs_files` was one `check-attr` per partition and `check_merge_conflict`'s `rev-parse --git-dir`
went through the never-cached `PchFacts::raw`, so both scaled with the partition count (≈ 20 at 96 files on 22 CPUs).
(c) The first differential run against the oracle showed D-DATA-08 half done: a second project's `repo: ../tools`
got its own clone but no `Initializing` line, because the `Initialized` marker was keyed by the verbatim string.
**Decision.** (1) B3's bound is ≤ 18 spawns (8 git + 10 hooks); B2 stays ≤ 8 (7 git: no F/G below three process hooks,
one `git diff` after its hook). (2) `PchFacts::lfs_files` keeps its signature and result but memoises: the first call
of a run asks for its own paths (a single-partition hook costs one spawn, as upstream); a later call takes one
`check-attr filter -z --stdin` over every index path (`ls-files -z --stage`, itself a cached fact) and answers each
partition whose paths are all index paths while the index, `HEAD`, `<git dir>/info/attributes` and the `.gitattributes`
of every directory of every asked path are unchanged (one `stat` per directory per call); any other path, a changed
attribute source or a failing whole-index call (a non-UTF-8 path, a git error) falls back to the per-partition call,
so results and errors stay upstream's byte for byte. (3) `PchFacts::raw` memoises a successful `rev-parse --git-dir`
(the git dir cannot move during a run; an error is never cached). (4) New `snp_store::marker_repo_name(id, repo,
deps)`: the `Initialized` marker of a local-directory repository is keyed by its canonical path (`abs:<realpath>`), the
same identity as its clone (D-DATA-08), so `repo: ../tools` in a second project prints `[INFO] Initializing
environment for ../tools.` when it clones; resolution, try-repo and gc derive it the same way.
**Rationale.** Spawn counts are not observable output; every byte a hook prints and every exit code is unchanged, and
the fallbacks keep the exact upstream calls wherever the memo could differ. Changing the bound instead of the stash
would mean dropping pre-commit's own `write-tree`/`diff-index`, the data-safety core's parity anchor.
**Consequences.** ARCHITECTURE §12.1, the §1.4.13 comments and §1.4.16 (`marker_repo_name`), WBS WP-36, `bench/README.md` and
`crates/snp/tests/it/budgets.rs` state the corrected bounds; B6 measures 10–11 spawns. Amends ADR-070 (item 10: "B3
stays ≤ 17 spawns").

### ADR-083 — Post-integration verification corrections

**Context.** An independent verification of the integrated `main` found: (a) the B2/B3 wall-clock tests of
`crates/snp/tests/it/budgets.rs` and the 100 MB resident-memory bound of `snp-proc`'s output-budget test ran in the
default nextest gate and failed 2 runs in 6 under load, although ADR-071 puts every threshold test in the release-mode
perf-smoke job; (b) snp forwards a SIGINT sent with kill(2) to the running hooks (ADR-065, ARCHITECTURE §10.4) while
pre-commit 4.6.2 never signals the hook (it keeps running, orphaned, after the restore) — an observable difference with
no register ID (K4), which `x13-eh-04` had hidden by narrowing its `git status`; (c) the orphaned-stash notice of §4.6
could never be dismissed: after the user ran the printed `Restore with:` command the journal still said `stashed`, so
the notice repeated on every run, and the `snp doctor --fix` removal §4.6 promised did not exist; (d) a stashing run
skipped the step-5 recovery scan (it reported inside `snp_stash::enter`, ADR-082's integration fix), so a run ending in
a preflight check before the stash (unmerged files, an unstaged config) never showed the notice; (e) `cargo xtask
difftest` built `-p snp -p snp-probe -p snp-difftest` in one cargo invocation, whose feature unification (the harness's
`insta`/`regex` turn on `syn` features) gave a different `snp` than a plain build; (f) B3's syscall ceiling was still the
provisional soft 4000; (g) `libyaml-safer` was declared in the workspace with a profile override although snp-yaml uses
its in-tree port (ADR-002's fork); (h) the two non-hermetic python rows of `crates/snp-repo/tests/catalog.rs` could not
pass with a real python: the harness answered every setup command with the scripted empty `ShimRunner` reply.
**Decision.** (1) Every wall-clock or resident-memory bound is `#[ignore = "perf"]` and named `perf_*` (the perf-smoke
filter, release build, 3× margin); the deterministic part of such a test (the output budget spills and keeps every byte)
stays in the default gate as its own test. (2) New register ID **D-DATA-18** (class DATA: the orphaned hook can write
into the worktree after the restore) with its rationale and scenario; `x13-eh-04` observes the whole `git status` and
the hook's signal record again under `expect_divergence: D-DATA-18`. (3) Dismissal of an orphan (§4.6): a recovery pass
(lock held) runs `git apply --check --reverse <patch>` for each orphan; when the changes are back, its journal and
pointer are removed with the one-time notice `snp: the unstaged changes saved in <patch> are back in the worktree; the
recovery notice of that interrupted run is retired` (plus the intent-to-add notice when it had cleared entries). Every
other orphan keeps its notice, followed once per pass by `snp: this notice repeats until those changes are back in the
worktree; to dismiss it and keep the patch, run: snp doctor --fix --stash`. `snp doctor --fix --stash` retires every
orphan of the worktree (`snp_stash::dismiss_unrestored`), applied or not, and names each patch. A retirement never
deletes the patch; it resets the patch's modification time, so gc's 30-day retention of unjournaled patches (D-DATA-04)
counts from the retirement. (4) Every non-nested run makes the recovery pass at step 5 (`snp_stash::recovery_report`,
non-blocking lock probe); `StashCtx::report_recovery` is `false` when that pass ran, so `enter` reports only when the
probe found the lock held: exactly one report per run on every path. (5) `cargo xtask difftest` builds `snp` in a cargo
invocation of its own, then `snp-probe` and `snp-difftest`. (6) B3's syscall ceiling is hard: 3842–3845 measured on the
dist build (`strace -f -c` over snp, its 8 git and 10 hook children; Linux 7.0, glibc 2.39) + ~10 % = 4230. (7) The
unused `libyaml-safer` workspace dependency and its profile override are removed (cargo-machete and cargo-deny clean).
(8) The snp-repo test harness can run setup commands with the real `ProcRunner` (`H::real()`), which the non-hermetic
rows use.
**Rationale.** A default gate that fails on machine load trains people to rerun it; the perf job measures the same
bounds where they mean something. The forwarding is the safer behaviour (the terminal ^C already stops the hook under
both tools), so it is registered, not removed. A notice that cannot be silenced is ignored, and then a real one is too;
retiring only on proof (the reverse check) or on an explicit command keeps the "never applied, never lost" rule of §4.6.
**Consequences.** ARCHITECTURE §4.6 and §12.3, COMPATIBILITY §5.2 (D-DATA-18), D-DATA-13/D-DIAG-04 (texts and
scenarios), `bench/budgets.toml`, `bench/README.md`, WBS WP-15 (owned divergence files) and the user troubleshooting page
state the new behaviour. Amends ADR-067 (recovery: dismissal) and ADR-082 (the print-once fix).

### ADR-084 — Final-review corrections

**Context.** The final multi-lens review of `main` found: (a) a kill(2) SIGINT landing between a hook's exec and its
registration reached nobody (`registry::forward` walked the registry without the spawn gate), so the hook got a
silent SIGKILL after the grace instead of the SIGINT of D-DATA-18 — `x13-eh-04` failed under load; (b) `cargo deny
check` failed on advisories (rustls 0.23.45, zip 8.6) and licenses (`Unicode-DFS-2016` of unicode_names2,
`CDLA-Permissive-2.0` of webpki-roots) and carried stale entries; (c) the stash recovery commands (D-DATA-13, the K+
hint of D-DATA-01) overwrote worktree files without a check (`git checkout <tree> -- <paths>`, `git checkout-index -f
-a`), so work done after the kill was lost; the long form never worked (`read-tree` without stat data plus `--3way`:
`does not match index`); `--3way` staged the user's unstaged changes; a temporary `GIT_INDEX_FILE` (`git commit --
<paths>`) had its tree reset into the real index; (d) the ADR-083 retirement check (`git apply --check --reverse`)
accepted a lost mode change (git only warns) and, under `apply.whitespace=fix`, a lost whitespace change; (e) a long
intent-to-add list was named by a journal deleted in the same pass; (f) perf: a thread per `map_ordered` call even at
one job, two 64 KiB reader buffers per spawn, `git config core.fileMode` once per partition, F/G as a serial git stage
after the stash, a second `rev-parse` in hook-impl, 32 regex threads holding a lazy-DFA cache per pattern each, and
`--version` probing the terminal and building a session; (g) UX: an unknown `--group` passed silently, `snp uninstalled`
read as if snp itself was removed, `stage_fixed`'s per-hook line claimed a staging that can still be refused, and
`snp migrate builtin` said "already migrated" for blocks it skipped.
**Decision.** (1) `registry::forward` holds the spawn gate exclusively and `spawn::start` reads its token only under
the gate: a signal either finds the child registered or the child is spawned already-cancelled (killed at once).
(2) rustls 0.23.46, zip 9 (the walker handles fallible names; zip is a `nix` wrapper: its macOS/BSD extraction helpers
are never called); per-crate `[[licenses.exceptions]]` for the two permissive licenses; the stale `MPL-2.0` allowance
and `tracing-subscriber`/`matchers` regex wrappers removed; `cargo deny check` joins the local gate list. (3) Every
recovery command restores with pre-commit's own `git apply --whitespace=nowarn <patch>` (worktree only,
all-or-nothing: it applies around later edits or refuses and changes nothing); with the run's own index it then resets
the `stage_fixed` paths (`git read-tree <tree>` for a long list), re-adds the intent-to-add paths, and says what to do
when the apply refuses: keep a copy (`git diff --binary <tree> > <patch>.now`), `git restore --source=<tree> --worktree`
the patch's paths (`-- :/` for a long or unparseable list), run it again. The K+ hint has the same shape. (4) The
retirement check is `git -c apply.ignoreWhitespace=no apply --check --reverse --whitespace=nowarn <patch>` plus a
comparison of every mode the patch sets with the worktree file. (5) A long intent-to-add list is kept in `<patch>.ita`
(or `<journal>.ita` for a run without a patch), NUL-separated, and re-added with `--pathspec-file-nul
--pathspec-from-file`; it is never deleted with a journal. (6) `map_ordered` runs inline at a limit of 1 and uses the
calling thread as a worker; reader buffers start at 4 KiB and double up to 64 KiB; `PchFacts` memoises `config
core.fileMode` on the stamps of the repository config files; a stashing run with three or more selected process-hook
candidates reads F and G in the step-6 scope (dropped when the stash cleared intent-to-add entries) and then uses the
stat path from one process hook on; hook-impl hands its `(Git, RepoInfo)` to the pipeline; the classifier matches
patterns on at most 8 threads; help and version texts are printed before the session is built. (7) A
`--group`/`--require-group` name no hook carries fails like an unknown id (``No hook in group `G` (known groups:
…)``, exit 1); `{hook_type} uninstalled` is never branded; the `stage_fixed` line reads `- files were modified by this
hook (stage_fixed: staged after the last hook)`; `snp migrate builtin` prints `pre-commit-hooks (line N) not converted:
…` per skipped block, and the unknown-id error of `repo: builtin` points to the list of ids without a builtin instead of
autoupdate. (8) Rejected: the merge-conflict file order is not an unregistered divergence — it is determinism note N-1
(COMPATIBILITY §6.1), compared with `lines-unordered` (`git/t10-git-05-06-conflicted-files` covers several files).
**Deferred.** The stat-based re-verification of the stash patch (B8): it must stamp every path the diff could include
— an autosave of a clean file is what the second `diff-index` catches — with git's racy-timestamp fallback, in the S3
core; it gets its own design. Also deferred: speculating the ITA query and `write-tree` before the worktree lock, one
poll(2) loop instead of two reader threads per spawn, the B4 path copies, and the B8/B11 measurement fixes.
**Consequences.** COMPATIBILITY §3.3, D-DATA-01, D-DATA-13, D-DIAG-04 (texts and scenarios), ARCHITECTURE §4.2, §4.6,
§8, the user configuration and troubleshooting pages state the new behaviour. Amends ADR-065 (forwarding), ADR-067
(recovery commands) and ADR-083 (retirement check).

### ADR-085 — `interactive` hooks (TTY passthrough)

**Context.** `interactive` was reserved for v1.1 (Appendix A, 05 item 11: "v1.1, Unix only"; COMPATIBILITY §7.1) and
`snp migrate lefthook` dropped it as lossy. Real configs need it: a monorepo's `prepare-commit-msg` job `commitzen:
{interactive: true, run: test -n "{2}" || bun x cz --hook}` must show commitizen's prompts and read the keyboard during
`git commit`, although git runs hooks with stdin on `/dev/null`. lefthook (`exec_unix.go`) opens `/dev/tty` when its
stdin is not a terminal (an open failure is logged and the job runs with its normal input), gives the job the terminal's
stdin and its own stdout/stderr, does not `setpgid` it, kills only the direct child on cancellation, and ignores
`interactive` under `no_tty`. Two process-group designs were possible: (a) keep the child in snp's process group; (b)
put it in its own group and hand it the terminal with `tcsetpgrp`, taking it back afterwards.
**Decision.** (1) `interactive: true` is a hook-level bool (also in the local overlay). `true` is refused at validation
for hooks that never spawn a process — meta hooks, `repo: builtin` hooks, `language: fail` / `pygrep` (`interactive:
true needs a hook that runs a process; <kind> hooks run inside snp`); a remote hook whose manifest language is
`fail`/`pygrep` ignores it; G4 of the fast path treats it as a user override, so a pre-commit-hooks hook with it runs
its real process. `SNP_COMPAT_STRICT=1` ignores it like every extension key; both brands behave the same (the texts are
snp's, like `timeout`'s). (2) Terminal: snp's stdin when it is a terminal (inherited), else `/dev/tty` opened read-write
(Windows: `CONIN$`). On Unix it must be usable: `tcgetpgrp(terminal) == getpgrp()`. stdout and stderr are inherited,
never captured. (3) Process group: design (a), lefthook's. The child stays in snp's group, which is the terminal's
foreground group (git, run by the shell as a foreground job, runs hooks in its own group), so reads and `tcsetattr`
(raw-mode prompt libraries) never raise SIGTTIN/SIGTTOU, ^C is delivered once by the kernel to snp and the hook
(`registry::forward` never re-sends a kernel-origin signal to an Inherit child, §10.4), and snp never owns or transfers
the terminal, so it can never leave it owned by a dead group. Design (b) was rejected: between the spawn and `tcsetpgrp`
the child can already read and be stopped (it needs a `SIGCONT` dance); taking the terminal back from a background group
needs SIGTTOU blocked or ignored in snp, which no safe wrapper offers (`unsafe_code = forbid`, no `pre_exec`); and ^C
would reach only the hook, so snp would have to infer the interruption from its exit. The price of (a): `timeout` (TERM,
grace, KILL) and the cancellation escalation signal the hook's own process, not its descendants (lefthook's behaviour
too); the docs advise `exec` in `sh -c` entries. snp saves the terminal modes before the spawn and restores them when
the hook was killed (deadline, cancellation) or died of a signal. (4) Scheduling: an interactive hook is a singleton
bucket — the members of its priority before it in config order form one bucket, the ones after it another — and its
xargs partitions run one at a time. Before it starts, every finished block is printed (with `priority`, also blocks
still waiting for config order), then `{name}{dots}(interactive)` (aligned with the status lines; also in `quiet` mode)
is written and flushed; when it ends its usual status line and details print at once (a line break first when it was
killed, since its prompt was cut off). The block has no output section: hook id, exit code, `- hook timed out after …`,
`fail_text`, the verbose duration. Config order is kept (lefthook 2.2.0 does not reorder interactive jobs either,
despite its docs; 05 "docs vs. code" item 6). (5) No usable terminal (no controlling terminal, `/dev/tty` unopenable, or
snp in a background group): one stderr line `snp: interactive: no terminal is available; running '<id>' without a
terminal` (or `…: snp is not in the terminal's foreground process group; …`) and a normal captured run — clearer than
lefthook's error-and-continue, and it never hangs. `SNP_NO_TTY=1` (lefthook's `no_tty`) runs interactive hooks as normal
captured hooks without a note. No `--no-tty` flag (it would change the snp-brand `run` parser and its goldens for no
gain over the variable). (6) Windows: compiled and supported best-effort: the console is inherited, `CONIN$` replaces
`/dev/tty` when stdin is redirected, no foreground check and no mode restore; the child is in snp's console group, so
`CTRL_C` reaches it once. (7) `snp migrate lefthook` maps `interactive: true` to `interactive: true`; with `--portable`
it stays lossy (dropped, the old note). A positional template inside a double-quoted string is substituted without its
own quotes (`test -n "{2}"` → `test -n "$PRE_COMMIT_COMMIT_MSG_SOURCE"`, was `""$…""`, field-split).
**Consequences.** Registered as D-EXT-08. COMPATIBILITY §7.1, ARCHITECTURE §3.4 and §8.2, the configuration, migration
and environment pages describe it; `snp_proc::tty`, `spawn::interactive` and `Stdin::Terminal` are new API (ARCHITECTURE
§1.4); `snp_testkit::Session` runs commands on a pty that is their controlling terminal (util-linux `setsid --ctty`).
Amends the Appendix A entry "05 item 11" (`interactive` → v1.1), which this ADR implements.

### ADR-086 — First CI run on macOS: exec semantics and portable tests

**Context.** The first CI run built and tested macOS (and Windows, musl and old git) for the first time. On macOS:
(a) a hook whose executable is neither a `#!` script nor a Mach-O binary ran under `/bin/sh`, because std spawns with
Apple's `posix_spawnp`, which emulates `execvp` and retries `ENOEXEC` with the shell; pre-commit (CPython's `fork` +
`execve`, no fallback) raises `OSError: [Errno 8] Exec format error` and the hook fails; (b) a spawn whose cwd is a
regular file failed with `ENOENT` instead of the `ENOTDIR` CPython's child `chdir` reports; (c) `current_exe()` is the
invoked path made absolute (`<repo>/../pbin/snp`), so `install` could embed a `..` path as `SNP_EXE` (11 §A.19.3
forbids it); (d) the lefthook converter emitted EREs with empty branches (`(x.*|y|)` from `{x*,y,}`), which BSD
`grep -E` refuses ("empty (sub)expression"); (e) many tests assumed Linux: the `/var` → `/private/var` and `/home`
→ `/System/Volumes/Data/home` real paths, GNU `sed -i`/`head -c 0`/`pwd` with operands, a case-sensitive file system,
non-UTF-8 file names (APFS refuses them with `EILSEQ`), dash as `/bin/sh`, Linux's 128 KiB per-argument limit, the
xcrun `/usr/bin/python3` shim (it adds `CPATH`/`LIBRARY_PATH`/`MANPATH` to the Python it starts, and ADR-079 never
persists its probe facts).
**Decision.** (1) On macOS the spawn pre-checks a program given as a path: an executable regular file whose first bytes
are neither `#!` nor a Mach-O/universal magic fails with `ENOEXEC` before `posix_spawnp` sees it — the kernel's own
rule, so the result equals Linux's and CPython's. (2) A spawn error attributed to the cwd carries the errno `chdir(cwd)`
gives (`ENOTDIR` for a file, `EACCES` for an inaccessible directory, the stat error otherwise). (3) The `SNP_EXE`
fallback (`current`) is canonicalised when it holds a `..` component; every other candidate stays uncanonicalised. (4)
An ERE alternation drops its empty branches and becomes optional (`(x.*|y)?`; all-empty → nothing); Python-flavour
regexes are unchanged. (5) The tests compare real paths, use portable shell, and skip only what the platform cannot
express, each with a comment: case-colliding fixtures on a case-insensitive file system, non-UTF-8 names where the
file system refuses them, dash's error texts where `/bin/sh` is not dash; B6 accepts the one `python3` probe of the
macOS shim.
**Consequences.** No divergence from pre-commit: (1), (2) and (4) restore parity on macOS, (3) applies 11 §A.19.3.
COMPATIBILITY §6.3 notes (1).

**Amendment to ADR-043 (micro limits on shared runners).** The micro gate's absolute WP-22 limit, classification of
50 k files with 10 filters in ≤ 40 ms, was set on the reference machine (29 ms there); the 4-vCPU GitHub runners
measure 44–52 ms for the same binary, dominated by the 50 k `lstat` calls `Classifier::new` makes (pre-commit's
`os.path.lexists` filter). A `[[micro]]` threshold may carry `reference_only = true`: `cargo xtask bench gate --micro
--reference` (the reference machine) keeps it hard, every other run reports a breach as a warning. The other micro
limits stay hard everywhere, and the release-mode `perf_classify*` smoke test still fails a 3× regression on every
runner (ADR-071).

### ADR-087 — `.snp.yaml`: discovering the config file

**Context.** Every command that reads a config defaults `-c/--config` to `.pre-commit-config.yaml` (`main.py:50-54`,
`C.CONFIG_FILE`); pre-commit accepts no other name (Appendix A, 01 Q6: no `.pre-commit-config.yml` fallback). Repos that
use snp's extension keys (or come from lefthook through `snp migrate lefthook`) want a file named after the tool that
reads them, and lefthook users expect a single short name. The compat brand must keep pre-commit's bytes (K2), the
installed hook scripts embed an explicit `--config=<file>`, the local overlay is named after the config
(`.pre-commit-config.local.y[a]ml`, ADR-070), and gc keeps the repos of every tracked config.
**Decision.** (1) Without `-c/--config` (the facade's `snp_cli::ConfigArg { path, given }`: argparse's `was_given`, so
`-c .pre-commit-config.yaml` is explicit) every command that reads the project's config — `run`, `hook-impl`,
`try-repo` (tracking only), `install`/`uninstall` (hook types and the embedded name), `install-hooks`, `autoupdate`,
`migrate-config`, `list`, `config dump`, `doctor` (also `--fast-path`), `migrate builtin|upstream` — discovers it in the
repository root with `snp_config::discover`: snp brand `.snp.yaml`, `.snp.yml`, `.pre-commit-config.yaml`; compat brand
`.pre-commit-config.yaml` whenever it exists, else `.snp.yaml`, `.snp.yml`. `.pre-commit-config.yaml` keeps pre-commit's
lookup (the invocation cwd when it exists there, else the root); an snp name is always the root's file, rebased like an
existing `-c` path (so it displays as `.snp.yaml`). (2) `.snp.yaml` and `.snp.yml` both present is `An error has
occurred: FatalError: both .snp.yaml and .snp.yml exist; keep one, or choose with -c/--config` (exit 1) — unless the
compat brand already chose `.pre-commit-config.yaml`. (3) snp brand, an snp name and `.pre-commit-config.yaml` both
present: the snp name wins and one stderr line `snp: using .snp.yaml; ignoring .pre-commit-config.yaml (pass
-c/--config to choose)` is printed, once per process (`install --install-hooks` discovers twice). The compat brand is
silent: with `.pre-commit-config.yaml` it is pre-commit. (4) snp brand, nothing found: the error texts stay pre-commit's
(`=====> .pre-commit-config.yaml is not a file`, `No .pre-commit-config.yaml file was found`; the brand map stays a pure
substitution), and the commands that cannot work without a config (`run`, `hook-impl` without `--config`,
`install-hooks`, `list`, `autoupdate`, `migrate-config`, `config dump`, `migrate builtin|upstream`) first print the
stderr line `snp: no .snp.yaml or .pre-commit-config.yaml file was found in the repository root` (class DIAG, so the
snp-brand differential comparison strips it). `install`, `uninstall` and `doctor` work without a config and print
nothing. (5) Explicit `-c` always wins, unchanged; `SNP_COMPAT_STRICT=1` turns discovery off (pre-commit's lookup). (6)
`install` embeds the discovered name relative to the root (`--config=.snp.yaml`, also in `--git-config-hooks`
commands); a re-install after a rename rewrites the script; script identification (`# ID:` hashes, ADR-048) is
unaffected. `init-templatedir` has no repository and keeps `.pre-commit-config.yaml` unless `-c` is given. (7) The local
overlay of `.snp.yaml`/`.snp.yml` is `.snp.local.yaml`, then `.snp.local.yml`; every other config keeps
`.pre-commit-config.local.y[a]ml`. (8) `snp migrate lefthook` writes `.snp.yaml` by default and `.pre-commit-config.yaml`
with `--portable` (pre-commit must find it); `--local` writes the overlay paired with the output name. (9) The
messages naming the config (`git add <config>` of the unstaged check, the `is not a file` error, hook-impl's missing
config texts, autoupdate/migrate-config) name the file actually read; tracking records its realpath. (10) The meta hooks
keep pre-commit's `files: ^\.pre-commit-config\.yaml$` and default argument (the existing `-c` quirk, Appendix A 01
Q8): with `.snp.yaml` they need `files`/`args` overrides. No `.pre-commit-config.yml` fallback (01 Q6 stands).
**Rationale.** The compat brand diverges only where pre-commit would fail (no `.pre-commit-config.yaml`), so a repo
that works with pre-commit gets pre-commit's bytes from the `pre-commit` alias, and a repo with only `.snp.yaml` works
with both entry points. Preferring the snp name in the snp brand makes the newer, snp-specific file authoritative when
both exist (a portable copy for pre-commit users beside it); the note keeps that visible without failing. An error for
two snp names avoids a silent, order-dependent choice. Keeping the missing-config texts and adding a DIAG line keeps
every existing differential scenario byte-identical in both brands.
**Consequences.** Registered as D-EXT-09 (discovery, the note, the error, the overlay names) and D-DIAG-06 (the
missing-config line). `snp_cli::ConfigArg` replaces `OsString` for `-c` in the facade (ARCHITECTURE §1.4.4);
`snp_run::doctor_fast_path` takes it; `snp_config::discover` is new API. The configuration, migration and README pages
and COMPATIBILITY §5.7/§5.8/§7.4 describe it.

### ADR-088 — Release channels: crates.io and Homebrew

**Context.** ADR-049 released snp through cargo-dist (GitHub releases, shell/PowerShell installers) and PyPI
`snp-cli`; the whole workspace was `publish = false`, and ADR-050 chose 2.0.0 because the crates.io name `snp` held
the unrelated 1.x, so `cargo install snp` installed that older tool (installation guide: "not yet available").
The owner holds the crates.io name `snp` (grok-rs, 1.0.1) and wants `cargo install snp` and Homebrew to install
2.x. A crates.io package can only depend on crates.io packages, so `snp` alone cannot be published: its normal
dependency closure has 22 internal crates (all free names on crates.io) plus the user crates cfgv 0.3 and
file-identify 0.5, which ADR-061 already requires to be published from the `deps.lock` commits. Packages must stay
far below crates.io's 10 MB limit (the snp-pyre test vectors alone are ~5 MB).
**Decision.** (1) **crates.io.** `snp` and the 22 crates of its normal (non-dev) dependency closure — snp-argparse,
-cli, -cmd, -config, -core, -files, -git, -hooks, -hooks-data, -lang, -migrate, -net, -os, -proc, -pycompat,
-pyparsers, -pyre, -repo, -run, -stash, -store, -yaml — are published at the workspace version; xtask, snp-testkit,
snp-difftest, snp-probe and fuzz stay `publish = false` (the workspace default; the published crates do not inherit
it). Metadata (license, repository, homepage, README, keywords, categories) is inherited from `[workspace.package]`;
each published package is `include = ["/src/"]` (snp-cli also `/completions/`), so tests, vectors, fixtures and
benches stay in the repository (cargo drops the test and bench targets whose files are not packaged); the largest
package is under 1 MB. (2) **Exact internal requirements.** `[workspace.dependencies]` requires every published
internal crate as `version = "=X.Y.Z"` (the workspace version), not `X.Y.Z` (caret): the internal APIs have no semver
promise, and a caret requirement would let `cargo install snp@2.0.0` (or a later `cargo update`) combine, e.g.,
snp-run 2.0.0 with a snp-core 2.0.1 it was never built or tested with. The price is that every release republishes
every crate, which the shared workspace version already implies. snp-testkit has no `version`: `cargo publish`
strips version-less path dev-dependencies, so the published crates' tests may keep using it, and no published crate
may depend on it (or any `publish = false` crate) otherwise. (3) **Gates.** `cargo xtask release gates` gains gate 6
(`crates`): every published package has the workspace version (hence the tag's), requires the other published
packages with exactly `=X.Y.Z`, has no normal or build dependency on an unpublished package and no versioned
dev-dependency on one, and `snp` is published; an xtask unit test runs it on the workspace in every CI run. The
release `gates` job then runs `cargo publish --workspace --dry-run --locked`, which packages and builds each crate
against crates.io before anything is built. (4) **Publishing.** The release workflow's `publish-crates` job runs
after the GitHub release is created (not for pre-releases, unless dist's `publish-prereleases`, like PyPI), checks
out the tag with the pinned siblings and runs `.github/scripts/publish-crates.sh`: it skips each crate whose version
is already in the crates.io index and publishes the rest with one `cargo publish --locked -p … -p …` (dependency
order, waiting for each crate to reach the index), so a re-run after a partial failure finishes the set — `cargo
publish --workspace` alone fails on the first existing version. crates.io rate-limits new crates (a small burst, then
about one per ten minutes); a first release creates 23 crates, so on HTTP 429 the script waits ten minutes and
retries (up to 30 rounds, job timeout 5.5 h). The token is the `CARGO_REGISTRY_TOKEN` secret (scope publish-new and
publish-update), used in the `release` environment. (5) **Homebrew.** dist's `homebrew` installer (`tap =
"grok-rs/homebrew-tap"`, `formula = "snp"`) builds `snp.rb` as a global artifact (released as an asset);
`build-global-artifacts` checks it (valid Ruby, the release version, one checksum per archive URL, `bin.install
"snp"` only, no binary aliases, no `pre-commit` outside the description). The `publish-homebrew` job (after the
release, never for pre-releases) clones the tap over SSH with the `HOMEBREW_TAP_DEPLOY_KEY` deploy key (github.com's
host keys from the API, never trust-on-first-use), writes `Formula/snp.rb` and commits it as `snp X.Y.Z` with the
github-actions[bot] identity; an unchanged formula is not committed. (6) `cargo binstall snp` uses the release
archives (`[package.metadata.binstall]` of crates/snp). The ADR-049 rule stands for every channel: only `snp` is
installed, never a `pre-commit` alias.
**Rationale.** Publishing the closure is the only way to make `cargo install snp` work while keeping the crate split
of ARCHITECTURE §1.1; exact pins make the 23 crates behave as one versioned unit on crates.io. Skipping published
versions keeps the job safe to re-run, which the new-crate rate limit makes likely on the first release. Committing
dist's own formula (rather than a hand-written one) keeps Homebrew on the same archives, checksums and targets as the
installers. A deploy key limits the workflow's write access to the tap.
**Consequences.** `cargo install --locked snp` builds with Cargo's default release profile (not the workspace's fat
LTO and overflow checks, which apply only to builds of this workspace); the archives remain the recommended channel.
The installation guide, the README and the CHANGELOG describe crates.io and Homebrew; users of snp 1.x get 2.x from
`cargo install snp`. The internal crates appear on crates.io with "no stable API" semantics. The owner adds the
`CARGO_REGISTRY_TOKEN` secret; `HOMEBREW_TAP_DEPLOY_KEY` and the PyPI pending publisher (project `snp-cli`, workflow
`release.yml`, environment `release`) are prerequisites of the respective jobs.

### ADR-089 — Retarget to pre-commit 4.7.0 (snp 2.0.1)

**Context.** snp 2.0.0 reproduces pre-commit 4.6.2 (COMPATIBILITY §1, ADR-050, ADR-054). pre-commit 4.7.0
(2026-10-12) is the current release. Its diff against 4.6.2 (`git diff v4.6.2 v4.7.0`) changes, in the shipped
package, only `pre_commit/languages/node.py` — `install_environment` runs `npm install` with
`lang_base.setup_cmd(prefix, install, env=no_git_env())` (pre-commit#3774, issue #3773: `language: node` hooks failed
or wrote the caller's index during `git commit -a` and in submodules since 4.6.1's `git+file://` install) — plus the
version (`setup.cfg`, `C.VERSION`) and `python_requires = >=3.11` (was `>=3.10`; both oracles are 3.12/3.13).
Everything else is upstream CI, linters, the zipapp builder, the test suite (`testing.util.cwd` →
`contextlib.chdir`, the new `test_node_hook_install_does_not_write_callers_index`) and the CHANGELOG. No resource
(hook templates, empty templates, ruby/rbenv archives) and no message changed. snp already ran every installer with
`no_git_env` (D-SEC-05), so 4.7.0's node behaviour is snp's.
**Decision.** (1) The reference is pre-commit **4.7.0**: `snp_core::profile::PRE_COMMIT = "4.7.0"` (the compat
brand's `--version`, `minimum_pre_commit_version`, the `version … is installed` text and the crash log's `pre-commit
version:` line). (2) The oracles are `target/oracle/pc470-py312` and `pc470-py313`, built by `cargo xtask oracle
install` from `xtask/oracle/requirements-pc470.{in,txt}`; the lock differs from the 4.6.2 one only in the
`pre-commit==4.7.0` pin and its two PyPI hashes (every other pin unchanged). CI workflows, the harness defaults
(`snp-difftest`, `snp-testkit::Oracle::Pc470Py312/Pc470Py313`) and the generators use them. (3) D-SEC-05 is narrowed:
the node installer is parity (4.7.0 strips the variables itself, `node.py:107`; the golang installer always did,
`golang.py:145`); the divergence remains for the pip/uv, cargo and gem installers and for snp-owned repository
operations (autoupdate's temporary repository, hazard c). (4) The release is **2.0.1** (workspace version and the
exact `=2.0.1` internal requirements of ADR-088). (5) Version-bearing goldens and vectors are regenerated from the new
oracles (`cargo xtask cli-goldens`, `gen_config_vectors.py`, `gen_v_pp.py`, `gen_partition.py`,
`gen_legacy_stages_fuzz.py`, `cargo xtask pch-goldens pygrep|hazmat`, `yaml-vectors`, `pycompat-vectors`): apart from
the version strings and provenance headers they are byte-identical. The `minimum_pre_commit_version` boundary cases
move to `4.7.0`/`4.7.0.0`. Data whose generator needs external inputs (the pre-commit-hooks P0 goldens, the tag-manifest
`legacy_stages.tsv`) is not regenerated: the pre-commit modules it exercises are byte-identical. (6) History stays as
written: `docs/research/*` (file:line citations of the 4.6.2 source, still valid for 4.7.0 except `node.py`), the
CHANGELOG, WBS.md and the text of earlier ADRs; where they say 4.6.2 or `pc462` as the target, read 4.7.0 and
`pc470`. Scenario `since:` fields keep the release where a behaviour first appeared. The embedded
`ruby-download-pre-commit-4.6.2` resource keeps its name (4.7.0 vendors the same bytes; renaming would only
invalidate caches).
**Rationale.** A drop-in replacement targets the current release; a one-function upstream change that snp already
matched makes the retarget a version bump plus a narrowed divergence, not a behaviour change. Renaming the oracles
(rather than reusing `pc462-*` for 4.7.0) keeps an old checkout's `target/oracle` from passing silently against the
wrong release: `cargo xtask oracle install` verifies `pre-commit --version` in the venv it creates.
**Consequences.** `pre-commit --version` through the alias prints `pre-commit 4.7.0`; configs with
`minimum_pre_commit_version: 4.7.0` are accepted. Developers rerun `cargo xtask oracle install`. The CHANGELOG's
`[Unreleased]` section records the change; the 2.0.1 tag is cut by the owner.

---

### ADR-090 — What the scheduled gates can judge

*Status: Accepted; amended by ADR-099 (pull requests and pushes gate on the paired rounds of the base measured in the same window; `--advisory-history` on every CI event). The amending ADR is normative where they differ.*

**Context.** The scheduled workflows stayed red on a `main` that had not changed since 2026-11-02, for reasons that
were not defects of snp. (a) The generated difftest failed every night, 8 runs of 2,000 cases on 56c58a4: 1,603
failures out of 16,000, in four classes and nothing else. pygrep's `open()` traceback (about 1,060), unregistered; the
`pygrep.py: error: unrecognized arguments:` list in the compat brand only (about 550), where CPython 3.12.3 stops
filling a `nargs='*'` positional at an unknown option and 3.13.14+, the canonical argparse profile, does not;
`snp: error: unrecognized arguments:` in the snp brand only (about 30), since `snp run` takes several hook ids
(D-EXT-07); and an intent-to-add file paired as a rename (about 20), the registered D-DATA-12. With every night red,
a new difference could not be told from the known ones. (b) `bench` failed on 2026-11-05 and 11-06 with no commit:
B1's "regression" of +21.6 % (glibc) and +36.7 % (musl) compared records from different runner VMs. One binary
measured B1 at 0.757, 0.820, 0.921 and 1.186 ms across VMs, and 0.482 to 0.659 ms on musl; the ratio to prek, measured
in the same run, still moved by 30 %. (c) Same-runner comparison is sound but not exact: over 22 CI runs of two
binaries differing by little or nothing, the difference never exceeded 0.045 ms, yet that was +5.8 % on B1 and failed
the push of 56c58a4, a version bump with no behaviour change (ADR-089).
**Decision.** (1) The pygrep failure is registered as D-MISC-09 (class TIERB). (2) `snp-difftest` reduces, for
scenarios tagged `generated` only: pygrep's `open()` traceback to its uncoloured exception line (D-MISC-09), in streams
and worktree files; the `pygrep.py:` argument list in the compat brand; the `snp:` argument list in the snp brand.
Each masked line stays byte-exact in the other brand. (3) The generator disables rename detection
(`diff.renames=false`) on scenarios combining an intent-to-add file with an unstaged deletion. (4) The regression rule
gains an absolute floor, `[gate] regression_min_ms = 0.1`: a regression must exceed 5 % **and** 0.1 ms. (5) `bench gate
--advisory-history` turns the regression and drift checks against the history into warnings; CI passes it wherever it
has no same-runner baseline (the nightly e2e and musl jobs). It refuses to combine with `--baseline`.
(6) D-MISC-09's scenario, run on Windows, showed `framework::io_exc` rendering `open()` failures in the Win32 form:
it now uses the CRT's `errno` (`repr::oserror_from_open`), which is what CPython's `open()` reports.
**Rationale.** A fuzzing suite is worth what its red means: narrow, named exclusions keep every other byte exact, where
Tier B or fewer generated `args` would have hidden more (that path found the `open('')` bug in 9ebf1f1). Masking one
brand loses nothing, because the other brand checks the same line against the right oracle. Rename detection is the
smallest way out of D-DATA-12: pre-commit crashes there, so nothing past the crash is comparable, and the state stays
in the suite. A 5 % threshold cannot be gated across VMs that differ by 57 %: it detects nothing and fails on nothing
but the hardware, and normalising by prek does not rescue it. 0.1 ms is twice the worst same-runner difference and
only matters below 2 ms; B1's hard 3 ms budget still bounds that scenario.
**Consequences.** The generated suite is green unless something new appears, and `tests/differential/README.md` lists
exactly what it leaves out. A slow creep below 5 % per commit is no longer a nightly failure: it shows as a warning in
the job summary, among noise, until a hard budget or a same-runner comparison catches it. Syscall counts, allocations
and the never-slower rule still gate the nightly. A same-runner drift check (building an older anchor commit in the
nightly) would restore that gate at the cost of a second `dist` build; it is not part of this decision. A developer's
own history stays a gate, since `--advisory-history` is opt-in.

### ADR-091 — Every scenario is measured; accepted misses are listed

*Status: Accepted; amended by ADR-099 (the regression check is the paired rule; B11 compares equal work, its known miss reworded). The amending ADR is normative where they differ.*

**Context.** `bench.yml` measured only B1: the list was to grow when `snp run` (WP-32) merged, and never did, so B2–B12
and their budgets were unchecked on a released product. Measuring them on CI shows real misses (`bench/budgets.toml`).
B2 is seven git processes against a 10 ms budget: 7.9 to 10.7 ms across runner VMs, so it depends on the VM. B8 computes
the stash patch twice to re-verify it (ADR-067, deferred by ADR-084). B11 is far behind prek, though prek reports
nothing to clean and may not do the same work. B4, B5, B6 and B12u are 3 to 26 % behind prek and not profiled. All 12
scenarios agree between two binaries on one runner, so the same-runner regression check holds at this scale.
**Decision.** CI measures every scenario. A miss accepted for now is a `[[known_miss]]` entry (scenario, check, reason):
that check warns, with its reason. Unlisted checks still fail, and the regression check is separate, so a listed
scenario that gets worse still fails. The budgets themselves do not change.
**Rationale.** Unmeasured budgets protect nothing, and red every night hides the next regression, which is how
ADR-090 began. Editing the numbers would hide that they are missed. Closing B2 and B8 reliably touches the stash path,
which protects user data, so that needs a design of its own.
**Consequences.** The bench jobs take longer, and each entry is open work. Meeting ARCHITECTURE §12.1 in full remains
the goal; the list records how far away it is.

---

### ADR-092 — November 2026 upstream refresh: nodeenv 1.12.0, uv 0.13.8, TLS roots

**Context.** A check of every upstream snp reproduces or ships (2026-11-13): pre-commit 4.7.0 is still the latest
release and its `main` has changed only CI and linter pins since; identify 2.6.20, cfgv 3.5.0, PyYAML 6.0.3,
pre-commit-hooks 6.0.0 and pygrep-hooks are the pinned versions (their `main` branches carry only `py311+`
refactors). lefthook 2.2.2 is still the latest tag; its unreleased `master` stops hooks on SIGTERM/SIGHUP (leaving an
inherited `SIG_IGN` alone), gives every `use_stdin` job its own reader, deduplicates files matched by several globs
and moves to doublestar 4.10.4 — snp already behaves that way (`snp-proc` signals, `Arc<[u8]>` stdin fed through a
pipe per process, one alternation regex per glob list, textual brace expansion that keeps the segment position). Two
dependencies pre-commit does not pin moved: nodeenv 1.12.0 (2026-10-31; 1.11.1 skips node pre-releases, which
`index.json` lists first since `v27.0.0-alpha.0.0.0`) and virtualenv 21.15.6. uv 0.13.5 is the first uv that rejects
chains through the compromised eMudhra intermediates (rustls-webpki 0.103.16); snp pinned uv 0.12.23. webpki-roots
1.0.12 follows Mozilla's removal of further roots; the fuzz lock still had rustls-webpki 0.103.15.
**Decision.** (1) The nodeenv reference is **1.12.0**: a `[nodeenv]` value is read with `ConfigParser.get`
(`%%`, `%(name)s`), read as written when interpolation is a syntax error and the value has no `%(` (nodeenv#440), and
loses one pair of matching outer quotes (nodeenv#321); where nodeenv crashes (an unknown or too deep reference) the
written value fails the version check. The oracle lock moves only the `nodeenv==1.12.0` pin. Pre-release skipping was
already snp's; D-SEC-08 now describes 1.12.0 (it still checks for a build only on x86_64-musl and riscv64).
(2) virtualenv stays **21.14.5**: snp creates environments with uv or `venv`, and nothing 21.15.x changed
(activators, app-data handling, the embedded pip) reaches an output snp reproduces; moving the pin would only perturb
the oracle. (3) The embedded uv is **0.13.8** (`cargo xtask tool-checksums --uv 0.13.8`). uv 0.13's breaking changes
miss snp's invocations: interpreters are passed as absolute paths with `--no-python-downloads` (the 3.15 default does
not apply), `uv python install` always gets an explicit request, pip-config constraints are already one
`--constraint` per file, and the omitted distutils patch matches virtualenv >= 21.6. Every flag snp passes is in
0.13.8's `--help`, and a Python environment built through the downloaded pin runs its hook. (4) rustls-webpki
0.103.17 and webpki-roots / webpki-root-certs 1.0.12 in `Cargo.lock` and `fuzz/Cargo.lock`, and rustix 1.1.6
(RUSTSEC-2026-0486, published after the last green `cargo deny`: xattr calls with a zero-length buffer; snp makes
none, but the advisory fails the `deny` gate); no other crate moves here. (5) Not in this change: Rust 1.100.0 (a new clippy lint on `map(..).unwrap_or_default()` and new cargo manifest
lints need their own change), prek 0.5.10 as the bench comparator, and the converter documentation's doublestar
version (it moves to 4.10.4 when a lefthook release ships it).
**Rationale.** pre-commit users installing 4.7.0 today get nodeenv 1.12.0, so that is the behaviour to reproduce; a
uv that trusts a distrusted intermediate weakens the TLS that D-SEC-08 relies on for PyPI traffic.
**Consequences.** Developers rerun `cargo xtask oracle install`. A quoted `node = "20"` in `setup.cfg`, `tox.ini` or
`~/.nodeenvrc` now selects node 20. The first Python install after upgrading downloads uv 0.13.8 into `tools-v1/uv`.

### ADR-093 — The stash checks out only the patch's files (D-DATA-19)

*Status: Accepted; amended by ADR-095 (fix F3 with kernel K7: the checkout sink refuses a list holding an empty or NUL-bearing path, `ValueError` before any call, and the rollback drops empty records of the `git diff` listing). The amending ADR is normative where they differ.*

*Amends ADR-016 (the two checkouts of the algorithm).*

**Context.** ADR-016 keeps pre-commit's stash commands, among them `git -c submodule.recurse=0 checkout -- .` after
the patch is saved and again as the conflict rollback. prek fixed the data loss that whole-tree checkout causes
(j178/prek#2912, prek 16564b3), and it reproduces with pre-commit 4.7.0 and snp 2.0.0: a working-tree change that a
clean filter hides from `git diff` is not in the patch, yet the checkout overwrites the file, because its size differs
from the index entry. The same happens to an edit in an `--assume-unchanged` file (`diff-index` skips it, `checkout
-- .` does not). Both need only one ordinary unstaged change elsewhere to trigger the stash. COMPATIBILITY K4(b)
admits a divergence where pre-commit loses user data.
**Decision.** (1) Stash: the files to check out are the patch's, read by git's own parser (`git apply --numstat -z
<patch>`, which changes nothing); the checkout is `git -c submodule.recurse=0 checkout --pathspec-from-file=-
--pathspec-file-nul` fed `:(literal)<path>\0` per file, in the run environment without the `GIT_*_PATHSPECS` switches,
with `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` as before. A patch with no listed file is refused before any mutation
(`ValueError`, cannot happen with a git-made patch). (2) Conflict rollback: the patch's files followed by every other
file `git diff --name-only -z --no-renames --no-ext-diff --no-color --ignore-submodules --no-relative` lists, each
once, through the same checkout as a restore command (the tty-stop notice and the K+ path unchanged); a failing listing
is the rollback's failure. (3) Hints: a fallback whose file list is too long to spell out names a kept
`<patch>.paths` (NUL-separated, like `<patch>.ita`) instead of `git restore … -- :/`. New register ID **D-DATA-19**.
**Rationale.** The patch is what the restore re-applies, so its file list is exactly what the stash may rewrite:
nothing is lost that the patch does not hold, and the restore still finds every file in its pre-image. `git diff`
after its stat refresh is the visible part of what pre-commit's rollback rewrites, so every hook fix git can see is
still discarded and the index is untouched. Pathspecs on stdin have no argv limit (git ≥ 2.25 reads them; 2.31 is the
floor), and literal magic needs no escaping for `*`, `[`, quotes, newlines or non-UTF-8 names. `git checkout` rather
than `checkout-index` keeps the `post-checkout` hook call pre-commit makes. Narrowing only when hidden changes exist
would keep the old error text for a failing checkout but needs a second listing and an untested path for the case
that matters; one path for every run is simpler to prove.
**Consequences.** Transcripts, patches, index, refs and the stash list are unchanged; only the argv in the
`CalledProcessError` of a failing stash or rollback checkout differs (X13-STASH-01), and the worktree after a run with
hidden changes. A file with both a visible and a hidden change still loses the hidden part (git cannot express it in a
patch), as does a hidden change in a file a hook visibly modifies before a rollback; D-DATA-19 documents both.
`snp_git::Git::checkout_all` is replaced by `checkout_paths`, `patch_paths`, `changed_paths` and `rollback_paths`.

---

### ADR-094 — Rust 1.100: toolchain and MSRV, the never type and the new std and Cargo features

**Context.** Rust 1.100.0 (2026-11-12) stabilizes the never type (`Infallible` is now an alias of `!`),
`str`/`[T]`/`Path::trim_prefix` and `trim_suffix`, `mem::DropGuard`, `bool::toggle`, custom allocators, Cargo's own
lint system (`[lints.cargo]`, with `unused_dependencies`) and dependency cooldowns, and moves test executables to a
new build-dir layout (`<profile>/build/<package>/<hash>/out/`). 1.99 stabilized `String::from_utf8_lossy_owned` and
`fs::set_times`. Clippy 1.100 extends `map_unwrap_or` to `map(f).unwrap_or_default()` and `redundant_clone` to loop
bodies. snp pinned 1.99.0 with MSRV 1.98 (ADR-005), which kept all of this out of reach.
**Decision.** (1) The toolchain is **1.100.0** and the MSRV is **1.100** (`rust-version`, clippy's `msrv`, the CI
`msrv` job): the MSRV equals the pin, so snp may use everything 1.100 stabilized; `cargo install snp` needs Rust 1.100.
(2) Adopted where it states intent better than the old form: `!` for errors that cannot happen (the lenient key
comparator of the JSON sort and the `pysort` tests, where `let Ok(()) = …` proves the absence of an error path instead
of an `unwrap`); `trim_prefix`/`trim_suffix` for every `strip_*(p).unwrap_or(receiver)` (54 sites; identical
semantics: at most one occurrence removed); `DropGuard` for `clean_path_on_failure`'s hand-written guard;
`bool::toggle`; `String::from_utf8_lossy_owned` where the bytes are owned and dead afterwards (17 sites: valid UTF-8
is no longer copied); `fs::set_times` for the recovery notice's patch mtime (no open for writing); `map_or_default`
(80 sites, clippy's suggestion). snp has no other site where `!` changes a type: no `Infallible`, no empty enums, and
the three diverging helpers already return `!`. (3) `[workspace.lints.cargo]` denies `unused_dependencies`,
`unused_workspace_dependencies`, `unused_workspace_package_fields`, `missing_lints_inheritance` and
`redundant_homepage`. Fixing them: `homepage` (equal to `repository`) is gone; `foldhash`, `smallvec`, `rstest` and
`pretty_assertions` leave `[workspace.dependencies]` (no member used them); snp-net's `snp-os` becomes a Windows-only
dependency (its one use is the Windows rename retry; cargo-machete could not see that); snp-lang's `flate2` and `tar`
become dev-dependencies (only its tests build archives). cargo-machete stays: Cargo's lint does not check
dev-dependencies. (4) Test helpers that find the workspace binaries from the test executable (`snp_testkit::Bins` and
snp-proc's probe lookup) walk up to the profile directory, so both layouts work. (5) A unit test ties the crash log's
`rustc` version to `rust-toolchain.toml`. (6) Behaviour changes audited: `ENOTSUP` now maps to
`ErrorKind::Unsupported`, but snp matches errno values (rustix) and never the kind of an OS error. std's case and
class tables move to Unicode 18.0; the few parity paths that use them (argparse's `dest.upper()`, textwrap's word
characters, path, INI and YAML helpers) already differed from CPython's Unicode 15.x only for characters assigned
after 15.1, and the bump adds 18.0's new characters to that set (the profiles of ARCHITECTURE §0.3 that carry their
own UCD data, `repr`, East Asian Width and the regex grammar, are unaffected). `str::escape_debug` is not used for
parity output; the never-type fallback was already `!` under edition 2024. (7) Not adopted: custom allocators (no measured need, and
implementing `Allocator` needs `unsafe`, FD-5); dependency cooldowns (`registry.global-min-publish-age` would also
hold back urgent security updates and `cargo publish`'s verification build, so it needs its own decision);
`Vec::from_fn`, `Box::map`/`Arc::map` (no call site).
**Rationale.** The owner asked for the current toolchain and its idioms; a pin and an MSRV that trail the release by
two versions only keep the codebase from using them.
**Consequences.** Contributors need Rust 1.100 (`rustup` reads `rust-toolchain.toml`). The crash log names rustc
1.100.0.

### ADR-095 — Machine-checked kernels: Verus proofs on the shipped source, Lean 4 models beside it

*Status: Accepted; amended by ADR-096 (decision (8): `proofs verus` is a blocking step of `xtask gates`, release gate 7 exists and the nightly model check and seed sweep run; decision (10): the release-notes acceptance is a `Proof pending: <kernel id>` line). The amending ADR is normative where they differ.*

**Context.** This ADR amends ADR-066 (a `reason = "verus: …"` expectation on a verified function in a mounted kernel
file is a proven site) and ARCHITECTURE §1.3, §11.7, §13.1 and §13.7, and clarifies ADR-056: items gated on
`cfg(verus_keep_ghost)` are not dependencies. Reference: docs/VERIFICATION.md. snp's parity and data-safety guarantees rest on unit, property, fuzz and differential tests, several of
which compare snp with a Rust transliteration of itself (`xargs::partition_matches_the_python_model`). The owner asked
for machine-checked proofs of the critical logic; research and three architecture prototypes followed on 2026-11-13.
Verus 0.2026.11.02.8bf68ee runs on its own rustc 1.99.1 with Z3 4.16.0 and a prebuilt vstd in a 36 MB zip; it proves
byte and index kernels in seconds, and it found a real divergence: snp's saturating `xargs.partition` returned
`Ok([0..1])` for `lens=[usize::MAX], base=1` at `max_len == usize::MAX`, where Python raises `ArgumentTooLongError`.
Ghost code can be written as `#[cfg_attr(verus_keep_ghost, verus_spec(..))]` attributes plus `#[cfg(verus_keep_ghost)]`
imports and proof statements, which rustc's own cfg processing strips in every normal build; a production file can
therefore carry its contract with no dependency, and standalone `verus` verifies that same file through a root that
mounts it with `#[path]` (prototype: 25 items in 1.7 s with `--no-cheating`; stable 1.100.0 clippy with `-D warnings`,
rustdoc and `cargo package` clean; rustfmt leaves the files unchanged). Three findings shape the guards: in a mounted
file Verus silently skips unannotated functions unless the mount carries `#[verus_verify]`; `--no-cheating` rejects
`assume_specification` but not `#[verifier::external]`; `assume_specification` cannot be declared under
`forbid(unsafe_code)`. Verus also refuses `--verify-module` together with `--no-cheating`, so only a whole-root run can
be a gate. The alternatives cost more: vstd as a dependency adds 8 weekly-prerelease crates (a second syn among them)
to the published closure (ADR-088), eats into the §11.7 budget of 120 packages (109 used) and is impossible in
snp-pycompat (ADR-056); a stub-macro facade adds two published crates and an optional vstd in the `all-features` deny
graph; token-mirrored copies need an eraser in the trusted base and a resynchronisation on every edit; Aeneas, Charon
and hax need a pinned nightly rustc, a Lean release candidate and Mathlib; `cargo verus` compiles the whole dependency
graph with rustc 1.99.1, which snp's 1.100 APIs rule out. Lean 4.34.1 with the core library only proved the
end-of-file-fixer and shlex models (7 theorems at research time, 8 with `split_join`; axioms within {propext, Classical.choice, Quot.sound}, clean
`leanchecker`); compiled into oracle executables they agree with CPython and with snp on about 600k inputs. `check-dag`
gives snp-difftest no edge to snp-hooks (`check_dag.rs:162` and the ancestor rule for dev-dependencies), which fixes
where oracle tests can live.
**Decision.** (1) *Scope.* A kernel is a pure function in a production module that the shipped binary calls
directly. Only kernels are claimed proved, and every claim names its spec ("kernel X satisfies spec Y"); "snp is
formally verified" is never written. Lean proves models; the link from a model to the code is a test. (2) *In place,
no dependency.* Kernel files are ordinary production modules whose only ghost forms are
`#[cfg_attr(verus_keep_ghost, X)]` with X one of `verus_spec(..)`, a bare `verus_verify`, `verifier::external_derive`,
`verifier::rlimit`, `verifier::loop_isolation` or `verifier::spinoff_prover`; `#[cfg(verus_keep_ghost)]` on a `use` of
`vstd::prelude::*` or of named `crate::spec` items (no glob, no prelude or std name); and `#[cfg(verus_keep_ghost)]` on
`proof!`/`proof_decl!` statements. They contain no `verus!` block, no other macro call or definition (a macro's
meaning comes from its module scope, which the root does not share), no out-of-line module and no other cfg
(platform behaviour is a `windows: bool` parameter), their exec code reaches only `core`, `alloc`, `std` and the same
file, and no non-private function has a `requires`. No manifest names vstd or any
Verus crate; `Cargo.lock`, `deny.toml`, the published closure and ADR-056 are unchanged; `[workspace.lints.rust]
unexpected_cfgs` gains `cfg(verus_keep_ghost)`. (3) *Proofs tree.* `proofs/` is not a workspace member and sits
outside every package's `include`: `registry.toml` (tool pins, kernels with their production callers and mutants, Lean
models with fingerprinted anchors, trusted items), `verus/kernels.rs` (the root, `#![forbid(unsafe_code)]`, one
`#[verus_verify] #[path]` mount per kernel file), `verus/spec/` (spec functions and lemmas) and `lean/` (a Lake package
on the core library only). Verification is `verus proofs/verus/kernels.rs --crate-type=lib --edition=2024
--no-cheating --output-json`; every function syn finds in a kernel file must appear in the JSON, and every registered
mutant must fail verification. (4) *Toolchains.* The Verus release is pinned by version and per-platform sha256,
together with verusfmt 0.7.4; its rustc is installed beside the snp toolchain and never made the default, and kernel
files use no 1.100-only API such as `trim_prefix`. Lean is pinned in `proofs/lean/lean-toolchain` to a stable release
(v4.34.1 at import). Mathlib, Batteries, release candidates, Aeneas, Charon and hax each need a new ADR; every bump is
a dedicated PR. (5) *Lints.* In a mounted kernel file, `#[expect(clippy::indexing_slicing,
clippy::arithmetic_side_effects, …, reason = "verus: …")]` on a verified function is a proven site under ADR-066.
(6) *Trust.* Kernel files and every file under `proofs/verus/` contain none of `assume`, `admit`, `external`,
`external_body`, `assume_specification` or `exec_allows_no_decreases_clause`, and no raw identifier; spec files hold
spec and proof items only; the root's `forbid(unsafe_code)` excludes
`assume_specification` from kernel files as well. A trusted std specification, if ever needed, lives in a separate
`proofs/verus/trusted/` crate listed in the registry. Fast primitives (memchr, memmem, HashSet) enter verified code
through kernel traits with contracts; each production implementation is listed with a differential test against a
verified reference implementation. (7) *Lean.* `lake build --wfail`, an axiom allowlist of {propext,
Classical.choice, Quot.sound} and `leanchecker`; model files contain no `sorry`, `admit`, `native_decide`,
`implemented_by`, `extern`, `partial def` or `axiom` (assumptions are theorem hypotheses), nor metaprograms, and a
check of the elaborated package rejects compiler attributes that would make an oracle run other code; every registered
theorem's statement is pinned by a fingerprint in the registry; oracle executables call the proved functions, and the
drivers and oracle tests are fingerprinted so that any edit to them is re-reviewed; oracle tests live in the home crate's `tests/` and use only std, so snp-pycompat keeps zero
dev-dependencies, skip without `SNP_LEAN_ORACLES` and fail when skipped under `SNP_REQUIRE_LEAN_ORACLES=1`; each
Lean-only link pins a token fingerprint of the production items it models. (8) *xtask and CI.* `cargo xtask proofs
{check, verus, lean, oracles, mutants, install, status, fingerprint}` is a WP-00 task file with no new dependency.
`proofs check` holds the tool-free guards and runs in the required `xtask gates` job from the start. The `proofs`
workflow (verus; lean with the oracle tests) is advisory until the owner's M1-exit decision; from then on `proofs
verus` is a step of `xtask gates` and release gate 7 (planned) re-verifies the tagged commit. Production calls K1–K3
already, so the decision is due. The Lean job is promoted under (11). A nightly job is to check the Lean models
against CPython, run a Z3 seed sweep and report proofs that fail at half the rlimit (planned; the CPython check is run
by hand until then). (9) *Kernels.* M1 covers the xargs partition core, the archive
entry-name check, end-of-file-fixer (Verus and Lean) and the shlex split model (Lean); M2 and M3 are as
docs/VERIFICATION.md lists them. A kernel extraction is behaviour-preserving, except for the hardening listed here. A
divergence that a proof exposes is either fixed toward pre-commit or registered; the first case: xargs at
`max_len == usize::MAX` now raises `ArgumentTooLongError` (CHANGELOG "Fixed"); the second: fix F3 lands with kernel K7
and amends ADR-093/D-DATA-19. The stash's checkout sink refuses a pathspec list holding an empty or NUL-bearing path
(`ValueError` before any git call, because an empty `:(literal)` selects the whole tree), and the conflict rollback
drops empty records of the `git diff` listing (docs/VERIFICATION.md §9.2 K7, §11 F3). Confirmed defects (gc patch protection, D-DATA-04) are fixed in ordinary
PRs before a proof locks the behaviour in. (10) *Escape hatch.* An urgent change to a kernel whose proof is not ready
may land with the kernel marked `pending` in the registry; it is then unmounted, and its marker line says so. Release
gate 7 (planned, M1 exit) will fail while any kernel is pending, unless the release notes accept it. (11) *Promotion and exit.* The Lean job
becomes required after 4 green weeks on main, with p95 under 5 min and the owner's approval. If upkeep exceeds about
3 engineer-days a month, or two consecutive Verus bumps break more than two proofs, the Verus step returns to
advisory; the Lean models and oracle tests stay.
**Rationale.** Verifying the very file `cargo build` compiles, with rustc's own cfg stripping as the eraser, leaves
the smallest possible gap between what is proved and what ships, and costs users nothing: no crate, no feature, no
new supply-chain surface. Standalone verification takes seconds and needs no vstd from crates.io. Lean covers what
SMT handles poorly (simulation proofs against CPython's state machines, inductive and protocol properties), and its
compiled oracles keep per-PR CI free of Python. Tool-free guards make "only ghost code differs" a mechanical fact
rather than a review convention.
**Consequences.** About a dozen kernel files carry contracts; editing one needs Verus (`cargo xtask proofs install`;
release zips exist for Linux, macOS and Windows; `proofs-tools.sh install` supports x86-linux and macOS and is
sha256-pinned for x86-linux today, and elsewhere a pinned `verus` on `PATH` is used); everyone else is unaffected.
Kernel files trail the newest std API by Verus's rustc lag. Specs and proofs appear in neither rustdoc nor crates.io.
`xtask gates` gains the sub-second guards now and about 45 s warm (about 2 min cold) once the Verus step is made
blocking; the advisory Lean job takes 3–5 min. docs/VERIFICATION.md lists every kernel with its guarantees, its
non-guarantees and the trusted base. ARCHITECTURE §13.1 gains layer L9 (proofs). WP-38 owns `proofs/`; package owners
review their kernel files. Until the Verus step is blocking, a kernel edit that breaks its proof can merge with only
the advisory job red; release notes must not claim a kernel proved for a commit whose `proofs verus` run failed (a
manual check until gate 7 exists). K1's shell builds a length vector before calling the kernel: about 14% slower on
the `partition_50k` micro-bench (about 11 µs per 50,000 files), output unchanged.

### ADR-096 — Blocking proofs: Verus in `xtask gates`, release gate 7, the nightly model check

**Context.** ADR-095 (8) left the Verus step advisory until the owner's M1-exit decision, with release gate 7 and the
nightly CPython check planned. M1's kernels have landed (docs/VERIFICATION.md §9.1) and production calls K1–K3, so a
kernel edit that broke its proof could merge with only the advisory `proofs.yml` job red. On 2026-11-14 the owner
decided: "code should be proved". Measured on the M1 tree: `cargo xtask proofs verus` (40 items, 10 mutants) takes
about 46 s; with `--no-mutants` the root run takes 3–6 s under each of Z3 seeds 1–5 and at `--rlimit 5` (half
Verus's default of 10); `py_vs_lean` agrees on 413,372 shlex and 39,524 end-of-file-fixer cases in about 20 s.
**Decision.** (1) *Blocking Verus.* The required `xtask gates` job of ci.yml runs `cargo xtask proofs install` and
`cargo xtask proofs verus` (V1–V4) after `proofs check`, on every push and pull request (no path filter), caching the
tools directory and `~/.rustup/toolchains/1.99.1-*` under a key of the registry's `[verus]` pins and
`proofs-tools.sh` (a kernel or mutant edit keeps the cache). No required check is renamed or added, so branch
protection is unchanged. The advisory `verus` job of proofs.yml is dropped; the `lean` job stays advisory until
ADR-095 (11) promotes it. (2) *Release gate 7 "proofs".* `cargo xtask release gates` gains `proofs/pending` and
re-verification of the checked-out (tagged) commit: `proofs/check` (G1–G10), `proofs/verus` (V1–V4) and
`proofs/lean` (L1–L5), run in process. release.yml installs Verus (`proofs install`) and Lean (lean-action, pinned by
SHA as in proofs.yml) before the gates, for tags and dry runs alike. `--skip-proofs` skips the three tool runs, never
the pending check; only platform.yml's documentation job, which has no proof tools, passes it. (3) *Accepting a
pending kernel.* The escape hatch of ADR-095 (10) is accepted per release by a line `Proof pending: <kernel id>` in
that version's CHANGELOG section (the GitHub release notes), optionally a list item and followed by any text, e.g.
`- Proof pending: K2-archive-entry (#123): …`; the marker is case-sensitive, the id is matched whole, and lines in
fenced code blocks do not count. Gate 7 fails for a pending kernel without such a line and for a line that names a
verified or unknown kernel. With `--allow-unreleased` the `## [Unreleased]` section counts when the version has no
section. (4) *Nightly.* nightly.yml gains `proofs-models` (lean-action builds the oracle executables; `cargo xtask proofs lean`; `python3 -I
xtask/oracle/py_vs_lean.py --expect-python 3.12 --rounds 10000 --seed $GITHUB_RUN_ID` under CPython 3.12 against
pre-commit-hooks checked out at the v6.0.0 commit gen-pch-data pins) and `proofs-sweep` (`proofs verus --no-mutants
--smt-option smt.random_seed=N` for N = 1…5, a failure fails the job; then `--rlimit 5`, a failure is a warning in
the job summary). `cargo xtask proofs verus` gains `--smt-option K=V` (repeatable) and `--rlimit N`, passed to every
Verus run of the command, and `--no-mutants`. Like every nightly job they report through the run's status and job
summary, with read-only permissions.
**Rationale.** A proof that may silently break is a test that nobody runs: the guarantee the kernel rows of
docs/VERIFICATION.md §9 state holds for `main` and for every release only if a broken proof stops the merge and the
tag. 45 s warm is small against the `test` jobs, and folding the step into an existing required job needs no
branch-protection change. A named line in the release notes keeps the acceptance of an unproved kernel visible to
users, reviewable in the release PR, and checkable by a tool. Seed and rlimit sweeps catch proofs that pass by solver
luck before a Verus bump or an unrelated edit exposes them; a half-rlimit failure is a margin, not a defect, so it
warns.
**Consequences.** Every pull request needs green Verus proofs; a kernel edit whose proof is not ready takes the
escape hatch (`status = "pending"`, unmounted, marker line), and the next release must accept it in its notes or
re-verify it. The pending check also runs per pull request in platform.yml's (non-required) documentation job
against `## [Unreleased]`, so the PR that takes the escape hatch adds its `Proof pending:` line there and the PR that
re-verifies the kernel removes it. Lean stays advisory per PR but blocks at gate 7: a Lean regression that merges
under the advisory job turns the nightly `proofs-models` job red and must be fixed before the next tag. `xtask
gates` takes about 45 s longer warm and about 2 min cold. A release run installs Verus and Lean
(a few minutes cold). The kill criterion of ADR-095 (11) still applies: past it the Verus step returns to advisory
by a new ADR. M1 exit criterion 7 (docs/VERIFICATION.md §12) is met.
### ADR-099 — The bench gate pairs interleaved base and head rounds; the base is built like the head

**Context.** The performance analysis of 2026-11 (P06, P07) measured why same-code pull requests failed the bench
gate. `bench.yml` measured the base commit's snp on every scenario, then the head's: the two windows of one scenario
were about a minute apart, and each window's 95 % interval only covered the scatter inside it. Same-binary repeats
on one machine moved by 6.6 % with disjoint intervals; a simulation of the rule over the CI records gave about 6.7 %
false fails per scenario, about one red job in two over 12 scenarios. Interleaving base and head in rounds removed them
(0 of 22 simulated) while +10 % stayed detected in every case. The base was also built with the head's user crates (the
base's `../cfgv-rs` resolved to the head's checkout), so a `deps.lock` bump (a slower cfgv commit) showed no delta at
all. Separately, B8 left one stash patch per run in each store (about 2.8 GB per job across the three tools), and
prek's B11 store tracked no config (its `install-hooks` installs without tracking; `run --config` tracks), so B11
compared unequal work.
**Decision.** (1) The base commit is checked out at `base/snp`; the head's `deps-lock.sh checkout` with the base's
lock puts its crates at `base/cfgv-rs` and `base/file-identify`; it is built from `base/snp` (its own
`rust-toolchain.toml` and `.cargo`, `--locked`) into the head's `target/bench/base-target`, and the job prints both
locks and toolchains when they differ. (2) `bench setup --base-snp` primes the base binary with its own warm, cold
and gc stores and its own B9 fixture. `bench run --base-snp` measures it with snp in deterministic ABBA rounds per
scenario: 10 rounds of 5 runs for the 50-run scenarios, 10 × 2 for the 20-run ones, 5 × 1 for B7, one warm-up per
binary in its first block, `SNP_BIN`, the own-repository check and `GIT_CEILING_DIRECTORIES` on every block, no RSS
runs for the base. pre-commit and prek follow in the same scenario window, and snp's budgets and the never-slower rule
use its pooled runs from it. (3) The regression rule is paired (a method amendment of ADR-043, ADR-090 and ADR-091;
the null stays "no regression above 5 %"): with `d_r = ln(head_r / base_r)` per round (each the median of its block),
`no paired regression > 5%` fails when `exp(median d) > 1.05`, the effect `base × (exp(median d) − 1)` exceeds
`regression_min_ms`, and `exp` of the lower end of the 95 % interval of the median of `d` (the existing
`median_ci95`) is above 1. The point estimate alone above 5 % warns. A base that fails a scenario (a fail-closed
stash on the shared B8 fixture, for instance) is "not measured", a warning, and the head's checks still run. When the
record carries a paired base, every comparison with another record (an explicit `--baseline`, the history, the
drift check) is advisory; the old unpaired rule stays for those. CI passes `--advisory-history` on every event.
(4) Run records become schema 2: per-round block medians (`rounds_ms`), the counters and heap peak of ADR-100, the
runner's CPU count and git version; schema 1 records are still read, so a mixed history stays usable. (5) B8: before
every run hyperfine's `--prepare` deletes exactly the previous run's patch (snp `<store>/patches/patch[0-9]*-[0-9]*`,
pre-commit `<store>/patch[0-9]*-[0-9]*`, prek `<store>/patches/[0-9]*-[0-9]*.patch`, `find -maxdepth 1 -type f`), and
the harness deletes the same names before each RSS run, after the scenario and at the start of `run`; it refuses when
a `run-*.snp-journal` or a fixture's `.git/snp-stash*.journal` exists (an interrupted run), and asserts that the
directory `find` starts from exists. (6) B11: prek is primed with `prek run --config <absolute config>` per config;
`setup` asserts that every store holds 500 repositories after the warm-up `gc` and that snp and prek track 200
configs. snp's half warns until the fresh-store tracking fix (C1) is in both binaries. B11's known miss is reworded:
prek's `gc` hashes repository keys from the configs and never loads manifests, while pre-commit and snp parse every
used manifest. (7) `ensure_mirror` removes a mirror directory without `HEAD` (an interrupted clone) and clones again.
**Rationale.** Pairing cancels the runner drift that both binaries see in the same minute, which is what made the
unpaired rule fail on unchanged code; a stricter minimum-effect rule (`CI_lo > 1.05`) was rejected because it detects
+10 % only 3 times in 10, and 7 scenarios rely on this check as their only hard gate. A base built with the head's
crates measures nothing a crate bump changes. The patches are deleted by exact name because a store also holds the
journals and pointers that recover a user's work; refusing on an interrupted run keeps the harness from hiding one.
**Consequences.** CI time stays about the same (the same number of runs per binary, in blocks; the base's priming adds
its own stores). A local comparison of two builds is `bench setup` and `bench run` with `--snp A --base-snp B`.
Validation before relying on it: an A/A run on CI (the same binary as base and head, 5 runs × 12 scenarios) must show
no false failure. snp's B11 numbers rise once C1 is fixed (200 configs, 500 repositories); `known_miss` updates wait
for CI evidence (batch M).

### ADR-100 — B8's memory budget is snp's heap peak; RSS names its process; deterministic counters

**Context.** `/usr/bin/time` reports the peak RSS of the largest process the tool waited for, not a sum and not snp's
own. On B8 that process is git computing the diff of the 10 MB file, so the soft budget "B3's RSS + 2 MB" could never
pass and said nothing about the streamed patch (ARCHITECTURE §12.1, T6); ADR-084 deferred the fix. dhat's heap peak
(the bytes live at the global maximum) measures what snp itself holds: 0.48 MiB on B8 against 0.24 MiB on B3, while a
buffered 21.4 MB patch would show 20+ MiB. Timing alone also hides changes that cost little on a fast runner but are
real work: an extra thread per spawn, a lost `statx` cache, a dropped or added git call. strace counts do not depend on
the runner's load: the same binary counts the same, 0.00 % spread on clones, `statx`, `newfstatat` and `getdents64`.
**Decision.** (1) New metrics `heap_peak_mib` (the saturating sum of `gb` over dhat's program points; a profile without
`gb` fails the metric) and `alloc_blocks` (the sum of `tbk`). The nightly `--dhat` run profiles B2, B3, B4, B6, B8, B10
and B12u with one dhat build and removes each `dhat-heap.json`. (2) B8's budget is `heap_peak_mib ≤ 1.0 × B3 + 1.907`
(2 MB), **hard**, nightly; the B8 tree-RSS budget is removed, and its number stays in the record as information for all
three tools. The RSS column and the README name it "peak RSS of the largest process of the tree"; the JSON key
`rss_mib` keeps its name. B4, B6 and B12u keep their hard RSS budgets: tree RSS can only overstate snp, so they fail
closed. (3) Absolute nightly allocation ceilings at the measured value + ~10 % (the B3 convention, ADR-083): bytes on
the single-threaded B2 and B10, blocks on the multi-threaded B4, B6, B8 and B12u (bytes vary by 6.4 % on B12u). They
are soft until the reference runner's nightly records confirm them, since they were measured on a 22-CPU machine and
the thread count follows the CPUs. (4) `--counts`: after the timing runs, one `strace -f --seccomp-bpf -c` pass per
binary and scenario (B7 replaying the cold store's removal, B8 the patch cleanup) counts clones (`clone` + `clone3` +
`fork` + `vfork`, the whole tree), `statx`, `newfstatat`, `openat`, `getdents64` and `execve`, and one `strace -f -e
trace=execve` pass gives the exec argv list. CI installs strace and passes `--counts` on every event. (5) With a paired
base (ADR-099), clones fail above the base's by more than 10 % and at least 5, `statx` and `newfstatat` separately by
more than 2 % and at least 50; opens, dirents and execs are reported only, execs with the multiset difference of the
normalized argv lines (`+` only in the head, `-` only in the base) in the PR comment. Counters are never gated
against the history (runner images change git and glibc); a run that asked for them and has no head counters fails.
The spawn shapes stay gated in `crates/snp/tests/it/budgets.rs`. (6) A per-push guard,
`crates/snp-stash/tests/perf_patch_stream.rs` (`#[ignore]`, `perf_` prefix, Linux, run by the `perf-smoke` job):
stashing and restoring a 24 MB patch may grow the process's `VmHWM` by at most 8 MiB.
**Rationale.** A budget that measures another process cannot fail for the right reason; the heap peak is the
quantity the streamed patch bounds. Block counts are stable where bytes are not. Counters compare base and head on
the same runner, the only comparison that excludes the runner image. Polling snp's own `VmHWM` (`rss_self`) was
rejected: it races the process's exit and would flap on B8 (9.2 to 10.5 MiB against about 10.1).
**Consequences.** B8's permanent soft warning leaves the PR comment; the nightly fails if the patch is ever buffered,
and so does `perf-smoke` on every push. The PR comment shows count deltas next to the timings. The new ceilings are
re-baselined from CI records (batch M), with breaches acknowledged through the budget and `[[known_miss]]` mechanism.

### ADR-098 — The tier-1 exact fallback runs behind the relaxed prefilter

*Amends ADR-003 (01 Addendum §A.6.5, §A.6.11) and the D-SEC-10 rationale.*

**Context.** A tier-1 pattern whose fast text took a cheaper anchor form sends the haystacks that meet a condition
bit (§A.6.5) to the exact text on `fancy-regex`. For a str `\b`/`\B` that is every non-ASCII haystack, and the
exact look-around forms make `fancy-regex` run its backtracking VM instead of delegating to `regex`. B12u's
`\b\w{3,8}\b/\w+\.md$` ran that VM on all 20 000 paths of the run (CPU profile: `fancy_regex::vm::run_with` among
the top self frames), and a non-matching shape such as `^(.+/)*\bconfig\.yaml$` on `é/` × 24 + `x` spent 2.7-4.7 s
per path and ended in `MatchError::Backtrack`, aborting the run with a D-SEC-10 error where CPython answers "not
matched". D-SEC-10 said "tier 1 is linear", which was false for this fallback. Tier 2 already avoids both with the
mandatory relaxed prefilter of §A.6.11.
**Decision.** (1) When a tier-1 pattern has an exact text and its condition has the `ascii`, `nonempty` or `span`
bit, `texts` also lowers `Variant::Relaxed` (no search quirk, which only narrows start positions) and counts its depth
in `Texts.depth`. A `$`-only (`nl`) condition gets no prefilter: its exact text has no word assertion and serves only
haystacks that end with `\n`. (2) One `engine::Prefilter` serves both tiers: the existing usefulness rule
(`relaxed_anchored || !is_match("")`); a build failure or a useless prefilter means *no* prefilter, never an error.
(3) The tier-1 prefilter is built on the first haystack that needs the exact engine (eagerly, without failing, when
the exact fallback itself is built eagerly), wrapped in `\A(?:…)\z` on the fullmatch engines, and rejects before
`re.search`, `re.match`, `re.fullmatch` and `search_bytes` reach `fancy-regex`. (4) Its lazy-DFA cache is capped at
256 KiB per pattern per thread: with the crate default 2 MiB, B12u's process RSS rises 25.5 → 30.8 MiB locally
(median of 30 interleaved runs), above the 28.61 MiB hard budget; with 256 KiB it is 27.1 MiB (max 28.0). Tier 2's
prefilter keeps 2 MiB (ADR-079). (5) The Cond `word` bit with a generated Unicode table (P01 part 3) is rejected: no
gain on top of the prefilter, +1.9 MiB RSS, and it drifts with UCD versions.
**Rationale.** Relaxed ⊇ exact is the invariant tier 2 already relies on: every relaxed substitution only adds
matches, at every span, so it holds for search, for a match at position 0 and inside `\A(?:…)\z`. The verifier's
differential (513 696 compiled rows byte-identical with and without the prefilter, 2.66 M direct superset checks
without a violation) and the new property tests (`relaxed_is_a_superset` against the exact text, now including the
fullmatch form; `tier1_prefilter_keeps_every_answer` across the four match calls) found no difference. Measured on
the B12u fixture: backtracking-engine runs 20 000 → 1 060 per run; snp's own user CPU −19 % (getrusage, user+sys
−11 %); execve, clone3 and statx counts unchanged (430 / 1 807 / 38 116).
**Consequences.** A haystack the prefilter rejects is "not matched", CPython's answer, where it used to be a
backtracking-limit error or seconds of work; haystacks it admits still get the exact engine, so the backtrack limit
and the D-SEC-10 error remain for them. D-SEC-10's rationale now reads "the tier-1 exact fallback is prefiltered"; no
new divergence. The 256 KiB cap re-tunes against CI's B12u RSS once batch C halves the classifier threads (P17).
### ADR-097 — Stash re-verification by stat stamps

*Amends ADR-067 (the second `diff-index` digest before the checkout); decides the stat-based re-verification ADR-084
deferred.*

**Context.** ADR-067 closes the autosave window its fsyncs open with a second full `git diff-index --binary` whose
blake3 digest must equal the streamed patch's. On the B8 scenario (a 10 MB unstaged change) that second diff is the
largest single cost of the run: about 268 ms of the local light-tracer timeline, on the critical path, and blake3 is
32.7% of snp's own CPU. It is the only scenario where snp is slower than pre-commit. ADR-084 deferred a stat-based
re-verification because it must stamp every path the diff could include (an autosave of a clean file is what the
second diff catches) and keep git's racy-timestamp protection. `git apply --numstat`, which lists the checkout's
files (ADR-093), also ran between the verification and the checkout, reopening the window (C4).
**Decision.** (1) The paths are listed (`git apply --numstat -z --whitespace=nowarn <patch>`, git's own parser,
still the only authority for the checkout list) right after the patch is durable and again after every rewrite; the
verification follows and is the last read of the worktree before the checkout. Its outcome wins: a listing error is
reported only once the patch is verified. (2) `M0` is `max(ctime, mtime)` of the held lock file, read with `fstat`
right after this run wrote its token, before the first `diff-index` starts, with the wall and monotonic clocks at
that moment. There is no `M0` for a nested run, on Windows, when the token could not be written, or with
`SNP_STASH_REVERIFY=diff` (the escape hatch, like `SNP_DIFF_MODE=git`). (3) The verification is a stamp pass when all
of these hold, else the ADR-067 loop: Linux (macOS falls back until APFS and HFS+ are shown to bump ctime on every
rename); work tree and git dir on `stat_oracle_safe` filesystems, the work tree on the lock's device; the wall clock
not before `M0` and within 100 ms of the monotonic clock's progress since `M0` (taken before and after the pass);
`git diff-index --name-only -z --ignore-submodules <tree> --` succeeds and lists only files of the patch (option
(a): an autosave of a file outside the patch makes git list it, which falls back, so D-DATA-13's guarantee stands and
git's racy check covers every tracked file; it costs one spawn, an index stat walk); every path the listing names is
a regular file or symlink on the lock's device with `max(ctime, mtime) < M0`, strictly, with no margin (a write after
the lock carries a stamp of at least `M0`, also with coarse or truncated timestamps; a racy margin would disable the
path whenever the previous restore wrote the file just before the run); every directory between such a path and the
work-tree root passes the same test (an older inode swapped in by a rename of an ancestor); `$GIT_DIR/config`,
`config.worktree`, the common config and both `info/attributes` pass it when present. A missing file, `ENOTDIR`, any
error or any doubt falls back. (4) The fallback compares the second `diff-index` byte for byte, and in total length,
with the durable patch read back through a new handle, keeping the 3 rewrites and `KEPT_CHANGING`; blake3 leaves the
stash (the first pass no longer hashes). (5) The stat-stamp path is not modelled on the `DiffOracle` (which decides
whether a hook changed files, with a racy margin and content hashes); it has its own argument above. Option (b),
dropping the name-only query and accepting that a rollback (`rollback_paths`) or `stage_fixed` could then lose a
clean-file autosave made during the window, is rejected.
**Rationale.** A file whose change stamps are all older than the lock has not changed since before the first
`diff-index` started, so the durable patch is still what git would print; the kernel bumps ctime on every write,
truncate, chmod, link change and (on the allowed Linux filesystems) rename, and the user cannot set it. The only
files the patch does not cover are covered by git itself.
**Residual risks.** A write through a shared mapping that was already dirty before `M0` reaches the file without a new
stamp; a single `write(2)` that straddles `M0` can leave a stamp just below `M0` with bytes written after it; an
untracked `.gitattributes` or a global or system git config created or changed during the window is not stamped (it
could change the diff's conversion, which the restore then reports as a conflict; nothing is lost); a filesystem that
does not bump ctime on rename (hence the macOS fallback). Each needs a concurrent writer inside a window of a few
milliseconds to tens of milliseconds.
**Consequences.** B8 runs one content `diff-index` and one name-only `diff-index` instead of two content ones (the exec
count is unchanged), and snp's own CPU drops by the blake3 share. `KEPT_CHANGING`, every message and the git argv of
every other call are unchanged. ARCHITECTURE S3 and §4.2 step 6, D-DATA-13 and WBS WP-23 state it.

## Appendix A — Open questions of the research documents, resolved

Format: question → decision (ADR or divergence). "Parity" = behave as pre-commit 4.6.2.

**01 config schema.** Q1 version compared → `4.6.2`; compat text verbatim; snp brand sentence per ADR-023;
`minimum_snp_version` added. Q2 branding of W8–W10 → ADR-023. Q3 crash paths → C-cfg conversions, exit 1 (D-CRASH-01…04,
07). Q4 piped ordering → reproduce (ADR-028). Q5 meta `language: system` → rejected (parity). Q6 `.pre-commit-config.yml`
fallback → no. Q7 regex dialect → CPython 3.12, `\z` rejected (ADR-003). Q8 meta hooks ignore `-c` → kept. Q9
migrate-config EOL/encoding/alias/block-scalar → fixed (D-DATA-09/14/15). Q10 unknown hook keys at validate time →
parity; snp brand `validate-config --strict`. Q11 rich YAML diagnostics → v1.1, stderr only. Q12 exotic YAML values →
exact via cfgv `Value::{Set, Tuple, Date, DateTime, BigInt, Bytes}`. Q13 identify drift → file-identify tables track
PyPI identify (2.6.20 now), weekly sync job.

**01 Addendum (§A.10).** Q1 opt-in `SNP_PY_RE=3.14` (`\z` acceptance) → no in v1; the grammar stays CPython 3.12 until
pre-commit's oldest supported Python is ≥ 3.14, then a superseding ADR. Q2 crash-to-validation-error → validation
errors (D-CRASH-04, ADR-003; `check_regex_try_with` unused); `PyReError::{Overflow, Value, Recursion}` keep their Python
types, no exit-3 mode planned. Q3 D1 (case-insensitive backreferences) → accepted divergence (D-MISC-06, corpus 0); no
upstream fancy-regex contribution required for v1. Q4 `\N{…}` → feature `py-re-names`, default on (parity outweighs
≈ 1.3 MB). Q5 FutureWarning rendering → `[WARNING] regex …` on stderr (D-MISC-04). Q6 backtrack limit on
`files`/`exclude` → abort the run (D-SEC-10), never "not matched". Q7 RegexSet across hooks → deferred, not in v1:
WP-36 measures B4/B5 and files an ADR if the measured ≈ 5 % is worth the complexity.

**02 run pipeline.** 1 branding → ADR-023. 2 `--version` → ADR-023. 3 argparse texts → byte parity (ADR-010). 4 CPU
count → ADR-017. 5 non-UTF-8 file names → ADR-052. 6 vanished file → excluded (D-CRASH-08). 7 crash log → ADR-013. 8
SHA-256 null OIDs → D-CRASH-10. 9 multi-ref pre-push → parity (first ref with work). 10 pty window size → 0×0 (parity).
11 `cat-file -e` stderr noise → kept. 12 SIGTERM/SIGHUP → ADR-026. 13 git ≥ 2.54 config hooks pass no `--` → keep
`rest[1:]`; every command snp writes ends with ` --`. 14 Unicode for widths → UCD 15.1 (ADR-020). 15 regex → ADR-003.
16 eager installs → parity (eager); fast-path natives need none. G-1 snp-only diagnostics → stderr. G-2 lenient `rest`
→ no. G-3 config-hook writer → `hook.snp-<type>.{command,event}`, absolute SNP_EXE, trailing ` --`, refuses another
manager's event unless `--force` (flag `--git-config-hooks`, ADR-057). G-4 Unicode-whitespace pre-push misparse → parity. G-5 masked git errors → stdout
unchanged, git stderr in the crash log. G-6 `no_git_env` asymmetry → parity allow-list. G-7 stash lock → ADR-026. G-8
object format → exact argv first, one retry on `mismatched algorithms` (D-SEC-06). G-9 partial-clone lazy fetch → parity,
covered by the git matrix. G-10 git 3.0 → release checklist re-runs the 02 G.3 probes.

**03 install/store/commands.** 1 store → ADR-011 (no importer from pre-commit's sqlite). 2 script identity → ADR-048. 3
script fallback → ADR-048. 4 `core.hooksPath` → parity refusal in both brands; `snp doctor` explains; no override in
v1. 5 `.legacy` clobbering → D-DATA-03. 6 product strings → ADR-023. 7 autoupdate failures → per-repo error lines,
others written, exit 1 (D-CRASH-06); missing final newline treated as a line end (D-DATA-16). 8 YAML reformat fidelity →
byte-identical PyYAML dumper. 9 pre-push → ADR-022. 10 hook-impl argv → `rest[1:]` parity. 11 post-rewrite legacy
stdin → parity, empty (ADR-046). 12 lazy store → yes; `clean` always prints (ADR-030). 13 `pre-commit hazmat` entries →
ADR-006. 14 git ≥ 2.54 → `--config-hooks` (snp brand; renamed `--git-config-hooks`, ADR-057); plain install warns when config
hooks exist for the same event.
15 annotated tags under `--freeze` → tag-object sha (parity). 16 crash log → ADR-013. 17 relative local repos →
D-DATA-08. H.11-1 hidden entry name → `__hazmat`. H.11-2 hazmat texts → CPython 3.13 (ADR-020). H.11-3 250 ms SIGINT kill
→ yes. H.11-4 `pre-commit` shim → ADR-049. H.11-5 nested rewrite inside `__hazmat` → no (parity). H.11-6 Windows
OSError texts → Tier B until verified on the Windows oracle. H.11-7 batch-file escaping refusal → D-PLAT-03.

**04a core languages.** Q1 store sharing → no. Q2 interpreter downloads → explicit missing version only,
`SNP_PYTHON_DOWNLOADS=never` (D-CRASH-19). Q3 pyvenv.cfg rewrite → no. Q4 uv config → `--no-config` + pip-config
translation, pip fallback. Q5 uv floor/pin → floor 0.8.0, pinned at release with embedded sha256. Q6 uv cache → isolated
`cache-v1/uv` unless `UV_CACHE_DIR`. Q7 pygrep fallback → none, explicit failure. Q8 pygrep after an unreadable file →
stop that partition (parity). Q9 nodeenv `.node-version` quirk → replicated. Q10 node `default` → latest Current
(parity). Q11 local python placeholder install → skipped without deps (invisible). Q12 docker rebuild → ADR-035. Q13
rust toolchain → persistent verified rustup, `--profile minimal`. Q14 Windows node `system` → allowed when explicit
(D-CRASH-20). Q15 message source → 4.6.2 texts. Q16 python reference interpreter → `python3`, `python` (Windows `py -3`,
`python`), cached by exe fingerprint. Q17 `$HOME` heuristic → replicated; `SNP_PREFER_SYSTEM_TOOLCHAINS=1` opt-in. Q18
Python 3.14 parity → argparse 3.13 texts, regex 3.12 (ADR-020). Q19 golang version supersets (`1.22`) → parity (no
supersets). §11.3 G7 (uv-vs-pip resolver differences) → `SNP_PYTHON_INSTALLER=auto|uv|pip` (ADR-073).

**04b extra languages.** 1 store sharing → no. 2 branding → ADR-023. 3 version strictness → ADR-009. 4 R health stamp →
yes (invisible). 5 `SNP_*` aliases for `PRE_COMMIT_USE_MAMBA` etc. → none. 6 `R_HOME` at run time → parity. 7 lua
version health → parity (no extra check). 8 parallel install cap → `min(N, 4)` distinct prefixes (ADR-024). 9 julia
`language_version` → ignored (parity). 10 strict UTF-8 in probes → lossy (D-CRASH-13). 11 lua/swift on Windows →
attempted as pre-commit does.

**05 lefthook.** 1 key naming → ADR-039 (`x-*` warned). 2 overlay → `.pre-commit-config.local.yaml`, ignored when
tracked or in CI unless named by `SNP_LOCAL_CONFIG`. 3 `env` expansion → literal. 4 implicit priority → flattened
index (prek). 5 `stage_fixed` UX → `Passed` + `- files were modified by this hook and staged`, verify re-run, pre-commit
stage only, `trust` opt-in. 6 non-pre-commit git hooks → not v1. 7 remote `extends` → not v1. 8 CI detection → `CI`
non-empty and not `0`/`false`. 9 `{all_files}` sources → v1.1. 10 parallel output → config-order head-of-line. 11
`interactive` → v1.1, Unix only (implemented by ADR-085, Unix and best-effort Windows). 12 Windows `sh` for migrated entries → Git-for-Windows `sh` lookup (as D-PLAT-02). 13
timeout scope → whole hook. 14 `SNP=0` → ADR-038. 15 LFS → legacy chaining. 16 AI-agent hooks (`ai:`, `snp install
--agent`) → out of v1 scope (COULD, 05 C9); revisit after v1.1. §18.4 A5 item 6 `SNP_STASH_BACKUP` → rejected (ADR-067).
Groups (`--require-group`, `@ungrouped`) → prek semantics (ADR-080).

**06 prek.** 1 workspace → ADR-042. 2 formats → YAML only; prek keys `priority/priorities/groups/env` honoured (groups incl. `--require-group` and
`@ungrouped`, ADR-080); glob dict
form rejected; `repo: builtin` accepted. 3 fast path → default-on `auto` (ADR-031). 4 store sharing → no. 5 pygrep →
fully native. 6 Python envs → uv + pip fallback. 7 claimed version → 4.6.2. 8 exit 2 for internal errors → never (usage
only). 9 shim → ADR-048 (prek scripts recognised). 10 multi-ref pre-push → parity. 11 `literal_separator` → n/a. 12 diff
scope → whole repo. 13 `PREK_*` aliases → no.

**07 Rust ecosystem.** 1 toolchain → ADR-005. 2 store → ADR-011. 3 libyaml-safer → ADR-002. 4 CLI text parity →
ADR-010. 5 `NO_COLOR` → ignored (ADR-029). 6 Windows colour → `winapi-util` (ADR-029). 7 Python `$` → ADR-003. 8 key
coalescing (`1 == 1.0 == True`) → snp-yaml key equality. 9 non-UTF-8 file names → ADR-052. 10 PyPI → ADR-049. 11 tokio
flavour → none (ADR-004). 12 bootstrap scope → ADR-034. 13 EAW version → UCD 15.1. 14 cfgv-rs evolution → bound to 0.3
(FD-3), pinned by `deps.lock` (ADR-061). 15 mimalloc on musl → decided by WP-36's measurement (ADR-079).

**08 user crates.** 1 Windows exec semantics → `ExecPolicy::PythonCompat`. 2 Unix `X_OK` → `PythonCompat` emulation of
`access(2)` (mode/owner/groups; file-identify). 3 regex dialect for `check_regex` → `snp_pyre::cfgv_is_valid`; match-time
limit → error. 4 message wording → verbatim. 5 PyYAML error texts → ADR-002. 6 shebang cap → 64 KiB (D-SEC-09). 7 FIFOs
→ never block (D-CRASH-09). 8 identify tracking → latest PyPI identify. 9 git-mode fast path → yes under ADR-041's
conditions. 10 cfgv numeric equality → handled in snp-yaml keys. 11 `yaml_serde` feature → unused. 12 UCD for
`isprintable` → 15.1.

**09 builtin hooks.** Q1 default `auto`; caveat hooks after their gate; `check-xml` delegated until its gate passes. Q2
emulated CPython → 3.13. Q3 crash paths → parse crashes keep exit 1 + stop; environmental crash on read-only unchanged
files fixed (D-CRASH-23). Q4 partition emulation default, coalesce opt-in. Q5 floor v3.0.0. Q6 Windows CRLF writes of
pretty-format-json/sort-simple-yaml replicated; UTF-8 decoding. Q7 explicit `language:` opt-out → yes. Q8 `str.lower()`
Unicode drift → accepted (UCD 15.1 emulation). Q9 recursion thresholds → CPython 3.13 constants. Q10 AST hooks →
delegate. Q11 ruamel ≥ 0.19 messages. Q12 blobless Tier B fetch → yes. Q13 `COLUMNS` → shared argparse width logic. Q14
builtin names identical to upstream → yes.

**10 test strategy.** Q1 brand by `argv[0]` (ADR-023). Q2 versions → ADR-023/050. Q3 embed pre-commit's hash → yes
(ADR-048). Q4 `pre-commit` entries → hazmat rewrite only; alias opt-in. Q5 store → ADR-011. Q6 regex → ADR-003. Q7 EAW →
UCD 15.1. Q8 git floor → ADR-051 as revised by ADR-063. Q9 crash-log lines → ADR-013. Q10 argparse bytes → yes (own engine). Q11 catastrophic
regex → backtrack limit + linear tier 1, error (D-SEC-10). Q12 language depth → all 21 in v1. Q13 Windows exit codes →
unsigned. Q14 `TRAVIS` → kept (2 jobs). Q15 prek CLI compatibility → out of scope beyond the listed keys.

**11 drop-in migration.** 1 repo URL → `github.com/grok-rs/snp`. 2 prek ID embedded → yes. 3 compat `--version` →
exactly `pre-commit 4.6.2` (ADR-023; the suffix is rejected). 4 `pre-commit` fallback in the script → yes, then fail
closed. 5 PyPI → ADR-049. 6 run hint → TTY-only stderr, `SNP_NO_HINTS`, never from `hook-impl`. 7 bucket names → `-v1`
suffixes; relocation detection. 8 Windows alias staleness → hard link + `snp self alias status`; launcher exe v1.1. 9
`snp clean --pre-commit` → no. 10 `snp uninstall` of prek scripts → skipped (not ours). 11 doctor `--json` → v1.1. 12
config hooks → `--config-hooks` (renamed `--git-config-hooks`, ADR-057). 13 CDPATH repair → yes (D-DATA-07). §A.5.4
install notes M1–M3/M12 → stderr in both brands (ADR-060, D-DIAG-02). §A.19.3 `SNP_EXE` selection → adopted, brand-aware
(ADR-060).

**12 CLI surface.** 1 canonical argparse → CPython ≥ 3.13.14 semantics and texts. 2 prog in the snp brand → `snp`. 3
compat `--version` → `pre-commit 4.6.2`. 4 hazmat with no tool → ADR-006. 5 entry-path prog → `hazmat.py`. 6 3.15
suggestions → ignored. 7 EPIPE → 120 everywhere. 8 snp-only commands in the compat brand → not accepted.

**13 crash catalog.** 1 K+ hint → stderr (ADR-045). 2 EH-03 → 120. 3 valid but over-limit regexes → validation error
plus an snp hint on stderr (§A.6.13 D5/D6, D-MISC-06). 4 CLS-02 → fail closed (exit 3) when the content is needed
(ADR-041). 5 EXE-02 → `ValueError: No closing quotation` as hook output (D-CRASH-05). 6 panic type token →
`PanicException` + crash-log hint naming snp's tracker. 7 CFG-14 gc knock-on → accepted (gc forgets configs it cannot
load, as pre-commit forgets invalid ones).

**14 platform matrix.** 1 batch-file line-break message → ``Executable `{cmd0}` cannot receive an argument containing a
line break: {repr}``, rc 1 (D-PLAT-03). 2 LF vs CRLF → CRLF where CPython uses text mode (ADR-029). 3 `PYTHON_CPU_COUNT`
→ ignored. 4 mintty colours → parity (none). 5 `snp-sys` → rejected (FD-5). 6 WSL CRLF hook script → `snp install`
rewrites ours with LF; `snp doctor` warns. 7 junctions → `symlink` per file-identify (D-PLAT-12). 8 oracle predictions →
`xtask platform-facts` job on real runners (WP-37). P-83 / D-WIN-05 env-name case → parity per call site (ADR-074). P-52
VT re-enabling after inherited-stdio children → yes (ADR-074).

**15 Python shims.** 1 pre-push crash lines → byte-identical. 2 CRLF `rev:` line → kept (D-DATA-10). 3 Windows env dir
with `:` → moot (ADR-009). 4 UCD → 15.1 tables, 3.12 vectors + 3.13 delta. 5 non-ASCII `normcase` → ASCII-exactness
accepted. 6 non-UTF-8 → ADR-052; `SKIP` via `strip_fsbytes`. 7 locale encoding → UTF-8 always (D-DATA-09).

**Judges' cross-cutting corrections** — all adopted: regex constants and grammar (ADR-003), per-subsystem Python
profiles (ADR-020), fd hygiene (ADR-037), poison recovery (ADR-040), global output budget (ADR-036), per-OS budgets
(ADR-043), real user-crate APIs (ARCHITECTURE §1.5), policy-6 audit of every divergence (COMPATIBILITY §5, which lists
only crash/data/security classes plus divergences forced by owner decisions or by the absence of `unsafe`), cold-run
transcript (ADR-032), legacy-safe kill switch (ADR-038), Windows restore isolation (ADR-026), languages split across
packages (WBS), `__hazmat` registered (D-MISC-02).

---

## Appendix B — Design review round 2 (2026-10-08): findings and dispositions

Every finding was verified against the research documents, the user crates' `snp-support` heads (cfgv `6a04a30`,
file-identify `1324f40`) and, where it made a claim about Rust or Python behaviour, by compiling or running a probe
(argparse abbreviation ambiguity and usage wrapping under CPython 3.13; `todo!()` in an RPIT body, `MutexGuard: !Send`
and the unstable `ExitCodeExt::from_raw` under rustc 1.99.0). All were confirmed. "Fixed" = applied as proposed;
"Fixed (variant)" = applied differently, with the reason; "Rejected" = the proposed change was not adopted.

| # | Finding | Disposition |
|---|---|---|
| 1 | `--config-hooks` breaks `--con`/`--conf`/`--confi` | Fixed: `--git-config-hooks`; fixture plants both rejected names (ADR-057) |
| 2 | `brand_map(oracle)` cannot reproduce wrapped argparse blocks; no `--version` row | Fixed: rendered snp-brand argparse texts, prog-patched oracle + JSON-built argparse cross-check, `brand_map` for non-argparse texts, `--version` row (ADR-058) |
| 3 | try-repo store unspecified | Fixed: ephemeral store, tools/cache shared, temp config untracked, two-run acceptance (ADR-059, ARCH §5.7) |
| 4 | M1–M3/M12 and prek/snp-1.x adoption unregistered | Fixed: notes on stderr in both brands (D-DIAG-02), D-BRAND-02, goldens in WP-33 (ADR-060) |
| 5 | `SNP_EXE` selection and hook-time brand undefined | Fixed: 11 §A.19.3/§A.19.4, brand-aware candidate name; compat install embeds the alias path (ADR-060) |
| 6 | Sibling path deps unavailable in CI/release | Fixed: `deps.lock`, sibling checkouts, drift job, release gates; `[patch]` git revs unnecessary (ADR-061) |
| 7, 48 | Catalog rows wrong/unowned/colliding | Fixed: `T10-`/`X13-`/`M11-` namespaces, index file, ownership table, real ranges, catalog-status owned by WP-35 (ADR-062) |
| 8 | Git policy divergences unregistered; EXT misclass | Fixed (variant): the < 2.19 refusal is narrowed to non-empty stashes (D-DATA-17) instead of being merely registered; D-CRASH-26; class DIAG (ADR-063) |
| 9, 71 | Prefetch retry locale/CI dependent; concurrent tty prompts | Fixed: eligible URLs only, BatchMode ssh, every failure a silent cache miss redone in the foreground (ADR-064) |
| 10, 55 | Orphan features (spawn log, panic injection, stash verify, run hint, install-hooks, migrate --install) | Fixed: `snp_proc::spawn_log`, `snp_core::fault`, `verify_restored`, classifier in snp-git, binary sequencing (ADR-066, ADR-070) |
| 11 | No Windows rename retry | Fixed: `rename_with_retry` in every rename primitive (ADR-068) |
| 12, 51 | `dir:hook` rejection breaks ids with `:`; prefix seam missing | Fixed: literal ids first; `HookFilter` prefix + `FileTable::relative_to` (ADR-069) |
| 13 | Open questions 01 Addendum §A.10 and 05 Q16 unresolved | Fixed: Appendix A entries |
| 14 | Groups lack prek semantics; `SNP_STASH_BACKUP` undecided | Fixed: `--require-group`, `@ungrouped`, OR/AND/exclusion (ADR-080); **`SNP_STASH_BACKUP` rejected** — a stash entry is user-visible state pre-commit never creates (K3) and patch + journal + pointer already cover recovery (ADR-067) |
| 15 | No `SNP_PYTHON_INSTALLER` | Fixed (ADR-073) |
| 16 | Windows env-name case and VT re-enable | Fixed (variant): parity per call site instead of a register entry (ADR-074) |
| 17 | gc and Tier A markers / relative local repos | Fixed: recorded project root, Tier-A-loadable keeps markers (ADR-075) |
| 18 | Natives ignore flag-shaped file names | Fixed: exact parity, argparse over `args + files` (ADR-072) |
| 19, 56 | cfgv bindings stale | Fixed: §1.5 re-verified; `check_regex_with` kept deliberately, `check_regex_try_with` unused; lossy display name; API tests (ADR-061) |
| 20 | Pre-push cap behaviour unspecified | Fixed: fail closed, exact text, D-SEC-12 (ADR-076) |
| 21 | RLIMIT_NOFILE ignored | Fixed (variant): **raising the soft limit is rejected** (children inherit it, a visible change for hooks); the pool is clamped to the fd budget instead; low-`ulimit` test (ADR-065) |
| 22 | Cross-reference errors (§6→§7; EXT class) | Fixed |
| 23 | snp-pycompat contract not compilable | Fixed: full production API in ARCH §1.4.2, regex-free, prototype adapted (ADR-056) |
| 24 | Binary cannot build the doctor fast-path check | Fixed: `snp_run::doctor_fast_path`; stash check in snp-cmd; no new edges (ADR-070) |
| 25 | G5 delegation handshake missing | Fixed: `Preflight` seam in `ResolveCtx` (ADR-070) |
| 26 | snp-store needs snp-lang items | Fixed: constant in snp-core, store-owned `.snp-tools.json` (ADR-070) |
| 27 | Nothing implements `ToolDirs`; RunCtx lacks limiters/probe/runner | Fixed: `StoreTools`, `Store::tool_lock`, RunCtx fields, S12 rename (ADR-070) |
| 28 | Registry kills restore-critical git | Fixed: `Role::Critical` never signalled (ADR-065) |
| 29 | Cancellation semantics undefined; FailFast | Fixed: defined; stash has a never-cancelled Critical backend; `FailFast` removed, `Panic` added (ADR-065) |
| 30, 67 | Restore registry without owner; panic hook deadlock | Fixed: registry deleted; hook records + cancels only; restore only on the main thread (ADR-066) |
| 31, 77 | JobPool not re-entrant | Fixed: permit holders run nested work inline; test with `PRE_COMMIT_NO_CONCURRENCY=1` (ADR-070) |
| 32 | W1 packages not leaves | Fixed: extended W0 baselines and composites (ADR-071) |
| 33 | Native framework not a contract; WP-27 deps | Fixed: `snp_hooks::framework` frozen; WP-26 M1; WP-27 depends on WP-13 and WP-26 M1 (ADR-071) |
| 34 | Divergence-file ownership collision | Fixed: W0 stubs, per-ID owners, WP-35 narrowed (ADR-071) |
| 35 | Overlay applied before merge | Fixed: `apply_root` + `HookPatch` in `merge_hook` (ADR-070) |
| 36 | Builtin validation in the wrong crate | Fixed: cfgv items in snp-config (ADR-070) |
| 37 | Key derivation duplicated across W3 packages | Fixed: `RepoId::from_config`, `db_repo_name` in snp-store (ADR-070) |
| 38 | Crate-level stub expectations | Fixed: per-file expectations (ADR-071) |
| 39 | snp-lang `Cargo.toml` unowned in W2 | Fixed: dependencies pre-declared by WP-00 and referenced from stub files (ADR-071) |
| 40 | `todo!()` in RPIT bodies | Fixed: `core::iter::from_fn(\|\| todo!())`; pycompat avoids RPIT entirely (ADR-071) |
| 41 | Missing `Debug` implementations | Fixed: Debug supertraits and impl rule (ADR-070) |
| 42 | `SpawnGuard` is `!Send` | Fixed: documented, excluded from assertions |
| 43 | `ExitCode` cannot carry > 255 | Fixed: `snp::Exit` (ADR-070) |
| 44 | `DiffOracle::new` forces extra spawns | Fixed: takes fan-out `IndexEntries` + origins; query G joins the fan-out (ADR-070) |
| 45 | `GitExe::resolve` vs `discover` ordering | Fixed: sequential (ADR-070) |
| 46 | snp-run lacks the argparse column rule | Fixed: `snp_hooks::columns` re-export (ADR-070) |
| 47 | xtask/dev-dependency edges outside check-dag | Fixed: policy + xtask edges (ADR-071) |
| 49 | Cross-wave acceptance items; criterion thresholds | Fixed: rephrased/[e2e]; `snp-stash-driver`; `perf-smoke` tests + bench gate (ADR-071) |
| 50 | snp-testkit/ToolProbe APIs unspecified | Fixed (ADR-070) |
| 52 | Natives cannot observe cancellation | Fixed: `HookCx::cancel` (ADR-070) |
| 53 | One tag cell per file gives stale partial sets | Fixed: one cell per tier (ADR-070, ADR-079) |
| 54 | `parity_gates.toml` unparseable without build.rs | Fixed: generated Rust table (ADR-071) |
| 57 | Grandchildren survive cancellation | Fixed: Linux subreaper + quiesce, Windows job; macOS residual risk documented in D-DATA-02 (ADR-065) |
| 58 | Reads to EOF block cancellation | Fixed: reader threads, bounded drain (ADR-065) |
| 59 | Signal forwarding gaps | Fixed: origin-aware normative table (ADR-065, ARCH §10.4) |
| 60 | Panic sources outside the lint set | Fixed: lint and API bans (ADR-066) |
| 61 | Patch rename overwrite; fsync window | Fixed: `create_new`, single dir fsync, re-verify before checkout (ADR-067) |
| 62 | Pid-based liveness; store-local journal; hint; stale token | Fixed (variant): lock-based liveness, git-dir pointer, `--3way`, token valid only while held; **recording `boot_id`/pid-namespace inode rejected** as unnecessary (ADR-067) |
| 63 | `stage_fixed` add not journaled | Fixed: `StagingFixed` state + recovery hint (ADR-067) |
| 64 | Restore stopped by SIGTTIN/SIGTTOU | Fixed: WSTOPPED detection, `GIT_TERMINAL_PROMPT=0` (ADR-065) |
| 65 | Windows console close | Fixed: documented journal recovery, `CREATE_NO_WINDOW` for Critical (ADR-065) |
| 66 | pty slave flags; macOS poll on ptys | Fixed: `O_NOCTTY\|O_CLOEXEC`; no poll anywhere (ADR-065) |
| 68 | Guard as last field | Fixed: `impl Drop for RunState` (ADR-066) |
| 69 | Baseline taken before installs | Fixed: warm-path-only elision (ADR-078) |
| 70 | Network-FS deny-list; missing attribute inputs | Fixed: allow-list, extra inputs, `attr.tree`/`GIT_ATTR_SOURCE` → Git mode (ADR-078) |
| 72 | Archive chains, hardlinks, devices, insecure-mirror checksums | Fixed (ADR-077) |
| 73 | Path-based deletion race; root executing foreign store | Fixed: fd-relative deletion, root refusal with `SNP_ALLOW_FOREIGN_STORE` (ADR-077, D-SEC-13) |
| 74 | Named spill files | Fixed: unnamed temp files, spill sweep dropped (ADR-077) |
| 75 | Deny-list secret redaction | Fixed: allow-list (ADR-077) |
| 76 | Pool ignores cgroup quota; parallel compile-heavy installs | Fixed: `min(target, available_parallelism)` unless explicit; exclusive heavy installs (ADR-065) |
| 78 | `dfa_size_limit` 64 MiB per thread | Fixed: crate default 2 MiB, B12 case (ADR-079) |
| 79 | FileTable memory | Fixed: SoA tiers, boxed errors, index arena, staged-only query (ADR-079) |
| 80 | Probe-fact persistence unsound for shims | Fixed: native non-proxy binaries only, env-keyed (ADR-079) |
| 81 | musl allocator unmeasured | Fixed: WP-36 measures; mimalloc allowed only via `wrappers = ["snp"]` (ADR-079) |
