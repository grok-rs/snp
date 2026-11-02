# snp — Compatibility policy, branding, DIVERGENCES register, extension keys

This document is normative for *what* snp must output and *where* it may differ from pre-commit. ARCHITECTURE.md
says *how*; DECISIONS.md says *why*.

## 1. Reference

* **Reference implementation:** pre-commit **4.6.2** with its pinned dependencies (cfgv 3.5.0, identify 2.6.20,
  PyYAML 6.0.3 with libyaml `CSafeLoader`/`CSafeDumper`, virtualenv 21.14.5, nodeenv 1.11.0).
* **Oracles** (built hash-locked by `cargo xtask oracle install`, never taken from `PATH`):
  * `pc462-py312` — pre-commit 4.6.2 on CPython 3.12.3: runtime behaviour, regex accept/reject and matching, the
    performance baselines of 10 §0 (`--version` 172 ms, 1-hook run 234 ms, 10-hook run 243 ms);
  * `pc462-py313` — pre-commit 4.6.2 on CPython ≥ 3.13.14: argparse help/usage/error texts (canonical; identical to
    3.14.8 texts).
* **Pre-commit-hooks** fast-path references: upstream tags v3.0.0 … v6.0.0, each in a pinned venv on CPython 3.13.
* **Python semantics profiles** per subsystem: ARCHITECTURE §0.3 (regex grammar CPython 3.12 with UCD 16 data;
  argparse texts 3.13.14+; `repr`/EAW UCD 15.1).

## 2. The compatibility contract

| # | Contract | How it is proven |
|---|---|---|
| K1 | **Inputs.** Every `.pre-commit-config.yaml` / `.pre-commit-hooks.yaml` that pre-commit 4.6.2 accepts is accepted with identical meaning; every one it rejects is rejected with the identical message — except where pre-commit crashes (§5, CRASH). | YAML/schema corpus through the differential oracle |
| K2 | **Bytes.** For configs that use no snp extension, in the **compat brand**, stdout, stderr and exit code are byte-identical to the oracle (Tier A), except registered divergences. In the **snp brand** the reference is the oracle run with argparse `prog` patched to `snp` (so usage/help/error blocks are wrapped exactly as argparse wraps them for the shorter prog), with the deterministic `brand_map` (§3.3) applied to every non-argparse text; snp's bytes must then be identical. Parsers that carry snp-only options are compared with the WP-12 goldens rendered by a real CPython argparse built from `SNP_SPEC` (D-EXT-07). | `snp-difftest` runs every scenario in both brands |
| K3 | **Side effects.** Worktree bytes and modes, index (`git ls-files -s`), refs, stash list and the behaviour of installed hook scripts are identical. | difftest snapshots tree/index/refs after each step |
| K4 | **Divergence policy.** snp differs only (a) where pre-commit crashes (exit-3 traceback, double fault, hang), (b) where it loses or corrupts user data, (c) where it has a security bug, or (d) where an owner decision forces it (FD-1 branding, FD-2 own store, FD-5 no `unsafe`, FD-7 uv/performance, FD-8 fast path, extensions that are active only when used). Every divergence has an ID below, a rationale, and a differential scenario asserting it **still diverges**. | `tests/differential/divergences/<ID>.yaml`; an unexpectedly matching scenario fails CI |
| K5 | **Fail closed.** No divergence may turn a non-zero outcome into zero unless every selected hook actually ran and passed. | review rule + crash-catalog scenarios (13 X.3 rule 5) |
| K6 | **Extensions are additive.** snp-only keys, flags and commands never change the bytes produced for a config that does not use them, never re-type a pre-commit key, and never break an existing long-option abbreviation (prefix-stability test). | golden + property tests |

**Comparison tiers.** **A** byte-exact. **B** semantic — same exit code and stream, structured comparison: crash-log
body below the version block, Python tracebacks reduced to their last line, usage/help blocks compared against the
`pc462-py312` oracle with the documented 12 §A.4.1 deltas, localized `[WinError N]` texts until verified on Windows
runners. **C** ignored — durations, stash patch file names and pids, random directory suffixes, store root paths (the
stores differ by design), installer chatter between `Installing environment` lines, xargs partition boundaries within a
few bytes of ARG_MAX (synthetic interpreter prefix).

## 3. Branding by `argv[0]`

### 3.1 Detection

`brand = basename(argv[0])` minus a trailing `.exe` (Windows, ASCII-case-insensitive) compared with `pre-commit`
(exact on Unix, ASCII-case-insensitive on Windows): equal ⇒ **compat brand**, anything else ⇒ **snp brand**.
`current_exe()` is never used (Linux resolves symlinks, which would hide the alias). There is no environment override.
Compat entry points: a `pre-commit` symlink/hard link/copy (`snp self alias install`), the pre-commit hook template's
`exec pre-commit` fallback, hook scripts written by `pre-commit install` through the alias (they embed the alias path
as `SNP_EXE`, so every commit keeps the compat brand, ADR-060), and (v1.1) the opt-in `snp-pre-commit-compat`
distribution.

### 3.2 Compat brand

