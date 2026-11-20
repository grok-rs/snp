# Changelog

All notable changes to snp are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project uses [Semantic Versioning](https://semver.org/). A release tag `vX.Y.Z` requires a `## [X.Y.Z]`
section (`cargo xtask release gates` checks it, and the GitHub release notes are that section). A kernel whose proof
is pending (docs/VERIFICATION.md §8.3) needs a line `Proof pending: <kernel id>` in that section (gate 7, ADR-096).

## [Unreleased]

Targets pre-commit 4.7.0 (was 4.6.2); the workspace version is 2.0.1 (ADR-089).

### Changed

- bench: the comparator is prek 0.5.11 (was 0.5.10), and the bench's pinned uv is 0.13.7, prek 0.5.11's default, so
  neither prek nor snp downloads one. prek 0.5.11 infers hook effects to skip `git diff` calls, groups read-only hooks
  and caches file contents across read-only built-in hooks, so its times on B6, B8 and B12u are expected to drop.
- bench: the B8 `p50 <= pre-commit` known miss is gone. PR #57's CI bench measured snp at 307 ms against
  pre-commit's 418 ms (1.36x), so the gate fails again if snp ever falls behind pre-commit on B8.
- cfgv and file-identify are part of the workspace (ADR-116): imported with their full history as `crates/snp-cfgv`
  (Python cfgv 3.5.0: byte-for-byte messages, PyYAML value model, `BigInt` with CPython's 4300-digit limit) and
  `crates/snp-identify` (Python identify 2.6.20: 323 tags as a `Copy` bitset, `env -S` / nix-shell shebangs, one
  `lstat` and at most one bounded read per path), published as `snp-cfgv` and `snp-identify` with every snp release.
  A fresh clone builds without sibling checkouts; `deps.lock`, the drift workflow and the user-crate release gates are
  gone. Their data are regenerated from the pc470 oracle (`cargo xtask identify-tables`, `identify-vectors`,
  `cfgv-vectors`), and their fuzz targets run with snp's. No behaviour changes; cfgv's unused `regex` feature and
  file-identify's command-line tool are not carried over.

- The compat brand is pre-commit 4.7.0: `pre-commit --version` prints `pre-commit 4.7.0`, `minimum_pre_commit_version`
  compares against 4.7.0 (a config requiring `4.7.0` is now accepted), and the `version … is installed` message and
  the crash log's `pre-commit version:` line name 4.7.0. pre-commit 4.7.0 differs from 4.6.2 only in
  `language: node` installs, which now run `npm install` without the `GIT_*` variables git exports to hooks
  (pre-commit#3774); snp already did, so no runtime behaviour changes.
- Divergence D-SEC-05 is narrowed to the pip/uv, cargo and gem installers and snp-owned repositories: the node
  installer is now parity with pre-commit.
- The differential-test oracles are `target/oracle/pc470-py312` and `pc470-py313` (`cargo xtask oracle install`, lock
  `xtask/oracle/requirements-pc470.txt`); rerun `cargo xtask oracle install` after updating.
- Windows: pygrep and the native hooks report a file they cannot open as Python's `open()` does
  (`[Errno 2] No such file or directory: 'x'`), not as a Win32 error (`[WinError 2] The system cannot find the file
  specified: 'x'`).
- Divergence D-MISC-09 is registered: a file pygrep cannot open prints the final line of Python's traceback, as snp
  has always done. No behaviour changes (ADR-090).
- `language: node` follows nodeenv 1.12.0 (was 1.11.0) for the `[nodeenv] node=` value of `setup.cfg`, `tox.ini` and
  `~/.nodeenvrc`: one pair of outer quotes is dropped (`node = "20"` selects node 20) and `%%` / `%(name)s` are
  interpolated as `configparser` does (ADR-092). The oracle lock pins `nodeenv==1.12.0`.

- Built with Rust 1.100.0; the minimum supported Rust version is 1.100 (was 1.98), so `cargo install snp` needs Rust
  1.100 or newer (ADR-094). The crash log's `sys.version` names rustc 1.100.0.
- `snp gc` does less I/O: one `repos-v1` snapshot under the exclusive lock instead of a marker read per manifest
  lookup, the markers of used repositories kept without being read, orphan repository locks decided from one listing,
  and the tool sweep reads no environment sidecar when no tool version is old enough. Output and the removal set are
  unchanged.
- Faster stash of large unstaged changes on Linux: when the stat stamps of the patch's files prove they did not change
  since the run took the worktree lock, and `git diff-index --name-only` shows no other changed file, the stash no
  longer runs a second full `git diff-index --binary` to re-verify the patch (ADR-097). snp's own CPU in the 10 MB
  stash scenario roughly halves. Anything unsure (another filesystem, a file touched during the run, a clock step,
  macOS, Windows) runs the full re-verification as before, which now compares bytes instead of blake3 digests.
  `SNP_STASH_REVERIFY=diff` always runs it.
- File classification uses less CPU (ADR-101): the file list's UTF-8 is checked once instead of per name and hook;
  on Linux each file's `lstat` and content probe are relative to one descriptor of the repository root (`statx`,
  `openat`) instead of a freshly built path (about 100,000 fewer allocations for 50,000 files); the classifier
  matches patterns on no more threads than CPUs (`SNP_JOBS=32` on a 4-CPU runner uses 4), a hook runs no more
  helper threads than the pool has permits, and a hook whose files are already classified does it on the calling
  thread. Output, argv and errors are unchanged. A root plus file name longer than `PATH_MAX` is now found, as
  pre-commit finds it through its relative path.
- The per-run git queries of the pre-commit-hooks natives (`ls-files`, `diff --staged --diff-filter=A`,
  `rev-parse --git-dir`, `config core.fileMode`, `symbolic-ref HEAD`) are computed on one background thread while the
  earlier in-process hooks run, instead of on the critical path; nothing runs beside a user process hook (ADR-102).
- check-case-conflict builds its repository index once per run instead of once per partition, and lowercases ASCII
  names without the Unicode tables (ADR-102).
- detect-private-key, trailing-whitespace and mixed-line-ending use less CPU per file. detect-private-key looks
  for `PRIVATE KEY` first and, in a file without it (nearly every file), only for the two patterns that lack it,
  with searchers built once per process instead of once per file and pattern. trailing-whitespace copies and
  allocates nothing until a line changes. mixed-line-ending skips its per-line count in a file without a carriage
  return. Output, fixes and exit codes are unchanged. snp's own instruction count (callgrind) falls by 5% on the
  B6 benchmark (13 pre-commit-hooks over 960 files) and by about 8% on a 372-file source tree.
- check-xml and check-yaml use less CPU: check-xml looks each byte's expat type up in a table instead of computing
  it, and check-yaml tests ruamel's character sets with `matches!` instead of string searches and no longer
  allocates for YAML versions, resolved tags, prefix checks and peeked tokens, or hashes node ids. Output, messages,
  marks and the emulated recursion limit are unchanged.
- Hook argv lists are borrowed from the run's file table and appended at spawn time: `run --all-files` on 50,000
  files makes about half as many allocations and its heap peaks about 7 MB lower (ADR-103).
- Loading a config or manifest does less work: hook and root keys are read without building a key value per lookup,
  and a hook's extension keys are parsed in one pass over it. `validate-config` of 1,000 local hooks makes about
  37,000 (16%) fewer allocations and runs about 7% fewer instructions; `gc` over 200 configs about 9% fewer. Output
  is unchanged.
- cfgv probes each key once while validating and applying defaults (a default is hashed once and cloned only when
  missing; a required key is read once), and the remaining config lookups that built a key value now use the new
  `cfgv::StrKey`. `validate-config` of 1,000 local hooks runs about 3% fewer instructions, `gc` over 200 configs about
  3.5% fewer. Output and messages are unchanged.
- The installed hook script (template v2, ID `405bd8559925d194c1d22af6ef66282e`) finds its directory by parameter
  expansion instead of running `dirname`: one process less per hook run, with the same `--hook-dir` (ADR-105). It
  keeps the old template's ID as a `# snp 2.0 compat ID:` line, so snp 2.0.0–2.0.1 still recognise it. Scripts
  installed by those versions keep working; `snp doctor` reports them as "an older snp hook script" until `snp
  install` (or `snp doctor --fix`) rewrites them. A script with snp's header, an ID this version does not know and
  pre-commit's compat ID is treated as an snp script, never as pre-commit's.
- Restore-critical git commands (the stash's queries, checkouts and applies) get `GIT_TERMINAL_PROMPT=0` set on the
  command instead of on a copy of the whole environment: two fewer allocations per environment variable and command
  (about 220 fewer in a no-op `run`, about 500 in the 10 MB stash scenario, and about 1% fewer instructions in both).
  The child's environment and the `SNP_LOG=trace` line are unchanged.
- Wave-1 tuning follow-up to the tier-1 prefilter (ADR-098; no output change): the relaxed prefilter in front of the
  exact fallback of a `\b`/`\B` pattern now runs on a forward-only `regex-automata` lazy DFA within the same
  256 KiB cache. `regex`'s engine also needs a reverse DFA there, and when the pair did not fit (as for
  `\b\w{3,8}\b/\w+\.md$`) it ran the bounded backtracker on every non-ASCII path. On the B12u benchmark snp executes
  55 % fewer instructions (callgrind 1,423 M → 647 M) and its heap peaks about 2 MB lower; the other scenarios are
  unchanged. `regex-automata` 0.4.18, already built as part of `regex`, is now a direct dependency of snp-pyre
  (ADR-108).
- `files`/`exclude` patterns with Python's `\w` or `[^\W\d_]` (and other str path patterns with a Unicode class of
  128 ranges or more) compile to the forward-only engine of ADR-108 instead of `regex`, which also built a reverse
  automaton, capture states and a one-pass DFA these yes/no questions never use; tier 2's prefilter uses the same
  engine. Every match decision and error message is unchanged; pygrep and bytes patterns keep `regex`. On the B12u
  benchmark snp executes 15 % fewer instructions (callgrind 652.6 M → 555.8 M) and its heap peaks about 2.5 MB lower;
  the other scenarios are unchanged (ADR-115).
- Loading YAML (every config and manifest load, `validate-config`, `gc`) uses less CPU and fewer allocations. The
  reader checks each 16 KiB chunk once and reads the current character without a function call. The constructor
  sizes its tables up front. The loaded value takes the parsed values of nodes that occur once instead of copying
  them (ADR-112). `validate-config` on 1,000 hooks runs about 12% fewer instructions with about 29,000 fewer heap
  allocations; `gc` on 500 configs runs about 5% fewer. Loaded values, errors and their order are unchanged.
- `run --all-files` on a clean worktree no longer `lstat`s every file (ADR-106). When `git diff` is empty and clean
  (exit 0, nothing on stderr), no index entry is assume-unchanged, skip-worktree, fsmonitor-valid or unmerged, and
  git's configuration lets it compare file kinds (`core.fileMode` and `core.symlinks` not false; no fsmonitor,
  `core.ignoreStat` or sparse checkout, and no `GIT_TEST_FSMONITOR`), the file kinds come from the index:
  `git ls-files -s -v -f -z` replaces `git ls-files -z --deduplicate`, one `git config` query is added, and the
  before-diff is taken before the files are classified. On 50,000 files snp's own `statx` calls drop from about
  50,000 to under 100 and its own CPU time from about 180 ms to 70–80 ms. The executable bit is never taken from the
  index: a run with a filter on `executable` or `non-executable` keeps the `lstat` pass, as do empty files, gitlinks,
  a file without name tags when a filter needs shebang tags, and every file first classified after a hook that may
  have changed the worktree. A tree with changes keeps the `lstat` pass and pays for the longer listing and the
  `git config` query in git (about 3% more CPU in the process tree on 50,000 files). Output, exit status and hook
  argv are unchanged.
- Hooks whose filters name only file kinds (`types: [file]`, the default, and `directory`, `symlink`, `socket`,
  `executable`, `non-executable`) are classified from each file's kind and executable bit alone, without computing
  name tags (ADR-110). `run --all-files` on 50,000 files with the default filter runs about 27% fewer instructions
  in snp and its heap peaks about 3 MB lower. A file's later name-tag or content classification reuses that first
  observation, as before. A clean `run --all-files` that also has a hook filtering on an interpreter tag (`python`,
  `shell`, …) classifies as it did before this change: there a file whose name gives no tags must be classified in
  full at its first hook. Output, argv and errors are unchanged.
- The diff oracle's Stat mode takes one `lstat` per input of `git diff` (the index, `HEAD`, every config and
  attributes file) instead of an `lstat` and a `stat`; only an input that is a symlink is also `stat`ed through to its
  target. A run of 10 hooks on a clean tree makes 143 fewer `statx` calls in snp's process (311 instead of 454: 13
  inputs, 11 snapshots and comparisons). What is compared, and so every result, is unchanged.
- File-name tags scan a base name once: the first dot is found eight bytes at a time, the last dot by a backward
  search that stops there, and the whole name and its first dot-separated part share one hash pass instead of being
  hashed separately (`crates/snp-identify`). `run --all-files` on 50,000 files with a `types` filter runs about 3%
  fewer instructions (name tags: about 14% fewer). The lookup tables and every tag set are unchanged.