Every byte as pre-commit 4.6.2 — command set, help, usage, choices lists, abbreviations, messages, `--version`
(`pre-commit 4.6.2`), the min-version hint (`Perhaps run \`pip install --upgrade pre-commit\`.`), pre-commit's bug URL in
the migration-mode text — except: store-derived paths (the store is snp's own, FD-2), the crash-log body, and the
registered divergences. snp-only commands and flags are **not accepted** in the compat brand (pre-commit's own
`invalid choice`/`unrecognized arguments` texts result). Environment variables (`SNP_*`) work under both brands.

### 3.3 snp brand and `brand_map`

Same layout; `--version` prints `snp 2.0.0`; snp-only commands and flags are available.

**argparse texts** (usage, help, usage errors: `usage: snp …`, `snp: error: …`, `snp run: error: …`) are **not** produced
by substitution: argparse wraps them relative to `usage: {prog} {cmd} `, so the shorter prog moves wrap points and
continuation indentation. They are rendered by snp-argparse from `SNP_SPEC` with prog `snp` and tested against the
pc462-py313 oracle with argparse `prog` patched to `snp` (parsers identical to pre-commit's) and against a real CPython
3.13 argparse built from `snp_argparse::export_json(SNP_SPEC, "snp")` (parsers with snp-only options; ADR-058).

**Every other text** is the compat text with these substitutions only (`tests/differential/brand_map.yaml` is the
machine-readable form; the difftest applies it to the prog-patched oracle's output and then requires byte equality):

| Compat text (pre-commit 4.6.2) | snp brand |
|---|---|
| `--version` / `-V` line `pre-commit 4.6.2` (whole-line rule: a plain substitution would give `snp 4.6.2`) | `snp 2.0.0` |
| `pre-commit installed at {path}` | `snp installed at {path}` (`{hook_type} uninstalled`, e.g. `pre-commit uninstalled`, names the hook type and is not branded, ADR-084) |
| `Use -f to use only pre-commit.` | `Use -f to use only snp.` |
| ``Skipping `pre-commit`.`` | ``Skipping `snp`.`` |
| missing-config block lines 3–4 (`- To permanently silence this, install pre-commit with the --allow-missing-config option`, ``- To uninstall pre-commit run `pre-commit uninstall` ``) | `snp` substituted in both lines |
| show-diff banner lines 2–3 (``reproduce locally with: `pre-commit run --all-files`.``, ``To run `pre-commit` as part of git workflow, use `pre-commit install`.``) | `snp` substituted |
| hints `` `pre-commit autoupdate` ``, `` `pre-commit migrate-config` ``, `pre-commit autoupdate --repo …` | `snp …` |
| `pre-commit normalizes …`, `pre-commit failed to diff …`, `for now, pre-commit requires/does not support …`, `pre-commit requires system-installed "cs" …`, `pre-commit has detected a mix of cygwin …`, `Locking pre-commit directory` | `snp …` |
| `Try upgrading identify and pre-commit?` | `Try upgrading snp?` |
| `pre-commit version {v} is required but version 4.6.2 is installed.  Perhaps run \`pip install --upgrade pre-commit\`.` | `pre-commit version {v} is required but snp 2.0.0 is compatible with pre-commit 4.6.2.  Perhaps upgrade snp.` |
| migration-mode text: `bug: pre-commit's script is installed in migration mode` / ``run `pre-commit install -f --hook-type {T}` to fix this`` / `Please report this bug at https://github.com/pre-commit/pre-commit/issues` | `bug: snp's script …` / ``run `snp install -f --hook-type {T}` …`` / `Please report this bug at https://github.com/grok-rs/snp/issues` |
| crash-log file name `pre-commit.log`; crash-log key `pre-commit version: 4.6.2` | `snp.log`; `snp version: 2.0.0` |
| help strings naming the tool (`main.py:222,245,273,290,299,335`) | `snp` (part of the rendered SNP_SPEC help, compared exactly against the prog-patched/JSON-built argparse references) |

**Never substituted:** file names (`.pre-commit-config.yaml`, `.pre-commit-hooks.yaml`), environment variable names
(`PRE_COMMIT_*`, `SKIP`), the phrases `pre-commit hook(s) made changes.` and `Your pre-commit configuration is
unstaged.` (they describe the framework and config format; CI greps match them), `https://pre-commit.com/…` URLs,
cfgv object names.

## 4. Divergence policy and audit rule

A difference from pre-commit 4.6.2 is admissible only in these classes; every entry of §5 names one:

| Class | Admissible because |
|---|---|
| **CRASH** | pre-commit crashes (exit-3 traceback, double fault) or hangs; snp behaves sensibly and fails closed |
| **DATA** | pre-commit loses or corrupts user data (worktree, index, unstaged work, config files, hook scripts) |
| **SEC** | pre-commit has a security bug (traversal, injection, unverified downloads, unbounded resources) |
| **STORE** | forced by FD-2 (snp's own store, layout and maintenance) |
| **BRAND** | forced by FD-1 (identity of the tool, hook script) |
| **PLAT** | forced by FD-5 (no `unsafe`, so a platform API is unavailable) or a platform crash fix |
| **PERF** | forced by FD-7/FD-8 (uv, fast path) and invisible in hook output |
| **EXT** | only observable when an snp extension (key, flag, env var, command) is used |
| **DIAG** | stderr-only snp diagnostics that are always on (old-git warning, install migration notes, the TTY-only run hint, recovery/lock notices, store-ownership warnings): stdout and exit codes unchanged; each has a scenario |
| **TIERB** | Python-internal bytes that cannot be reproduced (tracebacks with interpreter paths, Python's own warning lines) |

Rejected during the audit (now parity): chronological piped output, LF instead of CRLF for `print()` on Windows,
`anstyle-query` colour detection, `clean` silent when the store is absent, post-rewrite stdin forwarded to legacy hooks,
`\z` accepted in regexes, silently ignored `x-*` keys, `NO_COLOR`/`FORCE_COLOR` support, `snp X (pre-commit 4.6.2
compatible)` / `pre-commit 4.6.2 (snp X)` version strings, snp bug URLs and upgrade hints in the compat brand,
`Terminated: SIGTERM:` banners, refusing foreign-owned stores (except root over a user's store, D-SEC-13),
`%LOCALAPPDATA%` store on Windows, lazy installation of delegated fast-path hooks, missing `Initializing`/`Installing`
lines for fast-path repos, `--config-hooks` (broke `--conf` abbreviations), a persistent store for try-repo (dropped the
per-run `Initializing`/`Installing` lines), M1–M3 install notes on stdout, a blanket refusal of git < 2.19, a
foreground-clone retry only after locale-dependent auth errors, native hooks that ignore file names shaped like flags,
upper-case-free Windows env copies, mimalloc-less musl without measurement.

## 5. DIVERGENCES register (every deliberate difference from pre-commit 4.6.2)

Each ID has `docs/divergences/<ID>.md` (rationale, examples) and `tests/differential/divergences/<ID>.yaml` (a scenario
that must keep diverging). WP-00 pre-creates both as stubs; the **first** work package in the WP column owns and fills
them (WBS.md lists the files in its owned paths); others listed contribute behaviour. Catalog references are 13 X.4
rows (`X13-…`, ARCHITECTURE §13.8).

### 5.1 CRASH — pre-commit crashes or hangs; snp behaves and fails closed

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-CRASH-01 | `minimum_pre_commit_version: "abc"` (also `""`, `"4."`, `"4.6.3rc1"`) → `ValueError` exit 3 (X13-CFG-01) | validation error leaf `invalid literal for int() with base 10: 'abc'`, exit 1 | 21 |
| D-CRASH-02 | non-string / mixed root keys → `TypeError` exit 3 (X13-CFG-02/03) | `[WARNING] Unexpected key(s) present at root: …` (Python `str()` of the keys, sorted by type rank) | 21 |
| D-CRASH-03 | unhashable `stages`/`language`/`types` values → `TypeError` exit 3 (X13-CFG-04, X13-CFG-13 in manifests) | the one-of / type-tag validation errors, exit 1 | 21 |
| D-CRASH-04 | regex repeat ≥ 4294967295, nesting ≥ 485, `(?a)(?u)` in `files`/`exclude` → `OverflowError`/`RecursionError`/`ValueError` exit 3 (X13-CFG-05…07, 13) | validation error `'<repr>' is not a valid python regex`, exit 1 | 21, 13 |
| D-CRASH-05 | empty `entry`, shlex errors, `ArgumentTooLong`, unreadable entry script → exit 3 (X13-EXE-01…04) | the hook fails (`Executable `` not found`, `ValueError: No closing quotation`, `[Errno 7] Argument list too long: '<exe>'`, …), the run continues | 32 |
| D-CRASH-06 | `autoupdate`: one unreachable repo → `CalledProcessError` exit 3, nothing written (X13-GIT-02) | per-repo `[{repo}] <error>` lines, the others are updated and written, exit 1 | 33 |
| D-CRASH-07 | `migrate-config`/`autoupdate` with a missing config, a non-UTF-8 config, or an invalid re-indented fallback → exit 3 (X13-CFG-10…12) | validation-style errors, exit 1 | 21 |
| D-CRASH-08 | file deleted by an earlier hook before its tags were computed → `ValueError … does not exist.` exit 3 (X13-CLS-01/04) | file excluded from this and later hooks | 22 |
| D-CRASH-09 | FIFO / device passed via `--files` → hangs forever (X13-CLS-03) | opened non-blocking, classified, never blocks | 22 |
| D-CRASH-10 | pre-push with a 64-hex all-zero OID (SHA-256 repo) → crash on branch deletion | any all-zero OID (40 or 64 hex) is null | 18 |
| D-CRASH-11 | non-UTF-8 pre-push stdin → `UnicodeDecodeError` exit 3, push blocked (X13-HI-04) | same algorithm on bytes (ASCII whitespace), hooks run | 18 |
| D-CRASH-12 | non-UTF-8 file names from git or `--files` → `UnicodeDecodeError`/`UnicodeEncodeError` exit 3 (X13-GIT-05…08) | bytes end to end (argv, identify); lossy UTF-8 view for regex matching | 22 |
| D-CRASH-13 | non-UTF-8 output of tool probes (`node --version`, …) → exit 3 | decoded lossily | 16, 24, 25 |
| D-CRASH-14 | broken legacy hook (missing shebang interpreter, directory) → exit 3 (X13-HI-07) | stderr message, legacy rc 1, snp's hooks still run | 32 |
| D-CRASH-15 | unwritable `log_file` → exit 3 and the hook output is lost (X13-EXE-05) | output printed, `[WARNING]` line, run continues | 32 |
| D-CRASH-16 | pty allocation failure in colour mode → exit 3 (X13-EXE-06) | falls back to pipes | 15 |
| D-CRASH-17 | store not creatable/corrupt (stale db rows, locked `db.db`, gc ENOENT, double faults in the error handler: X13-ST-*, X13-CMD-07, X13-EH-01/02) → exit 3 / tracebacks | marker-based store never needs repair; commands that do not need the store work; the error handler never double-faults | 22, 33 |
| D-CRASH-18 | an unreadable file is identified (and crashes, exit 3) even when no hook needs its contents (X13-CLS-02) | crash (exit 3, same text) only when a selected hook's filter needs the content tier | 22 |
| D-CRASH-19 | explicitly requested Python version not installed → install crash exit 3 | uv-managed download into the store (`SNP_PYTHON_DOWNLOADS=never` restores the failure) | 24 |
| D-CRASH-20 | Windows node hook with explicit `language_version: system` → exit 3 | shim-less system env | 24 |
| D-CRASH-21 | `docker system info` fails during the rootless probe → `CalledProcessError` exit 3 | treated as "not rootless" | 25 |
| D-CRASH-22 | `pre-commit hazmat` with no tool → `AttributeError` exit 3 | argparse usage error, exit 2 | 26 |
| D-CRASH-23 | `end-of-file-fixer` (and the other fixers) on a read-only file that needs no change → crash in the hook | passes (opens for writing only when a change is needed) | 26 |
| D-CRASH-24 | stdout closed (EPIPE) → chained Python tracebacks on stderr, exit 120 (X13-EH-03) | silent, exit 120, stash restored | 33, 00 |
| D-CRASH-25 | `migrate-config` on a list-root / non-UTF-8 config under a non-UTF-8 locale (Windows cp1252) → crash or mis-decode | UTF-8 always (see D-DATA-09) | 21 |
| D-CRASH-26 | git < 2.31: `run --all-files`, the pre-push root push and the meta hooks call `ls-files -z --deduplicate` → `CalledProcessError` (rc 129) exit 3 (a traceback inside the meta hook's output) (02 G.3) | `ls-files -z` + adjacent dedup (byte-identical list); the real flag is kept on ≥ 2.31 for argv parity in error texts | 18 |

### 5.2 DATA — pre-commit loses or corrupts user data

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-DATA-01 | final stash-restore failure prints only a traceback; the user does not learn where the unstaged work is | one stderr line after the envelope: `[ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --whitespace=nowarn <patch>`, plus what to do when it does not apply (X13-STASH-02) | 23 |
| D-DATA-02 | SIGTERM/SIGHUP kill pre-commit without restoring the stash (unstaged work only in the patch) | descendants of the cancelled hooks are killed (Linux subreaper, Windows job), then the stash is restored (restore messages printed), then death by the same signal (exit status 143/129 as before). Residual risk, documented: on macOS an orphaned descendant of an Inherit-group hook (e.g. a formatter started by a shell entry) cannot be found and may still write after the restore; the patch file is kept, so the work stays recoverable | 23, 15 |
| D-DATA-03 | `install` silently overwrites an existing foreign `.legacy` hook | refused unless `-f`; a `.legacy` that is itself a manager script is removed | 33 |
| D-DATA-05 | hook script: bash template written through symlinks, CRLF on Windows | POSIX `sh` template, LF everywhere, atomic write that replaces a symlink instead of writing through it (template identity: BRAND) | 33 |
| D-DATA-07 | an exported `CDPATH` corrupts `--hook-dir`; legacy hooks (git-lfs pre-push) are silently skipped | value repaired (all lines equal and an existing directory ⇒ use one line) | 32 |
| D-DATA-08 | relative local-path repos (`repo: ../tools`) share one cache entry across projects | keyed by the canonical absolute path; messages unchanged | 22 |
| D-DATA-09 | migrate-config/autoupdate read/write with the locale encoding and rewrite CRLF files as LF; non-atomic writes | UTF-8 always, each line's original EOL kept, atomic replace keeping the mode | 21 |
| D-DATA-10 | `autoupdate --freeze` rewrites a CRLF `rev:` line with LF (mixed endings) | the line keeps its `\r` (DEV-PY-1) | 21 |
| D-DATA-11 | `GIT_DIFF_OPTS=--unified=0` makes the stash patch unrestorable (hazard a) | `GIT_DIFF_OPTS` removed for the stash `diff-index` | 23, 18 |
| D-DATA-12 | an intent-to-add file paired as a rename is missed ⇒ restore fails and a deletion is lost (hazard b) | `--no-renames` on the ITA query | 23, 18 |
| D-DATA-13 | two concurrent runs in one worktree can stash over each other; a killed run leaves no recovery hint | per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice on stderr (`git apply --whitespace=nowarn`, never overwriting later edits) | 23 |
| D-DATA-14 | migrate-config doubles the rewrite of an aliased `stages` list (`[pre-commitpre-commit]`) | rewrites deduplicated by YAML node | 21 |
| D-DATA-15 | migrate-config turns a folded `language: >-\n  python_venv` into invalid YAML | block scalars rewritten safely | 21 |
| D-DATA-16 | autoupdate: a config without a final newline triggers a full PyYAML re-dump that drops all comments | end of file treated as a line end; comments kept | 21 |
| D-DATA-17 | git < 2.19 does not report intent-to-add files: a stash with an ITA file fails after the rollback (`already exists in working directory`, exit 3), leaving the unstaged work only in the patch (02 G.3); runs without a stash work | a stash that would be non-empty on git < 2.19 is refused before any mutation: `An error has occurred: FatalError: git <V> cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged changes (found <git>).` (exit 1); every run that needs no stash works as in pre-commit | 23, 18 |
| D-DATA-18 | a SIGINT sent to pre-commit alone (`kill -INT <pid>`: an IDE or CI cancel) becomes `KeyboardInterrupt` in its own process only: the running hook never sees it and keeps running, orphaned, after the stash was restored (X13-EH-04) | the sent SIGINT is forwarded to every running hook (ARCHITECTURE §10.4, ADR-065), leftover descendants are killed (Linux), then the stash is restored and the run exits 130 as before; a terminal ^C (kernel origin) is not re-sent | 15, 32 |

### 5.3 SEC — security

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-SEC-01 | `language_version: ../../x` (or an absolute path) makes the install step delete arbitrary directories; Windows `C:` versions break | sanitized-or-hashed env dir names; deletion contained in the repo's `envs/` | 22 |
| D-SEC-05 | installers and snp-owned repo operations inherit `GIT_DIR`/`GIT_INDEX_FILE` (index corruption in linked worktrees; autoupdate writes partial-clone config into the user's repo — hazard c) | installers and owned repos get pre-commit's `no_git_env` allow-list | 18, 24, 25 |
| D-SEC-06 | clones fail when the user's git defaults to SHA-256 objects (hazard d) | one retry with the other `--object-format` after `mismatched algorithms` | 18 |
| D-SEC-07 | unset `PATH` ⇒ activation patches produce `"<bin>:"` (cwd on PATH) | no trailing separator | 00 |
| D-SEC-08 | toolchain downloads (node, go, rustup, ruby helpers) are not verified | TLS + sha256 verified before extraction; safe extraction | 16 |
| D-SEC-09 | shebang reads are unbounded | shebang line capped at 64 KiB (file-identify) | 15 |
| D-SEC-10 | catastrophic regexes hang (tier-1-able patterns are exponential in CPython; backtracking patterns unbounded) | linear tier 1; tier 2 `backtrack_limit` 1e8 → `An error has occurred: snp: regular expression {pattern!r} exceeded the backtracking limit on {path!r}` (exit 1) | 13 |
| D-SEC-11 | YAML alias bombs / deep nesting / huge files exhaust memory or the stack | depth 512, 1 M constructed nodes, 16 MiB file limits → YAML error | 14 |
| D-SEC-12 | pre-push stdin (and the `use_stdin` copy) is read without a limit | more than 64 MiB fails closed before the legacy hook and the hooks run: `An error has occurred: FatalError: snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list` (exit 1); never truncated | 32 |
| D-SEC-13 | a store writable by another user is used as is, including when running as root (tools and environments that user can replace get executed) | as root (euid 0), a store owned by a non-root uid is refused with `An error has occurred: FatalError: snp refuses to run as root from a store owned by uid <n>: <path>; set SNP_ALLOW_FOREIGN_STORE=1 to trust it` (exit 1) unless `SNP_ALLOW_FOREIGN_STORE=1`; other foreign owners only warn (D-DIAG-05) | 22 |

### 5.4 STORE and BRAND — forced by FD-2 and FD-1

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-STORE-01 | store `$PRE_COMMIT_HOME` or `~/.cache/pre-commit` (SQLite `db.db`); crash log `pre-commit.log` there | own store `SNP_HOME` > `$XDG_CACHE_HOME/snp` > `~/.cache/snp` (Windows `%USERPROFILE%\.cache\snp`), versioned layout, no SQLite; refuses pre-commit's store; crash log named by brand in snp's store; the crash-log body (below the version block) is snp's | 22, 33 |
| D-STORE-02 | `clean` also removes `~/.pre-commit`; `gc`/`clean` operate on pre-commit's store | snp never touches pre-commit's directories; `clean` always prints `Cleaned {snp store}.` | 33 |
| D-DATA-04 | restored stash patches are kept forever | gc removes *restored* patches older than 30 days (never unrestored ones); `clean` keeps journals, unrestored and < 1 h old patches, `.lock` and files outside snp's layout — class STORE | 22 |
| D-BRAND-01 | `--version` and texts | snp brand: `snp 2.0.0`, the §3.3 substitutions and argparse texts rendered with prog `snp`; compat brand: byte-identical | 12, all |
| D-BRAND-02 | `install` treats prek and snp 1.x hook scripts as foreign: moves them to `.legacy` and prints `Running in migration mode with existing hooks at …` / `Use -f to use only pre-commit.` (both tools then run on every commit; a pre-commit script in `.legacy` blocks every commit, 11 §A.5.2 chains C1–C4) | prek and snp 1.x scripts are manager scripts (hook-script identity): replaced in place, no `.legacy`, no migration-mode lines; `uninstall` still restores a `PreCommit`/`Prek` `.legacy` (rollback) | 33 |
| D-MISC-02 | `pre-commit __hazmat …` → `invalid choice: '__hazmat'`, exit 2 | `argv[1] == "__hazmat"` is reserved (in-hook hazmat entry, both brands) — class BRAND | 33 |

### 5.5 PLAT — platform

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-PLAT-02 | Windows: a missing absolute shebang interpreter (`#!/bin/sh`) fails the hook / crashes the legacy hook | falls back to the interpreter's basename on PATH (Git for Windows semantics) | 15 |
| D-PLAT-03 | Windows: batch-file (`.cmd`/`.bat`) arguments with `&`, `^`, `%VAR%` are interpreted by cmd.exe | Rust's BatBadBut-safe quoting; arguments containing CR/LF fail the hook with ``Executable `{cmd0}` cannot receive an argument containing a line break: {repr}`` (rc 1) — class SEC | 15 |
| D-PLAT-06 | Windows: config/`--files` on another drive → `ValueError` (relpath) exit 3 | absolute path kept | 32 |
| D-PLAT-08 | cygwin/MSYS2 git on Windows: a warning, then undefined behaviour | clear error, exit 1 | 18 |
| D-PLAT-09 | Windows: `.cmd` partitions can exceed 8191 characters after quoting (crash) | partitions re-split | 15 |
| D-PLAT-10 | Windows CPU count = `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`; on macOS and Windows under CPython ≥ 3.13 a positive `PYTHON_CPU_COUNT` replaces the count (`multiprocessing.cpu_count()`; Linux uses `sched_getaffinity`, which ignores it) | `available_parallelism` (differs only above 64 logical CPUs; no `unsafe`); `PYTHON_CPU_COUNT` ignored on every OS (14 A.9 Q3; `SNP_JOBS` sets the count) | 15 |
| D-PLAT-12 | Windows: a directory junction is tagged `directory` | tagged `symlink` (reading the reparse tag needs `unsafe`) | 22 |

### 5.6 PERF — forced by FD-7/FD-8, invisible in hook output

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-PERF-01 | Python envs built with virtualenv + pip | uv (`--seed`) with pip-config translation and a pip fallback; seeded packages may differ in version, and uv's resolver can differ on pre-releases, yanked versions and extra-index strategy (FD-7); `SNP_PYTHON_INSTALLER=pip` restores pip's resolver per env (ADR-073) | 24 |
| D-PERF-02 | local `language: python` hooks without deps `pip install` a placeholder package | placeholder install skipped | 24 |
| D-PERF-03 | pre-commit-hooks run from a cloned venv | native implementations (fast path, ADR-031); output byte-identical; Tier A repos are never cloned (FD-8) | 31, 26, 27 |
| D-PERF-04 | docker hooks run `docker build` every run | skipped when the tagged image exists (`SNP_DOCKER_ALWAYS_BUILD=1` restores it) | 25 |

### 5.7 EXT — observable only when an snp extension is used

| ID | Divergence | WP |
|---|---|---|
| D-EXT-01 | recognised extension keys (§7) produce no `Unexpected key(s)` warning and take effect | 21 |
| D-EXT-02 | the local overlay `.pre-commit-config.local.yaml` / `SNP_LOCAL_CONFIG` | 21 |
| D-EXT-03 | local-overlay notices on stderr: `snp: using local overrides from {path}`, the ignored-overlay note (tracked by git / CI), `snp: local config overrides unknown hook '<id>'` | 21 |
| D-EXT-04 | `_SNP_STASH_LOCK=<token>` is exported to hooks while a stash is held (nested runs skip the worktree lock) — class DATA | 23 |
| D-EXT-05 | `SNP=0`/`false` makes `hook-impl` skip snp's hooks (the legacy hook still runs) | 32 |
| D-EXT-06 | `repo: builtin` (pre-commit rejects these configs) | 21, 26, 31 |
| D-EXT-07 | snp-brand commands and flags (§8), including their presence in the snp brand's usage/help blocks | 12 |

### 5.8 DIAG — always-on stderr diagnostics (stdout and exit codes unchanged)

| ID | Diagnostic (stderr only) | WP |
|---|---|---|
| D-DIAG-01 | git < 2.31: `[WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.` (once per process; 02 G.4) | 18 |
| D-DIAG-02 | `install`/`uninstall`/`init-templatedir`/`doctor --fix` notes when replacing other managers' scripts: M1 `Replaced hook script from {owner} at {hook_path}` [+ ` (Python: {INSTALL_PYTHON})`], M1b, M2 `Removed {legacy_path}: it is a hook script from {owner}, not a user hook`, M3 ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp``, M12 (11 §A.5.4) — both brands | 33 |
| D-DIAG-03 | `run` hint, only when stderr is a TTY, `SNP_NO_HINTS` unset/empty and `core.hooksPath` unset: ``hint: {hook_path} runs {owner}, not snp; run `{prog} install` to switch`` (11 §A.14) | 32 |
| D-DIAG-04 | stash notices: the next-run recovery notice (§4.6), `snp: waiting for another snp run in this worktree…`, `snp: restoring stashed changes, please wait`, `snp: git needed the terminal during the restore and was stopped` | 23 |
| D-DIAG-05 | store notices: the foreign-owner warning (ADR-044), `Removed snp 1.x cache data from {store}.` (gc) | 22 |

### 5.9 TIERB — Python-internal bytes

| ID | pre-commit 4.6.2 | snp | WP |
|---|---|---|---|
| D-MISC-04 | regex `FutureWarning` lines: `…/cfgv.py:386: FutureWarning: Possible nested set at position 1` + `  re.compile(v)` on stderr | one stderr line per distinct (pattern, warning): `[WARNING] regex {pattern!r}: {msg} (Python FutureWarning)`; `SNP_PYTHON_WARNINGS=0` silences | 21 |
| D-MISC-05 | invalid pygrep entry: one full traceback per xargs partition | one line in Python's form (`re.error: … at position N`), exit 1 | 26 |
| D-MISC-06 | regex corner cases (01 Addendum §A.6.13 D1–D6): IGNORECASE backreferences across `K`/`k`, `ſ`/`s`, `ς`/`σ`; code points unassigned in UCD 15.0 treated as UCD 16; `\N{…}` without the `py-re-names` feature; `(?L)` under a single-byte non-UTF-8 locale; patterns too large for the engines (`Unsupported`, reported as the validation error plus a stderr hint) | as stated (corpus frequency 0) | 13 |
| D-MISC-07 | in-hook hazmat exec failure: full Python traceback | only the final traceback line (`pre_commit.parse_shebang.ExecutableNotFoundError: Executable `x` not found`) | 26 |
| D-MISC-08 | Python tracebacks in the crash log body | error source chain / Rust backtrace (crash-log body is Tier B) | 33 |

## 6. Not divergences

### 6.1 Determinism notes (N-*)

* **N-1** Merge-conflict file sets: pre-commit iterates a Python `set` (string hashing is randomised per process, so the
  order is random for `require_serial` hooks); snp uses first-seen order. Every snp output is one of pre-commit's
  possible outputs; the difftest compares these streams unordered.
* **N-2** Parallel buckets (`priority` extension) print in config order (head-of-line), identical to sequential output.

### 6.2 Exact optimisations (no observable change; each has an equivalence test)

DiffOracle stat path (allow-listed filesystems only) and baseline elision on the warm path (`SNP_DIFF_MODE=git`,
`SNP_VERIFY_BASELINE=1` cross-check), tiered lazy tagging and the git-mode fast path, P17 classifier skip, speculative
config load, regex fast kinds and lazy engine compilation, spawn-free health checks, parallel clones (background
prefetch whose every failure is discarded and redone in the foreground exactly as pre-commit) and installs with
ordered transcript replay, local deps-variant clones, git fan-out, cached `git --version` and probe facts (native
binaries only), persistent rustup and shared toolchain directories, a job pool sized by the cgroup-aware CPU count and
the fd budget (partition contents always use pre-commit's `jobs`). Message-parity markers (`keys-v1`) make cold and
warm transcripts identical with the fast path (ADR-032).

### 6.3 Parity decisions that look like divergences but are not

* try-repo uses a throwaway store, as pre-commit does, so its `Initializing`/`Installing` lines print on every run.
* Native hooks and pygrep parse `args + files` with argparse emulation per partition, so file names shaped like flags
  behave exactly as in Python (ADR-072).
* Windows: environment names are upper-cased exactly where pre-commit passes an `env=` copy, original spelling
  elsewhere (14 P-83/D-WIN-05, ADR-074); VT processing is re-enabled before coloured writes after inherited-stdio
  children (no byte change, P-52).
* Hook ids containing `:` are plain ids (no selector syntax in v1).
* macOS: an executable that is neither a `#!` script nor a Mach-O binary fails with `[Errno 8] Exec format error`, as
  under CPython's `execve`; Apple's `posix_spawnp` would run it with `/bin/sh` (ADR-086).

## 7. Config extension keys (lefthook-inspired; all optional)

Absent keys ⇒ exact pre-commit behaviour. Under pre-commit these keys only produce `[WARNING] Unexpected key(s) present
…` lines (hook keys at run time, root keys at load) and change nothing else, so a config stays usable by both tools.
`SNP_COMPAT_STRICT=1` makes snp treat them exactly as pre-commit does. Keys use prek's names where prek defined them.
Unknown keys, including `x-*`, keep pre-commit's warnings in snp.

### 7.1 Hook-level keys

| Key | Type | Default | Validation | Semantics |
|---|---|---|---|---|
| `priority` | u32 or alias string | implicit: index in the flattened config | alias must exist in root `priorities` | lower runs first; equal values form a bucket whose hooks run concurrently on the global job pool; a bucket is a barrier; output stays in config order |
| `env` | map str → str | `{}` | names non-empty, no `=`/NUL | literal values (no `$VAR` expansion), applied after the language environment (user wins); also applies to `skip`/`only` `run:` conditions |
| `groups` | list of str | `[]` | non-empty, no whitespace, not starting with `@` (reserved for selectors such as `@ungrouped`) | prek semantics (ADR-080): `snp run --group G` (repeatable, OR), `--require-group G` (repeatable, AND), `--no-group G` (exclusion wins over both), `@ungrouped` = hooks without groups; `SNP_SKIP_GROUPS=a,b` and overlay `skip_groups` exclude |
| `stage_fixed` | `true` (= `verify`) \| `verify` \| `trust` \| `false` | `false` | warning if the hook has no `pre-commit` stage | pre-commit stage with a stash only: files of the hook's staged inputs modified by it are staged after an apply-check; `verify` re-runs a hook that exited non-zero after fixing; status `Passed` + `- files were modified by this hook (stage_fixed: staged after the last hook)` |
| `skip` / `only` | condition or list | — | compiled at load (globs, durations) | conditions: `merge`, `rebase`, `merge-commit`, `cherry-pick`, `revert`, `bisect`, `{ref: <glob>}` (short branch name; `*` crosses `/`), `{env: NAME}` (set and non-empty), `{ci: true\|false}`, `{run: <command>}` (`sh -c`, cwd repo root, 30 s, cached per run); keys of one map AND, list items OR; a skipped hook prints pre-commit's yellow `Skipped` line |
| `timeout` | Go duration (`30s`, `1m30s`, `500ms`) | none | > 0, ≤ 24 h | whole-hook wall clock; the hook runs in its own process group; TERM, 2 s, KILL; `Failed` + `- hook timed out after <d>`; counts for `fail_fast` |
| `fail_text` | str | — | single line recommended | `- <fail_text>` appended to the failure block |
| `root` | relative directory | — | normalised, no `..`, not absolute | cwd = `<repo>/<root>`; only files under it (boundary-aware) are passed, relative to it; missing directory ⇒ `Failed` + `- root directory '<r>' does not exist` |
| `use_stdin` | bool | `false` | — | the hook receives a copy of the git hook's stdin (pre-push ref lines); stdin over 64 MiB fails closed (D-SEC-12) |
| `fail_on_changes` | `always` \| `never` \| `ci` \| `non-ci` (+ `true`/`false`) | root value | — | `never`: modifications do not fail the hook; `ci`: only when `CI` is non-empty and not `0`/`false` |
| `interactive`, `follow` | bool | `false` | reserved | rejected with "not available in this version" until v1.1 |

### 7.2 Root-level keys

| Key | Type | Default | Semantics |
|---|---|---|---|
| `priorities` | map name → u32 | `{}` | aliases usable in `priority` |
| `minimum_snp_version` | semver string | — | `snp {v} is required but snp {SNP_VERSION} is installed.` exit 1 |
| `fail_on_changes` | as above | `always` | default for hooks |
| `skip` / `only` | conditions | — | whole-run conditions, evaluated once |
| `allow_disable` | bool | `true` | `false` ⇒ `SNP=0` is ignored (one stderr note) |
| `fast_path` | `off` \| `exact` \| `auto` | `auto` | pre-commit-hooks fast path mode (ADR-031) |
| `fast_path_exclude` | list of hook ids | `[]` | never substituted |
| `fast_path_coalesce` | bool | `false` | native hooks run once instead of once per xargs partition (output may then differ from pre-commit in partition footers) |
| `output` | `default` \| `summary` \| `quiet` | `default` | `summary`: footer with wall time, per-hook durations, counts; `quiet`: failure blocks only; `SNP_OUTPUT` overrides |

### 7.3 `repo: builtin`

`- repo: builtin` without `rev`; hooks as in `repo: local` but `entry` and `additional_dependencies` forbidden and
`language` absent or `system`; ids and defaults of pre-commit-hooks v6.0.0 with upstream bugs fixed (09 §7.2) and extra
`--check` flags. Not portable to pre-commit; `snp migrate builtin` / `snp migrate upstream` convert both ways.

### 7.4 Local overlay

`<config dir>/.pre-commit-config.local.yaml` (or `.yml`), or `SNP_LOCAL_CONFIG=<path>` (empty/`0` disables);
`--no-local-config` disables. Ignored (stderr note) when tracked by git or when `CI` is truthy, unless named by
`SNP_LOCAL_CONFIG`. Schema:

```yaml
hooks:                         # keyed by hook alias, else id; an id matching several hooks applies to all
  pytest:
    skip: true                 # or any condition
    entry: "docker compose run --rm app {entry}"   # {entry} = merged base entry
    args: ["-x"]               # replaces
    env: {PYTHONWARNINGS: ignore}                  # merged, overlay wins
    priority: 10
    stage_fixed: true          # any hook-level key except id/alias/repo
skip_groups: [slow]
fail_on_changes: never
repos: []                      # appended after the main config's repos
```

Unknown ids warn on stderr (`snp: local config overrides unknown hook '<id>'`); an active overlay is announced once on
stderr and marked in `snp config dump`. Hook entries are applied after the manifest ∪ config merge, so `{entry}` is the
merged entry of remote and builtin hooks too, and an overlay that sets `language`, `entry` or
`additional_dependencies` delegates a fast-path hook and changes its install key exactly like the same key in the
config (ARCHITECTURE §8.4).

## 8. CLI extensions and environment variables

### 8.1 snp-brand commands and flags

`snp run` (and `try-repo`): several hook ids, `-j/--jobs N`, `--group G`, `--require-group G`, `--no-group G`,
`--no-fast-path`, `--stdin-files[=nul|lines]`, `--summary`, `--no-local-config`. `snp install`/`uninstall
--git-config-hooks` (git ≥ 2.54; `--config-hooks` was rejected: it would make `--conf` ambiguous, ADR-057).
`snp install-hooks --with-fallback`. `snp validate-config --strict --portable`. Commands: `list [--json] [--hook-stage
S] [--group G] [--require-group G] [--no-group G] [-c C]`, `config dump [--json] [--no-local-config]`, `config schema`, `cache {dir,size,gc,clean}`,
`migrate {lefthook,builtin,upstream}`, `doctor [--fix] [--fast-path] [--stash]`, `self alias {install,uninstall,status}`,
`self check`, `completions {bash,zsh,fish,powershell}`. All long options pass the prefix-stability test.

### 8.2 Environment variables

pre-commit's, unchanged: `SKIP`, `PRE_COMMIT_COLOR`, `TERM`, `PRE_COMMIT_NO_CONCURRENCY` (presence), `TRAVIS`
(presence), `PRE_COMMIT_ALLOW_NO_CONFIG`, `PRE_COMMIT_RUNNING_LEGACY`, `_PRE_COMMIT_SKIP_POST_CHECKOUT`,
`PRE_COMMIT_USE_MAMBA`, `PRE_COMMIT_USE_MICROMAMBA`, `PATH`, `PATHEXT`, `HOME`/`USERPROFILE`, `XDG_CACHE_HOME`,
`COLUMNS`. `PRE_COMMIT_HOME` is read only to refuse pre-commit's store. `CI` is read only by extensions.

snp-only: `SNP_HOME`, `SNP` (`0`/`false` kill switch), `SNP_BIN` (hook script), `SNP_RUNNING_LEGACY`,
`SNP_ALLOW_NO_CONFIG`, `SNP_JOBS`, `SNP_SKIP_GROUPS`, `SNP_OUTPUT`, `SNP_NO_FAST_PATH`, `SNP_COMPAT_STRICT`,
`SNP_LOCAL_CONFIG`, `SNP_NO_HINTS`, `SNP_LOG`, `SNP_LOG_FILE`, `SNP_OFFLINE`, `SNP_UV`, `SNP_PYTHON_DOWNLOADS`,
`SNP_NODE_MIRROR`, `SNP_ALLOW_INSECURE_MIRROR`, `SNP_DOCKER_ALWAYS_BUILD`, `SNP_PREFER_SYSTEM_TOOLCHAINS`,
`SNP_ALLOW_OLD_GIT`, `SNP_STASH_LOCK_TIMEOUT`, `SNP_STASH_VERIFY`, `SNP_KILL_GRACE`, `SNP_DIFF_MODE`,
`SNP_VERIFY_BASELINE`, `SNP_OUTPUT_BUDGET`, `SNP_PYTHON_WARNINGS`, `SNP_PYTHON_INSTALLER` (`auto`|`uv`|`pip`, ADR-073),
`SNP_ALLOW_FOREIGN_STORE` (D-SEC-13), `SNP_PREFETCH` (`0` disables background clone prefetch), `SNP_NODE_SHASUMS_URL` /
`SNP_GO_JSON_URL` (https checksum manifests for mirrors). Duration variables are parsed with an upper bound; an
invalid value is a FatalError naming the variable. Test builds only (`--features testing`, never in releases):
`SNP_FAULT`, `SNP_TEST_SPAWN_LOG`.

Exported to hooks: exactly pre-commit's set (02 §6.1) plus `_SNP_STASH_LOCK` while a stash is held (D-EXT-04);
`SNP_RUNNING_LEGACY=1` and `PRE_COMMIT_RUNNING_LEGACY=1` for legacy hooks. Deliberately ignored: `NO_COLOR`,
`FORCE_COLOR`, `CLICOLOR_FORCE`, `PYTHON_CPU_COUNT` (pre-commit ignores it too on Linux; on macOS and Windows under
CPython ≥ 3.13 it does not: D-PLAT-10), `PYTHONWARNINGS` (passed through to hooks unchanged), `RUST_LOG`, `PREK_*`.

## 9. Migration notes (for the user documentation)

* Installing the snp binary changes nothing for git, CI or scripts until the user runs `snp install` (or `snp doctor
  --fix`), which overwrites pre-commit's, prek's and snp 1.x hook scripts in place (no `.legacy` chains) and reports
  each replacement on stderr. The embedded executable is a stable PATH entry (never a versioned install path), so
  upgrades of Homebrew/mise/cargo-binstall installs keep commits working. Rollback: `pre-commit install` (Python
  pre-commit recognises snp's script) and `snp self alias uninstall`.
* Running as root (CI containers) against a cache volume owned by another user needs `SNP_ALLOW_FOREIGN_STORE=1`
  (D-SEC-13).
* git older than 2.19 can no longer stash unstaged changes (D-DATA-17); upgrade git (2.31+ recommended).
* CI caches keyed on `~/.cache/pre-commit` must cache `~/.cache/snp` (or `$SNP_HOME`) and change their cache key;
  `PRE_COMMIT_HOME` no longer moves the store.
* Configs need no change. Extension keys keep configs usable by pre-commit (with warnings); `repo: builtin` does not.
* `entry: pre-commit hazmat …` works without a `pre-commit` executable; other `pre-commit …` entries need the alias
  (`snp self alias install`) exactly as they need Python pre-commit today.
* `snp migrate lefthook` known limits (each is reported or refused, never silent):
  * glob patterns follow gobwas/glob v0.2.3 exactly (one range or a literal list per `[…]`, a trailing `\` dropped, an
    unclosed `{` closed at the end; checked against vectors from the real library), except where gobwas itself is
    inconsistent: a brace group whose alternatives are all empty (`{}`, `{,}`) is refused (the job becomes a `fail`
    hook, `lossy`), and gobwas's byte-length bugs on multi-byte characters under `?` / `[…]` are not reproduced
    (snp matches characters);
  * lefthook lowercases pattern and path (`strings.ToLower`); the converted `files` regex uses `(?i:…)` and the shell
    filter `grep -i`, which also fold `ſ`↔`s`, `ı`↔`i`, `µ`↔`μ` and a few Greek letter variants. Paths containing
    those letters can match a converted `glob` that lefthook's would not;
  * `--mode portable` reports as `lossy` (so `--strict` exits 2) every behaviour change it cannot express: lefthook's
    default `fail_on_changes: never`, `fail_text`, `tags`, `skip`/`only`, `stage_fixed`, `timeout`, `use_stdin`;
  * `snp migrate builtin` → `snp migrate upstream` restores the original bytes when `rev` directly follows `repo`;
    a `rev` written before `repo` or after `hooks` comes back right after `repo` (same parsed config). A `repo`/`rev`
    whose value is on the line after its key, or that carries an anchor or tag or is an alias, stays upstream;
  * `glob_matcher: doublestar` patterns follow bmatcuk/doublestar v4.10.2 (checked against vectors from the real
    library): negated classes match `/`, braces expand textually, a trailing `/**`, `/**/` or `*` also matches the
    name without it, `***` is two plain stars. doublestar keeps a single `*` backtrack, so with a negated class that
    matches `/` after a star, or a plain `x**/` after a star in the same segment, it can miss a match the regex finds
    (or the reverse); such patterns are not otherwise detected;
  * shell-computed file lists (`{files}`, and `{all_files}` / `{staged_files}` outside their stage) run the command
    with `core.quotePath=false` (unless `GIT_CONFIG_COUNT` is already set) and trim each line like lefthook; names git
    still quotes (with `"`, `\`, a tab or other control characters) are skipped where lefthook unquotes them;
  * env values are expanded with Go's `os.ExpandEnv` grammar: `$NAME` / `${NAME}`; special parameters (`$1`, `$@`,
    `$$`, …) and any other `${…}` form (`${X:-d}`, `${#X}`) are empty, as lefthook looks up a variable of that literal
    name; values are expanded in the hook's environment in order, so an earlier `env` entry is visible to a later one
    (lefthook expands each against its own environment);
  * extends globs read at most 16384 directory entries and return at most 4096 paths per load (lefthook has no
    bound; symlink loops such as `a -> .` make `*/*/…` patterns exponential): past that the load fails (exit 1).
    A config or extends path that is not a regular file (a FIFO, a device) is refused instead of blocking.