- Benchmarks (developer tooling, no runtime change): pull requests and pushes measure the base commit's snp, built
  against its own `deps.lock` crates and toolchain and primed into its own stores, in interleaved ABBA rounds with the
  head, and the regression gate pairs the rounds (`cargo xtask bench setup|run --base-snp`, ADR-099). Records are
  schema 2 (schema 1 is still read). `--counts` gates clones, `statx` and `newfstatat` base against head and shows the
  exec argv diff; B8's memory budget is snp's dhat heap peak (≤ B3's + 2 MB, hard, nightly) instead of the tree RSS,
  which is git's; the RSS column names the largest process of the tree; new soft nightly allocation ceilings; a
  `perf_patch_stream` perf-smoke test bounds the stash's resident growth on a 24 MB patch; B8's stash patches no longer
  pile up in the stores; prek's B11 store tracks its 200 configs (ADR-100).
- Benchmarks: B8's unstaged rewrite also changes the file's size, as real rewrites do. With an equal size, the run
  after one whose checkout and restore fell in the same second found the file's stat equal to its index entry (a
  default git build compares stat times to the whole second, and the restored file got the checked-out file's inode
  number back), so about seven git calls hashed the 10 MiB file again: +80 % instructions for the whole tree, in a
  timing-dependent share of every tool's runs. B8 times are not comparable with earlier records.
- A stash of unstaged files does less on the critical path (ADR-117; same git argv per call, patch bytes, messages and
  journal states, and every `fsync` still runs before the checkout). The stash's own checkout rewrites the index, which used to drop the oracle's early
  index read, so a run with one or two hooks fell back to a real `git diff` after each hook; on a small tree the
  index is now read again instead (and the config file list kept unless the worktree lock was contended), so the
  stat oracle decides without that diff. `git apply --numstat` lists the patch's files while the patch is being made
  durable, and a patch larger than 4 MiB is written back to disk (Linux) while git still prints it, so the final
  `fsync` has only the tail left. On B8 (a 10 MB unstaged rewrite) the whole tree runs 10% fewer instructions
  (2,960 M → 2,655 M) and the numstat and most of the patch's disk writes leave the critical path.
- Benchmarks (CI only, no runtime change): the musl bench job also runs on pull requests that change the allocator
  selection (`crates/snp/Cargo.toml`, `crates/snp/src/main.rs`), the workspace `Cargo.toml`, `Cargo.lock`, the bench
  gate, budgets or allocator decision (`xtask/src/tasks/bench.rs`, `bench/budgets.toml`, `bench/musl-allocator.md`),
  `.github/workflows/bench.yml`, or the source of the crates that hold the musl scenarios' memory (`snp-files`,
  `snp-identify`, `snp-git`, `snp-pyre`, `snp-run`, `snp-proc`, `snp-hooks`); the `MUSL_PATHS` regex of `bench.yml`
  is the list. It gates both allocator records as the nightly does, without appending them to the history. Before,
  it ran only nightly and on dispatch, so a pull request could merge over a musl hard RSS budget its own dispatch
  run had failed. The nightly and dispatch runs are unchanged.
- After a stash of unstaged files, the baseline `git diff` before the first hook is skipped when snp can prove it
  empty (ADR-119; output and side effects unchanged, `SNP_VERIFY_BASELINE=1` cross-checks it): on Linux, on the warm
  path, in stat-oracle mode, with no attributes file (in the worktree or tracked) and no `core.attributesFile` or
  `attr.tree` setting, no `core.autocrlf`, fsmonitor, `core.ignoreStat`, sparse checkout or configuration-based hook,
  no `post-checkout` or `post-index-change` hook, no intent-to-add entry, and every checked-out file a regular file of
  its own with its index mode's executable bit.
  Anything else takes the real diff, as before. On B8 the git process that re-hashed the 10 MiB file no longer runs:
  2,653 M → 2,292 M instructions for the tree (prek: 2,467 M), 16 → 15 processes.

### Fixed

- The nightly musl bench passes again (ADR-118). The `musl-mimalloc` build exceeded the hard peak-RSS budgets on
  B4/B6/B12u (53.7/29.7/58.8 MiB), because mimalloc madvised its arenas into transparent huge pages. mimalloc is now
  built with `no_thp` (19.6/10.6/21.8 MiB on four CPUs). The released musl binaries use the system allocator and are
  unchanged.

- `cargo xtask bench setup` no longer fails with `destination path … already exists` when an interrupted clone left
  the pre-commit-hooks mirror directory without `HEAD`: it removes it and clones again.

- `snp gc` no longer removes repositories still in use after a first `install-hooks` (or `run`, `try-repo`) on a
  fresh store: the config was not tracked when the store did not exist yet, so a following gc removed its
  repositories (for example `3 repo(s) removed.` where pre-commit 4.7.0 prints `0 repo(s) removed.`). The config is
  now recorded as soon as the store is created.
- A file an editor saves while `run` installs a hook environment (or clones a repository, or downloads a tool) is no
  longer reported as modified by the first hook when the install took under 250 ms: the baseline `git diff` is taken
  after any clone, install or download, as pre-commit does (ADR-078(3), ADR-104). Before, only the time spent
  decided it.
- The stash no longer loses a working-tree change that `git diff` cannot see: a line a clean filter hides (for
  example `filter.<name>.clean = grep -v SECRET`) or an edit in an `--assume-unchanged` file was overwritten whenever
  another file had an unstaged change, because the stash checked out the whole tree (pre-commit 4.7.0 does the same;
  prek fixed it in j178/prek#2912). The stash now checks out only the files of its patch, and the conflict rollback
  those plus every file `git diff` shows changed, so hook fixes are still discarded as before. A failing stash
  checkout's `CalledProcessError` names the new command; recovery hints with too many files to spell out read them
  from a kept `<patch>.paths` list instead of restoring the whole worktree. New divergence D-DATA-19 (ADR-093).
  The path-limited checkout refuses a list holding an empty path or a path with a NUL byte with `ValueError` before
  calling git, since an empty `:(literal)` pathspec would select the whole tree, and the rollback list drops empty
  `git diff` records. Git-made lists never hold such a path, so no run changes in practice (D-DATA-19 amendment,
  ADR-095 K7).
- xargs partitioning no longer accepts an argument whose length overflows the command-length arithmetic: with a
  budget of `usize::MAX` the saturating sum let such an argument fit, where pre-commit's unbounded integers raise
  `ArgumentTooLongError`. The greedy loop is now the kernel `partition_lens`, which is proved to satisfy a spec transcribed
  from `xargs.partition` (ADR-095). Real budgets are at most 131072, so no run changes in practice.
- `gc` and `clean` no longer delete a stash patch that an unrestored stash journal still needs for recovery (possibly
  the only copy of unstaged work after a failed restore). The journal was searched for the patch's raw file name, so
  the patch was unprotected when the store's path is not valid UTF-8 (the journal stores it as `{"hex": …}`), when the
  journal was bigger than 1 MiB (a long intent-to-add list pushes the name past the read cap), or when the journal could
  not be read at all. Journals are now decoded with the stash's own codec and read in full; a journal that cannot be
  read or understood keeps every stash patch and `gc`/`clean` print `[WARNING] the stash journal <journal> cannot be
  read or understood; every stash patch is kept. Remove the journal once no snp run uses it and no patch it may name is
  needed.` on stderr. The `<patch>.*` files next to an unrestored patch (the `<patch>.now` backup the recovery hint
  suggests, the kept `.ita` and `.paths` lists) are kept with it. pre-commit's `gc` never looks at patches and its
  `clean` removes the whole store; divergences D-DATA-04 and D-DIAG-05 are updated.
- CI fuzzing: the fuzz shards run with a 64 MB AddressSanitizer quarantine (`ASAN_OPTIONS=quarantine_size_mb=64`,
  default 256 MB) to keep targets well below the 512 MB RSS cap they crept toward; leak detection, use-after-free
  detection and the §13.5 limits are unchanged.
- CI fuzzing: the fuzz shards also compress AddressSanitizer's stack depot (`compress_stack_depot=2`). The depot keeps
  every distinct allocation and free stack for the life of the run and never shrinks; `cfgv_validate`, whose inputs
  are nested values, added about 1.4 stacks per execution and crossed the 512 MB RSS cap after about 840k executions
  of the nightly run with a 30 MB live heap. Compressed, the depot takes about a sixth of the memory (locally 257 MB
  to 42 MB, peak RSS 421 MB to 246 MB after ~756k executions); reports keep their full stacks, and the RSS cap and
  every other limit are unchanged. The product was not at fault: without sanitizers the target stays at 31 MB, and the
  reported input alone peaks at 39 MB.
- A `files`/`exclude` pattern with `\b` or `\B` no longer runs the backtracking engine on every non-ASCII path: the
  relaxed linear prefilter now guards tier 1's exact fallback too, so a path it rejects is "not matched" in
  milliseconds (as in pre-commit) instead of taking seconds or aborting the run with a backtracking-limit error
  (for example `^(.+/)*\bconfig\.yaml$` on a deep non-ASCII path). On the B12u benchmark the backtracking engine runs
  1 060 times instead of 20 000, and snp's own user CPU falls by about 19 % (ADR-098).
- The stash lists its patch's files (`git apply --numstat`) before the re-verification instead of after it, so an
  editor autosave that lands while the list is taken is caught and stashed instead of overwritten by the checkout
  (the window was about 48 ms for a 10 MB patch). A listing error is still reported only once the patch is verified.
- The cached `git ls-files` and `git diff --staged --diff-filter=A` answers of the pre-commit-hooks natives are
  recomputed when the repository config changes during a run (they follow `core.quotePath` and `diff.renames`); before,
  a hook that changed either setting left the next native with a stale list (ADR-102).

### Security

- The embedded uv is 0.13.8 (was 0.12.23): uv 0.13.5 and later reject certificate chains through the compromised
  eMudhra intermediate CAs.
- The TLS trust store follows Mozilla's latest removals: webpki-roots and webpki-root-certs 1.0.12 and rustls-webpki
  0.103.17 (also in the fuzz workspace, which was still on rustls-webpki 0.103.15 without the eMudhra distrust).
- rustix 1.1.6 (RUSTSEC-2026-0486; snp calls none of the affected xattr functions).

## [2.0.0] - 2026-11-02

snp 2.0.0 is a complete rewrite: a drop-in replacement for pre-commit 4.6.2. It shares only the name with the
unrelated snp 1.x releases of the crate.

### Added

- Byte-for-byte compatibility with pre-commit 4.6.2 for configs, manifests, commands, options, output, exit codes and
  side effects, in two brands chosen by the invocation name: `pre-commit` (exactly pre-commit 4.6.2) and `snp` (the
  same, named `snp`, with the snp-only commands and options). Every deliberate difference is registered in the
  [DIVERGENCES](docs/user/divergences.md) book with a differential test.
- All pre-commit commands: `run`, `install`, `uninstall`, `install-hooks`, `init-templatedir`, `autoupdate`, `gc`,
  `clean`, `try-repo`, `validate-config`, `validate-manifest`, `migrate-config`, `sample-config`, `hazmat`,
  `hook-impl`, `help`.
- snp-only commands: `doctor [--fix] [--fast-path] [--stash]`, `list`, `config dump|schema`, `cache dir|size|gc|clean`,
  `migrate lefthook|builtin|upstream`, `self alias install|uninstall|status`, `self check`, `completions`.
- Config extension keys, all optional and ignored (with pre-commit's usual warning) by Python pre-commit: `priority`
  and root `priorities` (parallel buckets), `env`, `groups` (`--group`, `--require-group`, `--no-group`,
  `SNP_SKIP_GROUPS`), `stage_fixed`, `skip`/`only` conditions, `timeout`, `fail_text`, `root`, `use_stdin`,
  `fail_on_changes`, `interactive`, `minimum_snp_version`, `allow_disable`, `fast_path*`, `output`; `repo: builtin`;
  the local overlay `.pre-commit-config.local.yaml`.
- `interactive: true` hooks (lefthook's TTY passthrough, ADR-085): the hook reads the terminal (`/dev/tty` when git
  gives it `/dev/null`) and writes to it directly, runs alone, and is announced with a `name....(interactive)` line;
  without a terminal it runs captured after a one-line note, and `SNP_NO_TTY=1` opts out. `snp migrate lefthook`
  keeps `interactive` (still dropped with `--portable`) and substitutes positional templates inside double quotes
  without extra quotes (`test -n "{2}"` → `test -n "$PRE_COMMIT_COMMIT_MSG_SOURCE"`).
- `.snp.yaml` (or `.snp.yml`) as the config file name (ADR-087): without `-c`, every command reads `.snp.yaml`, then
  `.snp.yml`, then `.pre-commit-config.yaml` from the repository root (through the `pre-commit` alias
  `.pre-commit-config.yaml` keeps priority); `.snp.yaml` beside `.pre-commit-config.yaml` wins with a one-line note,
  both snp names are an error, the hook scripts embed the file found, and its local overlay is `.snp.local.yaml`.
  `snp migrate lefthook` writes `.snp.yaml` (`.pre-commit-config.yaml` with `--portable`).
- Native implementations of the pre-commit-hooks hooks (tags v3.0.0 to v6.0.0) and of pygrep hooks, used
  automatically when they are verified identical (the fast path).
- 21 language backends with verified toolchain downloads; Python environments created with uv by default.
- snp's own cache (`$SNP_HOME`, `$XDG_CACHE_HOME/snp` or `~/.cache/snp`) with versioned, hash-keyed entries; it never
  reads or writes pre-commit's cache.
- A journaled stash of unstaged changes that is recovered after a crash or `kill -9`.
- Hook scripts that recognise and replace pre-commit, prek and snp 1.x scripts in place, embed a stable path of snp and
  run under POSIX `sh` (BusyBox included).
- Releases for Linux (glibc and static musl, x86_64 and arm64), macOS (arm64 and x86_64) and Windows (MSVC with static
  CRT, x86_64 and arm64) with shell and PowerShell installers, SHA-256 checksums, CycloneDX SBOMs, auditable binaries
  and GitHub attestations; the `snp-cli` wheel on PyPI; the `snp-action` GitHub Action.
- snp 2.x on crates.io (ADR-088): `cargo install --locked snp` (and `cargo binstall snp`, which fetches the release
  archive) installs this release; the internal crates it is built from (`snp-core`, `snp-cli`, ...) are published
  with it, each requiring exactly the same version. The crate name `snp` previously held the unrelated snp 1.x.
- A Homebrew formula in the grok-rs/homebrew-tap tap: `brew install grok-rs/tap/snp` (macOS and Linux; `snp` only,
  no `pre-commit` alias).
- User documentation: installation, migration guide, CI integration, configuration reference, troubleshooting, the CLI
  reference generated from the parser tables and the DIVERGENCES book generated from the register.
