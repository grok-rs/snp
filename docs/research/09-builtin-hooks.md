# 09 — Native Rust "fast path" for popular hooks (`pre-commit/pre-commit-hooks`) and `repo: builtin`

Status: research spec for snp engineers. Self-contained: implementers should not need to re-read the Python sources.

**Sources (all treated as data):**

| tag | what | where |
|---|---|---|
| `pch/` | pre-commit-hooks, main @ `ccfd994` (2026-10-06): v6.0.0 plus unreleased commits | `refs/pre-commit-hooks/` |
| `pch.git` | full history of pre-commit-hooks (bare clone), tags v0.9.2 … v6.0.0, used for version epochs | `tmp-builtin/pch.git` |
| `pc/` | pre-commit 4.6.2 @ `c429238` (runner semantics) | `refs/pre-commit/` |
| prek | j178/prek docs and code, read for lessons only (no code copied) | `refs/prek/` |

**Observation rig:**
- pre-commit 4.1.0 from `~/.local/bin`, running the cached pre-commit-hooks **v6.0.0** env: CPython **3.12.3**, **ruamel.yaml 0.19.1** (pure Python; no `ruamel.yaml.clib` installed, because 0.19 made clib an extra), **expat 2.6.1**.
- Some checks also ran under CPython **3.13.14**, using the cached v5.0.0 env with `language_version: python3.13`.
- Every hook ran through `pre-commit run <id> --files …` in throwaway repos, with `PRE_COMMIT_NO_CONCURRENCY=1` unless stated.
- The harness is at `tmp-builtin/harness/probe.py` and the cases are in `tmp-builtin/cases/*.py`.
- ✓ = behaviour verified this way. Everything else comes from reading the source.

**Path conventions:** `pch/pre_commit_hooks/x.py:N` means pre-commit-hooks source line N on main (identical to v6.0.0 unless noted). `pc/pre_commit/x.py:N` means pre-commit 4.6.2. `02 §n` refers to `research/02-run-pipeline.md`.

**Classification legend:**
- **NATIVE**: snp can reproduce, byte for byte, the exit code, the combined output and the resulting file bytes for every input on which the Python hook does not crash. Crash paths (uncaught Python exceptions) map to exit 1 with an snp message, and processing of that invocation stops (see §2.6).
- **NATIVE-WITH-CAVEATS**: pass/fail and file effects match for the overwhelming majority of inputs. Documented differences are limited to (a) third-party parser diagnostics (text or positions) and/or (b) exotic inputs whose acceptance depends on the Python or library version the user happened to install.
- **DELEGATE**: keep running the upstream Python hook (clone + venv), as pre-commit does.

---

## 0. TL;DR — the facts and decisions that matter most

1. **The repo has 34 hook ids** (v6.0.0 manifest). Their classes:
   - **23 NATIVE**: everything byte-oriented or git-oriented.
   - **3 NATIVE-WITH-CAVEATS**: `check-yaml` (ruamel), `check-toml` (tomllib), `check-xml` (expat).
   - **1 NATIVE-WITH-CAVEATS for INI parsing**: `detect-aws-credentials`.
   - **5 DELEGATE**: they need CPython's own `ast`/`tokenize`: `check-ast`, `check-builtin-literals`, `check-docstring-first`, `debug-statements`, `double-quote-string-fixer`.
   - **2 already Python-free**: `check-illegal-windows-names` and `forbid-submodules` use `language: fail`. They only need the manifest, not a venv.
2. **The ten hooks to build first (P0)** cover the vast majority of real-world configs: `trailing-whitespace`, `end-of-file-fixer`, `check-yaml`, `check-added-large-files`, `check-merge-conflict`, `mixed-line-ending`, `check-json`, `detect-private-key`, `no-commit-to-branch`, `fix-byte-order-marker`. For these the fast path removes the clone, the virtualenv and pip install, and the per-partition Python startup (about 30–60 ms per process).
3. **Parity is byte-level and must include pre-commit's invocation model** (§2):
   - files arrive **deterministically shuffled**;
   - they are split into **xargs partitions** of `max(4, ceil(n/jobs))`;
   - each partition is a separate process, so stateful hooks print per-partition footers and duplicates. ✓ Example: `check-case-conflict` printed the same conflict 9 times.
   - **Decision:** by default, native hooks run once **per partition**, with the same shuffle and partitioning as pre-commit. An opt-in "coalesce" mode exists.
4. **Python I/O quirks are part of the observable output:**
   - `print()` vs `sys.stdout.buffer.write` ordering (pretty-format-json diffs come *before* messages when not on a tty);
   - `out.strip()` is applied by pre-commit;
   - stdout and stderr are merged;
   - argparse errors (exit 2, usage wrapped at 78 columns);
   - **allow_abbrev** prefix matching ✓ (`--mark=md` works);
   - an uncaught exception **aborts the remaining files in that partition**.
5. **Several upstream quirks change pass/fail and must be reproduced in substitution mode ("bug-compat").** Examples, all ✓:
   - a file named `foo.` is treated as markdown by trailing-whitespace;
   - a file containing exactly `\n` is rewritten forever by sort-simple-yaml;
   - non-ASCII staged paths are invisible to `check-added-large-files` and `check-case-conflict` because git's `core.quotePath` quoting is applied to non-`-z` output;
   - cherry-pick conflicts are not detected by `check-merge-conflict`;
   - `--maxkb=-1` flags empty files;
   - mixed-line-ending appends a newline to the last line when it fixes.
6. **Detection.** The URL is normalised to `github.com/pre-commit/pre-commit-hooks` (https, ssh, scp-like, `.git` and trailing slash variants all match). Then there are two tiers:
   - **Tier A:** a known tag, or the full commit SHA of a known tag. The embedded per-tag manifest and behaviour profile are used, so **no clone and no venv** when every selected hook from the repo is native.
   - **Tier B:** any other rev, or any repo URL such as forks and mirrors. snp clones as pre-commit does, but substitutes only when the git **blob ids** of the hook module (and its imports) are in snp's verified table. Otherwise it delegates.
7. **Version awareness.** Behaviour changed across tags for 17 hooks; §5 gives the per-hook epochs.
   - Supported floor: **v3.0.0** (2020-05). Older revs delegate.
   - Examples: `check-merge-conflict` message format changed in v4.2.0; the `no-commit-to-branch` default gained `main` in v4.0.0; `requirements-txt-fixer` dedupe arrived in v4.6.0 and `pkg_resources` handling in v5.0.0; `check-toml` switched from the `toml` package to `tomllib` in v4.3.0 (delegate before that).
8. **Opt-outs**, from highest precedence:
   - `snp run --no-fast-path` or `SNP_NO_FAST_PATH=1`;
   - snp settings `fast_path = "off" | "exact" | "auto"` (default `auto`) plus `fast_path_exclude = [ids]`;
   - per hook, an explicit `language:` (any value), an `entry:` override, or non-empty `additional_dependencies` forces DELEGATE. The `language:` rule is the same convention prek uses, and all three keys are valid pre-commit keys, so configs stay portable.
9. **`repo: builtin`** (same spelling as prek):
   - snp's own hook implementations: no `rev`, no network;
   - same ids, args and defaults as pre-commit-hooks;
   - upstream bugs fixed (documented per hook), plus extras such as `--check`;
   - not portable to pre-commit. `snp migrate builtin` and `snp migrate upstream` convert configs both ways.
10. **Safety and performance rules:**
   - `#![forbid(unsafe_code)]`, no panics, no mmap;
   - in-place `O_TRUNC` writes, the same as Python. No temp-file+rename: it breaks hardlinks, permissions and cross-device moves (prek hit this bug);
   - tail-only reads for end-of-file-fixer, metadata-only checks for large files;
   - per-file parallelism with ordered output;
   - git facts are cached per run and computed **with the same git CLI invocations** (quoting, renames and pathspec semantics are part of the behaviour);
   - native fixers report modified paths directly instead of running `git diff` before and after, except when eol/filter attributes are active (§6.7).

---

## 1. Inventory and classification (v6.0.0 manifest, `pch/.pre-commit-hooks.yaml`)

Common manifest defaults, unless a key is listed in the table:
- `language: python`
- `types: [file]` (pre-commit's default when absent)
- `stages`: all
- `pass_filenames: true`, `always_run: false`, `require_serial: false`

| # | id | entry (= argparse `prog`) | manifest filters / keys (v6.0.0, line) | class | prio |
|---|---|---|---|---|---|
| 1 | `check-added-large-files` | `check-added-large-files` | `stages: [pre-commit, pre-push, manual]`, `minimum_pre_commit_version: 3.2.0` (:1-7) | NATIVE | P0 |
| 2 | `check-ast` | `check-ast` | `types: [python]` (:8-13) | DELEGATE | – |
| 3 | `check-byte-order-marker` | `pre-commit-hooks-removed check-byte-order-marker fix-byte-order-marker https://github.com/pre-commit/pre-commit-hooks` | `types: [text]` (:14-19). Name `check-byte-order-marker (removed)` | NATIVE (stub) | P1 |
| 4 | `check-builtin-literals` | `check-builtin-literals` | `types: [python]` (:20-25) | DELEGATE | – |
| 5 | `check-case-conflict` | `check-case-conflict` | (:26-30) | NATIVE (bug-compat) | P1 |
| 6 | `check-docstring-first` | `check-docstring-first` | `types: [python]` (:31-36). Name gets `(deprecated)` on main | DELEGATE | – |
| 7 | `check-executables-have-shebangs` | same | `types: [text, executable]`, stages as #1, min 3.2.0 (:37-44) | NATIVE | P0/P1 |
| 8 | `check-illegal-windows-names` | `Illegal Windows filenames detected` | `language: fail`, `files: '(?i)((^\|/)(CON\|PRN\|AUX\|NUL\|COM[\d¹²³]\|LPT[\d¹²³])(\.\|/\|$)\|[<>:\"\\\|?*\x00-\x1F]\|/[^/]*[\.\s]/\|[^/]*[\.\s]$)'` (:45-49) | Python-free (fail) | P0 (manifest only) |
| 9 | `check-json` | `check-json` | `types: [json]` (:50-55) | NATIVE | P0 |
| 10 | `check-shebang-scripts-are-executable` | same | `types: [text]`, stages as #1, min 3.2.0 (:56-63) | NATIVE | P1 |
| 11 | `pretty-format-json` | `pretty-format-json` | `types: [json]` (:64-69) | NATIVE | P1 |
| 12 | `check-merge-conflict` | `check-merge-conflict` | `types: [text]` (:70-75) | NATIVE | P0 |
| 13 | `check-symlinks` | `check-symlinks` | `types: [symlink]` (:76-81) | NATIVE | P1 |
| 14 | `check-toml` | `check-toml` | `types: [toml]` (:82-87) | NATIVE-WITH-CAVEATS | P1 |
| 15 | `check-vcs-permalinks` | `check-vcs-permalinks` | `types: [text]` (:88-93) | NATIVE | P2 |
| 16 | `check-xml` | `check-xml` | `types: [xml]` (:94-99) | NATIVE-WITH-CAVEATS | P2 |
| 17 | `check-yaml` | `check-yaml` | `types: [yaml]` (:100-105) | NATIVE-WITH-CAVEATS | P0 |
| 18 | `debug-statements` | `debug-statement-hook` | `types: [python]` (:106-111) | DELEGATE | – |
| 19 | `destroyed-symlinks` | `destroyed-symlinks` | `types: [file]`, stages as #1 (no min version) (:112-118) | NATIVE | P1 |
| 20 | `detect-aws-credentials` | `detect-aws-credentials` | `types: [text]` (:119-124) | NATIVE-WITH-CAVEATS | P2 |
| 21 | `detect-private-key` | `detect-private-key` | `types: [text]` (:125-130) | NATIVE | P0 |
| 22 | `double-quote-string-fixer` | `double-quote-string-fixer` | `types: [python]` (:131-136) | DELEGATE | – |
| 23 | `end-of-file-fixer` | `end-of-file-fixer` | `types: [text]`, stages as #1, min 3.2.0 (:137-144) | NATIVE | P0 |
| 24 | `file-contents-sorter` | `file-contents-sorter` | `files: '^$'` (:145-150) | NATIVE | P1 |
| 25 | `fix-byte-order-marker` | `fix-byte-order-marker` | `types: [text]` (:151-156) | NATIVE | P0 |
| 26 | `fix-encoding-pragma` | `pre-commit-hooks-removed fix-encoding-pragma pyupgrade https://github.com/asottile/pyupgrade` | `types: [python]` (:157-162) | NATIVE (stub); ≤v5.0.0 real hook = NATIVE P3 | P1 |
| 27 | `forbid-new-submodules` | `forbid-new-submodules` | `types: [directory]` (:163-168) | NATIVE | P2 |
| 28 | `forbid-submodules` | `submodules are not allowed in this repository:` | `language: fail`, `types: [directory]` (:169-174) | Python-free (fail) | P1 (manifest only) |
| 29 | `mixed-line-ending` | `mixed-line-ending` | `types: [text]` (:175-181). `stages: [pre-commit, pre-push, manual]` exists **only on main** (commit e6efff2, unreleased) | NATIVE | P0 |
| 30 | `name-tests-test` | `name-tests-test` | `files: (^\|/)tests/.+\.py$` (:182-187) | NATIVE | P2 |
| 31 | `no-commit-to-branch` | `no-commit-to-branch` | `pass_filenames: false`, `always_run: true` (:188-193) | NATIVE | P0 |
| 32 | `requirements-txt-fixer` | `requirements-txt-fixer` | `files: (requirements\|constraints).*\.txt$` (:194-199) | NATIVE | P1 |
| 33 | `sort-simple-yaml` | `sort-simple-yaml` | `files: '^$'` (:200-205) | NATIVE | P2 |
| 34 | `trailing-whitespace` | `trailing-whitespace-fixer` | `types: [text]`, stages as #1, min 3.2.0 (:206-213) | NATIVE | P0 |

Historical ids (relevant only to old pins):
- `autopep8-wrapper`, `flake8`, `pyflakes`: removed stubs in v3.x; dropped in v4.0.0.
- `check-byte-order-marker`: a real check up to v5.0.0.
- `fix-encoding-pragma`: a real fixer up to v5.0.0.

§4.H covers them. Hook display `name`s differ per tag (§5.3), so snp needs the per-tag manifest to print the status line exactly.

**Package metadata** (`pch/setup.cfg`):
- `version = 6.0.0`.
- `install_requires = ruamel.yaml>=0.15` (unpinned) and `tomli>=1.1.0; python_version<"3.11"`.
- `python_requires = >=3.9` at v6.0.0; `>=3.10` on main.
- The 31 console scripts (`[options.entry_points]`, `setup.cfg:30-62`) map each entry name to `pre_commit_hooks.<module>:main`. Examples: `debug-statement-hook → debug_statement_hook`, `double-quote-string-fixer → string_fixer`, `name-tests-test → tests_should_end_in_test`, `trailing-whitespace-fixer → trailing_whitespace_fixer`, `pre-commit-hooks-removed → removed`.
- Every Python hook runs in a per-repo virtualenv created by pre-commit's `python` language, i.e. `pip install .` of the cloned repo. This env creation is exactly what the fast path avoids.

---

## 2. Runtime contract: how pre-commit runs these hooks (what "byte-identical" means)

The native implementation reproduces the **subprocess** pre-commit would have spawned. These are the observable properties.

### 2.1 argv

- `cmd = shlex.split(entry) + args`; filenames are appended last (`pc/pre_commit/lang_base.py:174-178`; 02 §11.3).
- argparse `prog` is the basename of the console script, i.e. the first token of `entry`. Note `trailing-whitespace-fixer` and `debug-statement-hook`. `prog` appears in usage and error text.
- pre-commit never inserts `--`. A filename that starts with `-` is parsed as an option by argparse: it is either an error or, worse, a matching option prefix. Reproduce this through the argparse emulator (§9.3).

### 2.2 File order and partitioning (details in 02 §11.6–11.7)

- With `require_serial: false` (every pre-commit-hooks hook), filenames are **deterministically shuffled**: `random.Random().seed(1542676187)` then `shuffle` (`pc/pre_commit/lang_base.py:22,146-153,166-170`). ✓ The output order of `Fixing …` lines follows the shuffled order. With `require_serial: true` (user override) there is no shuffle.
- `jobs` is 1 if `PRE_COMMIT_NO_CONCURRENCY` is set; else 2 if `TRAVIS` is set; else the CPU count (`sched_getaffinity`) (`lang_base.py:135-143`). It is 1 with `require_serial`.
- `max_args = max(4, ceil(n/jobs))`. The command-line length limit is `min(SC_ARG_MAX-2048-envsize, 2**17)` (min 4096) on POSIX, and 30720 UTF-16 units on Windows (`pc/pre_commit/xargs.py:49-117`). Partitions run on a thread pool of `min(#partitions, jobs)` threads. The **outputs are concatenated in partition order**. The **exit code is the partition exit code with the largest absolute value** (`xargs.py:175-184`).
- **Consequence ✓.** Every hook whose output is not purely per-file prints once **per partition**:
  - 33 files in total with a staged `A`/`a` conflict on a 22-CPU machine gave **9 partitions**, and `check-case-conflict` printed the two conflict lines 9 times.
  - 13 files with 12 bad links gave `check-vcs-permalinks` output with its two-line footer 4 times, interleaved.
  - The same applies to the footers of `forbid-new-submodules` and `destroyed-symlinks`, and to the git queries of `check-executables-have-shebangs`.
- **Decision for snp.** The native runner receives the same partitions the subprocess runner would have computed and runs the native hook once per partition, concatenating results in partition order.
  - Partitions may run in parallel; outputs are still concatenated in order.
  - For length-based splits, the hypothetical command length is computed from a synthetic venv path, `<store>/repoXXXXXXXX/py_env-python3.NN/bin/<entry>`. Divergence is only possible for multi-thousand-file runs.
  - `fast_path_coalesce = true` (snp setting) instead runs one invocation with all files. Footers are then printed once and conflicts reported once. This is cosmetic and deterministic across machines.

### 2.3 Output streams

- **Non-color mode** (CI, `--color=never`, stdout not a tty): stdout and stderr go to **one pipe** (`stderr=STDOUT`) (02 §11.8). Python, writing to a pipe:
  - `sys.stdout` is a block-buffered `TextIOWrapper` over a `BufferedWriter`;
  - `sys.stderr` is line-buffered (CPython ≥3.9).
  
  The effects:
  - Hooks that only use `print()` produce program order.
  - Hooks that only use stderr (`check-executables-have-shebangs`, `check-shebang-scripts-are-executable`) produce program order.
  - **`pretty-format-json` mixes `print()` with `sys.stdout.buffer.write()`.** The text layer holds pending text and only pushes it into the byte buffer:
    - when pending + new > 8192 bytes, existing pending text is flushed *before* adding;
    - when pending ≥ 8192 bytes after adding;
    - at exit, or before a traceback is printed.
    
    `print(x)` performs two writes: `x`, then `"\n"`. ✓ Result: all unified diffs come first, then all `Fixing file …` / `Input File … is not a valid JSON …` lines, unless they exceed 8 KiB. snp must emulate this two-layer writer for pretty-format-json in non-color mode.
  - `check-vcs-permalinks` mixes too, but calls `sys.stdout.flush()` before every `buffer.write`, so it stays in program order.
- **Color mode on POSIX** (`--color=always`, or `auto` with a tty): the hook runs on a **pty** with `ONLCR|OPOST` cleared, so `\n` is not translated (`pc/pre_commit/util.py:127-200`). Python sees a tty, so stdout is **line-buffered** and pretty-format-json output appears in program order.
  - With CPython ≥3.14, argparse usage/errors and ≥3.13 tracebacks may contain ANSI colour, unless `NO_COLOR`/`PYTHON_COLORS=0` is set.
  - snp native: emit program order in color mode, and **never** emit ANSI in hook output (documented deviation for CPython ≥3.14 envs).
  - On Windows there is no pty, so the pipe semantics apply.
- pre-commit displays `out.strip()` (ASCII-whitespace strip of the combined bytes) between blank lines, and writes the same to `log_file` (`pc/pre_commit/commands/run.py:230-233`). Leading and trailing newline differences are therefore invisible, but interior bytes are not. Native code still produces exact bytes.

### 2.4 Pass/fail and "files were modified"

- A hook fails if `retcode != 0` **or** `git diff --no-ext-diff --no-textconv --ignore-submodules` (unstaged diff) changed across the hook run (`run.py:203-213,274-279`).
- Every pre-commit-hooks fixer returns 1 when it writes. The `- files were modified by this hook` line, however, depends on the git diff:
  - modifying an **untracked or ignored** file (passed via `--files`) does not print it;
  - a CRLF→LF rewrite that is invisible after `core.autocrlf`/`text` normalisation does not print it.
  
  §6.7 explains how snp reproduces this without running two `git diff` commands per hook.

### 2.5 Working directory, paths and environment

- cwd is the repo root. Filenames are repo-relative with `/` separators; pre-commit normalises them on Windows.
- Hooks inherit the full environment plus `PRE_COMMIT=1`, `PRE_COMMIT_FROM_REF`/`PRE_COMMIT_TO_REF`, and so on (`run.py:375-417`).
- **The `GIT_*` variables set by git itself** (`GIT_INDEX_FILE` during `git commit <paths>`, `GIT_DIR`, `GIT_WORK_TREE`, …) **are inherited by the hook's `git` subprocesses.** Native git queries must use exactly the same environment (§9.6).

### 2.6 Python exit and termination semantics (emulate)

| situation | Python behaviour | snp native |
|---|---|---|
| `main()` returns int | exit code = value (0/1/2) | same |
| argparse error | usage plus `"{prog}: error: {msg}\n"` on **stderr**, exit **2**; no file processed | same text (Appendix A usage strings) |
| `-h`/`--help` in args | help on stdout, exit **0**, **no file processed** | same text |
| uncaught exception | earlier stdout is flushed, then a traceback on stderr, exit **1**. **Files later in the same invocation are not processed**; changes already made persist | exit 1; message `"{prog}: error: {short description}\n"`. Stop processing the rest of this partition (files are *not* fixed). The traceback is not reproduced (it contains machine-specific paths anyway) |
| `raise SystemExit(str)` (removed stubs) | the str plus `\n` on stderr, exit 1 | same |

### 2.7 I/O modes used by the hooks (affects encoding, newline and Windows behaviour)

| hook | read | write |
|---|---|---|
| trailing-whitespace, mixed-line-ending | `rb` (whole) | `wb` (truncate + write) |
| end-of-file-fixer | `rb+` (**needs write permission even when no change**; ✓ read-only file → PermissionError crash) | in-place append/truncate |
| file-contents-sorter, requirements-txt-fixer | `rb+` | seek(0) + write + truncate |
| fix-byte-order-marker | `rb` (3 bytes), then text `utf-8-sig`, `newline=''` | text `utf-8`, `newline=''` (no newline translation) |
| check-json, check-merge-conflict, detect-private-key, check-vcs-permalinks, detect-aws-credentials (scanned files), check-toml, check-xml, check-ast, … | `rb` | – |
| check-yaml | **text** `encoding='UTF-8'`, universal newlines (CRLF/CR → LF before ruamel) | – |
| pretty-format-json | **text** `UTF-8`, universal newlines | **text** `UTF-8`; on **Windows `\n`→`\r\n`** |
| sort-simple-yaml | **text, locale encoding** (`r+`), universal newlines | text: Windows `\r\n`; locale encoding (cp1252 on Windows unless UTF-8 mode; CPython ≥3.15 defaults to UTF-8 mode, PEP 686) |
| detect-aws-credentials (credential INI files) | `configparser.read()`: locale encoding | – |

### 2.8 Writes

- Python `open(path, 'wb')` is `O_WRONLY|O_CREAT|O_TRUNC` on the **same inode**: mode, owner, hardlinks and ACLs are preserved, the write is not atomic, and there is no fsync.
- `rb+` writes overwrite in place, then `truncate()`.
- snp: open the existing path for write without truncating, `write_all(new)`, then `set_len(new.len())`. This minimises the window in which the file is empty or short, and keeps the inode semantics.
- Never use temp-file + rename (§9.8). Follow symlinks exactly as Python does: plain `open`.

---

## 3. `pch/pre_commit_hooks/util.py` and the git command catalogue

`util.py`:
- `cmd_output(*cmd, retcode=0)` (:16-24): runs with stdout and stderr piped, `stdout.decode()` (**strict UTF-8**, so non-UTF-8 git output is a crash), and raises `CalledProcessError` if the return code ≠ `retcode` (unless `retcode=None`).
- `added_files()` (:11-13): `set(cmd_output('git','diff','--staged','--name-only','--diff-filter=A').splitlines())`.
- `zsplit(s)` (:27-32): `s.strip('\0')`, then split on `\0`; an empty string gives `[]`.

| git command (exact argv) | used by | `-z`? | parity-relevant semantics |
|---|---|---|---|
| `git diff --staged --name-only --diff-filter=A` | check-added-large-files, check-case-conflict | **no** | Paths with bytes ≥0x80 or control characters, `"` or `\` are **C-quoted** (`"caf\303\251.txt"`) when `core.quotePath=true` (default). They never match real filenames ✓. `str.splitlines()` also splits on `\x0b \x0c \x1c-\x1e \x85 \u2028 \u2029` (only reachable with `quotePath=false`). **Rename detection follows `diff.renames`** (default true): a staged rename is `R`, not `A`. Honours `GIT_INDEX_FILE`. Index vs HEAD; in an unborn repo, everything is `A`. Cacheable per run (depends only on HEAD and the index). |
| `git ls-files` | check-case-conflict | **no** | Index entries, C-quoted as above, `str.splitlines()`. |
| `git check-attr filter -z --stdin` (stdin = `'\0'.join(filenames)`, **utf-8**, `check=True`, stderr→DEVNULL) | check-added-large-files (≥v4.1.0) | yes | Output triplets `path\0attr\0value\0`. Remove the path when value == `lfs`. A non-zero exit is a crash. |
| `git config core.fileMode` (retcode ignored) | check-executables-have-shebangs (≥v4.3.0) | – | **The raw string** is compared with `'false'` after `.strip()`. `False`/`no`/`0` do **not** count; unset prints nothing. |
| `git ls-files -z --stage -- <paths…>` | check-executables-have-shebangs (fileMode false), check-shebang-scripts-are-executable | yes | Paths are **pathspecs** (glob magic: `[`, `*`, `?` match other files). Entries look like `"<mode> <sha> <stage>\t<path>"`. Unmerged paths yield up to 3 entries, so the message is printed up to 3 times. Untracked files are silently skipped. |
| `git rev-parse --git-dir` | check-merge-conflict | – | `.git` or an absolute worktree/submodule gitdir; `.rstrip()`. Outside git it is a crash. |
| `git symbolic-ref HEAD` | no-commit-to-branch | – | A non-zero exit (detached HEAD) means "not on branch". |
| `git status --porcelain=v2 -z -- <files…>` | destroyed-symlinks | yes | Only `1 ` records are parsed (pathspecs again). |
| `git cat-file -s <sha>` / `git cat-file -p <sha>` | destroyed-symlinks | – | – |
| `git diff --diff-filter=A --raw <--staged \| FROM...TO> -- <files…>` | forbid-new-submodules | **no** | Quoted paths are printed quoted. |

---
## 4. Per-hook specifications

Template per hook:
- **Manifest**
- **CLI**: argparse; the exact usage/help strings are in Appendix A
- **Algorithm**
- **Output**: verbatim; the stream is stdout unless stated
- **Exit codes**
- **Edge cases**: ✓ = verified
- **Epochs**: details in §5
- **Native notes**

Byte-set names used below:
- **ASCII-WS** = `b' \t\n\r\x0b\x0c'`. This is what Python `bytes.strip()`, `bytes.rstrip()`, `bytes.isspace()` and `bytes.split()` use. `\x1c`–`\x1f`, `\x85` and NBSP are **not** included ✓.
- **PY-STR-WS** = the characters for which `str.isspace()` is true: `\t \n \x0b \x0c \r \x1c \x1d \x1e \x1f ' ' \x85 \xa0 \u1680 \u2000-\u200a \u2028 \u2029 \u202f \u205f \u3000`. It is used by `str.rstrip()`/`str.strip()`.

### 4.A Whitespace / EOL fixers

#### `trailing-whitespace` — NATIVE (P0)

- **Manifest:** entry `trailing-whitespace-fixer`; `types: [text]`; stages `[pre-commit, pre-push, manual]`; min 3.2.0. Name `trim trailing whitespace`.
- **CLI** (`pch/pre_commit_hooks/trailing_whitespace_fixer.py:44-69`):

| option | action / default | notes |
|---|---|---|
| `--no-markdown-linebreak-ext` | store_true, `help=SUPPRESS` (hidden from usage/help) | If given, first prints `--no-markdown-linebreak-ext now does nothing!` ✓, then continues |
| `--markdown-linebreak-ext *\|EXT[,EXT,...]` | `append`, default `[]` | repeatable; comma lists allowed |
| `--chars CHARS` | default `None` | the set of characters to strip |
| `filenames` | `nargs='*'` | |

- **Argument post-processing** (:74-91), in this order:
  1. `md_args = args.markdown_linebreak_ext` (list of raw strings).
  2. If `''` is in `md_args` (an element exactly empty, e.g. `--markdown-linebreak-ext=`), call `parser.error('--markdown-linebreak-ext requires a non-empty argument')`, which gives exit 2 ✓.
  3. `all_markdown = '*' in md_args`. This compares **whole elements**: `--markdown-linebreak-ext=md,*` does *not* enable all-markdown.
  4. `md_exts = ['.' + x.lower().lstrip('.') for x in ','.join(md_args).split(',')]`. With no args this is `''.split(',') == ['']`, so **`md_exts == ['.']`** (see the edge case below).
  5. For each ext, if any of `.`, `/`, `\`, `:` occurs in `ext[1:]`, call `parser.error(f"bad --markdown-linebreak-ext extension {ext!r} (has . / \\ :)\n  (probably filename; use '--markdown-linebreak-ext=EXT')")` ✓. Exact stderr:
     ```
     usage: trailing-whitespace-fixer [-h]
                                      [--markdown-linebreak-ext *|EXT[,EXT,...]]
                                      [--chars CHARS]
                                      [filenames ...]
     trailing-whitespace-fixer: error: bad --markdown-linebreak-ext extension '.foo.md' (has . / \ :)
       (probably filename; use '--markdown-linebreak-ext=EXT')
     ```
     `{ext!r}` is a Python `repr` of str: single quotes, or double quotes if the value contains `'` and no `"` (Appendix B).
  6. `chars = None if args.chars is None else args.chars.encode()` (UTF-8). **The chars form a byte *set***, so a multibyte character contributes each of its bytes individually ✓ (`--chars xé` strips the bytes `x`, `\xc3` and `\xa9`). `--chars=` gives `b''`, which strips **nothing** ✓.
- **Per file** (:93-98):
  - `_, ext = os.path.splitext(filename.lower())`. This is Python `splitext` semantics (Appendix B): leading dots of the basename are not an extension, and `"foo."` gives `"."`.
  - `md = all_markdown or ext in md_exts`.
  - If the file was changed, print `Fixing {filename}` and set rc=1.
- **`_fix_file`** (:8-22):
  1. `lines = f.readlines()` in **binary**, i.e. split *after* each `\n`. The last element may lack `\n`. A lone `\r` is **not** a line break.
  2. Process every line (below).
  3. If `newlines != lines`, rewrite the whole file with `wb`.
- **`_process_line`** (:25-41):
  1. If the line ends with `\r\n`, set eol=`\r\n` and strip it. Else if it ends with `\n`, set eol=`\n`. Else eol=`''`.
  2. If `md` and the line is not all-ASCII-WS (`not line.isspace()`; an empty line counts as "not space") and the line ends with two spaces: return `line[:-2].rstrip(chars) + b'  ' + eol`. That is, all trailing stripped bytes before the final two spaces are removed and exactly two spaces are kept.
  3. Otherwise return `line.rstrip(chars) + eol`. With `chars=None`, `rstrip()` strips ASCII-WS, **including `\r`**. A lone CR at EOF or before LF is therefore stripped (`"g\r"` becomes `"g"` ✓; `"d\r\r\n"` becomes `"d\r\n"` ✓).
- **Verified examples ✓** (default args):

| input | output |
|---|---|
| `a \n` | `a\n` |
| `b\t\r\n` | `b\r\n` |
| `c \x0b\x0c\n` | `c\n` |
| `e \x1c\n` | unchanged |
| `f \xc2\xa0\n` | unchanged |
| `x  \n` in `c.md`, default args | `x\n` (markdown is **not** default) |
| `x  \n` in a file named **`d.`** | **unchanged**: ext `'.'` ∈ default `md_exts ['.']` |
| `q  \n` in `e.MD` | `q\n` |

- **Verified examples ✓** with `--markdown-linebreak-ext=md,.Txt`: exts are `.md` and `.txt`.

| input | output |
|---|---|
| `y   \n` | `y  \n` |
| `   \n` | `\n` |
| `z\t  \r\n` | `z  \r\n` |
| `f.txt` containing `x  ` | kept |

- **Output:** `Fixing {filename}` per changed file; `--no-markdown-linebreak-ext now does nothing!` first when that flag is present.
- **Exit:** 0 when nothing changed; 1 when anything changed; 2 on argparse error.
- **Edge cases:**
  - Empty file: unchanged.
  - Invalid UTF-8 and Latin-1 are fine; it is byte-level ✓.
  - CR-only files are one line, so only the end is stripped.
  - A read-only file that needs a change gives `PermissionError`, i.e. a crash (§2.6).
  - The file is opened only when a change is needed.
- **Epochs:** identical behaviour v2.4.0 → v6.0.0. Before v2.4.0 there is no `--chars`.
- **Native notes:**
  - One pass over `memchr(b'\n')` boundaries with copy-on-write: keep borrowing until the first changed line, then build the output `Vec`.
  - Pre-reserve `len`.
  - The byte-set for `chars` is a `[bool; 256]` table.
  - The markdown test is computed on the line without its eol.

#### `end-of-file-fixer` — NATIVE (P0)

- **Manifest:** `types: [text]`; stages as above; min 3.2.0; name `fix end of files`.
- **CLI:** `filenames nargs='*'` only.
- **Algorithm** `fix_file` (`end_of_file_fixer.py:9-49`) on a file opened `rb+`:
  1. `seek(-1, SEEK_END)`. On an empty file this raises `OSError`, so return 0.
  2. Read the last byte `c`. If `c ∉ {\n, \r}`: seek to the end, append `b'\n'`, return 1. A plain `\n` is appended **even in CRLF files** ✓ (`a\r\nb` becomes `a\r\nb\n`).
  3. While `c ∈ {\n, \r}`:
     - if `tell() == 1` (i.e. the byte just read is at offset 0): `seek(0)`, `truncate()`, return 1. **A file made only of `\r`/`\n` bytes becomes empty, including a file that is exactly `\n`** ✓;
     - otherwise `seek(-2, CUR)` and read 1 byte.
  4. `position = tell()` (just after the last non-EOL byte); `remaining = read()` (all trailing CR/LF bytes).
  5. For `seq` in `(b'\n', b'\r\n', b'\r')`, in this order:
     - if `remaining == seq`: return 0;
     - elif `remaining.startswith(seq)`: truncate at `position + len(seq)`, return 1.
- **Truth table ✓:**

| remaining tail | result | changed |
|---|---|---|
| `\n` / `\r\n` / `\r` | keep | no |
| `\n\n…` | `\n` | yes |
| `\n\r\n` | `\n` | yes |
| `\r\n\r\n` | `\r\n` | yes |
| `\r\n\n` | `\r\n` | yes |
| `\r\n\r` | `\r\n` | yes |
| `\r\r` | `\r` | yes |
| whole file `\n`, `\r\n`, `\r` or `\n\n\r\n` | empty | yes |
| `a\n \n` (space line) | unchanged (the space is not EOL) | no |

- **Output:** `Fixing {filename}`. **Exit:** 0 or 1.
- **Edge cases:**
  - Read-only file: crash *even when no change is needed*, because of `rb+` ✓ (`PermissionError: [Errno 13] Permission denied: 'ro.txt'`).
  - Windows: an open file locked by another process also crashes.
- **Epochs:** identical v2.1.0 → v6.0.0.
- **Native notes:**
  - Read the **tail only**: seek to `len - 4096`, scan backwards for the first non-`\r\n` byte, extend backwards in blocks if the whole block is EOL bytes.
  - Write by `set_len` (truncate) or by appending `\n` with `OpenOptions::append`.
  - O(1) I/O for normal files.
  - Bug-compat for read-only files is **not** reproduced. Natively, open read-only first and reopen for write only when needed. Documented crash-path deviation: the native hook passes where Python crashes.

#### `mixed-line-ending` — NATIVE (P0)

- **Manifest:** `types: [text]`; name `mixed line ending`. Since main (unreleased) it has `stages: [pre-commit, pre-push, manual]`.
- **CLI:** `-f/--fix {auto,no,cr,crlf,lf}` (default `auto`); `filenames *`. An invalid choice ✓ gives:
  `mixed-line-ending: error: argument -f/--fix: invalid choice: 'xx' (choose from 'auto', 'no', 'cr', 'crlf', 'lf')`.
  The quoting of choices is Python-version dependent: newer CPython prints them without quotes (§10 Q2).
- **Algorithm** (`mixed_line_ending.py:16-62`):
  1. `contents = read()` (binary). `lines = contents.splitlines(True)`. **`bytes.splitlines` splits only on `\r\n`, `\r` and `\n`** (not on `\x0b`, `\x0c`, `\x1c`, `\x85` ✓).
  2. For each line, count the first ending that matches in the order `CR`, `CRLF`, `LF` (`line.endswith`). A line ending `\r\n` does not end with `\r`, so it counts as CRLF. The last line without an ending is not counted.
  3. `mixed = number of ending kinds with count > 0 > 1`.
  4. `--fix=no`, or `auto` and not mixed: return `mixed`.
  5. `auto` (mixed): iterate `(CR, CRLF, LF)` with `>=`, so **ties prefer the later kind: LF > CRLF > CR**. Rewrite every line as `line.rstrip(b'\r\n') + ending`. **The last line without an ending also gets the ending appended** ✓ (`a\r\nb\nc` becomes `a\nb\nc\n`). Return 1.
  6. `cr`/`crlf`/`lf`: drop the target kind from the counts; if any other kind remains, rewrite the same way (including appending to an unterminated last line) and return 1; else return 0. This does not require "mixed": a pure-CRLF file with `--fix=lf` is converted ✓. A file with no line endings is untouched (`a\nb` with `lf` is unchanged ✓).
- **Verified ✓:**

| input | args | result |
|---|---|---|
| `a\rb\rc\r\nd\n` (CR 2, CRLF 1, LF 1) | auto | `a\rb\rc\rd\r` |
| `x\r\r\ny\n` | auto | lines `x\r`, `\r\n`, `y\n`: one each, so LF wins: `x\n\ny\n` |
| `a\rb\nc\r\n` | auto | LF |
| `a\r\nb\r\nc` | `--fix cr` | `a\rb\rc\r` |

- **Output:** with `--fix=no`: `{filename}: mixed line endings`; otherwise `{filename}: fixed mixed line endings`.
- **Exit:** 0 or 1; 2 on argparse error.
- **Edge cases:**
  - Empty file: 0.
  - Writes with `wb`.
  - With `core.autocrlf`/`text` attributes, the rewrite may be invisible to `git diff`, giving "Failed" with exit 1 but no "files were modified" line (§6.7).
- **Epochs:** identical v2.1.0 → v6.0.0; the manifest `stages` change is on main only.
- **Native notes:** a single memchr2 pass (`\r`, `\n`) counts endings; a second pass emits. No per-line allocation.

#### `fix-byte-order-marker` — NATIVE (P0)

- **Manifest:** `types: [text]`; name `fix utf-8 byte order marker`.
- **Algorithm** (`fix_byte_order_marker.py:14-25`):
  1. Read the first 3 bytes. If they are not `EF BB BF`, do nothing.
  2. Otherwise read the whole file as text with `encoding='utf-8-sig', newline=''`. This **strictly validates the remainder as UTF-8** (surrogates, overlongs and >U+10FFFF are rejected, exactly like Rust `str::from_utf8`) and strips **one** BOM.
  3. Write it back with `utf-8`, `newline=''`. The bytes are identical to `input[3..]`.
  4. Print `{filename}: removed byte-order marker`, rc=1.
- **Verified ✓:**

| input | result |
|---|---|
| `\xef\xbb\xbfhello\r\n` | `hello\r\n` (no newline translation) |
| double BOM | one BOM remains |
| BOM only | empty file |
| BOM + invalid UTF-8 | **crash** (`UnicodeDecodeError: 'utf-8' codec can't decode byte 0xe9 in position 2: invalid continuation byte`), exit 1, file unchanged, remaining files in the partition not processed |

- **Exit:** 0 or 1.
- **Epochs:** v3.3.0 → v6.0.0 identical (the hook was introduced in v3.3.0).
- **Native notes:**
  - Read the 3-byte prefix. Only if it is a BOM, read the rest, validate with `std::str::from_utf8` (the acceptance set is the same as CPython's strict UTF-8), then write `rest` in place (`write_all` + `set_len`).
  - Invalid UTF-8 is the crash path (§2.6).

### 4.B Sorters

#### `file-contents-sorter` — NATIVE (P1)

- **Manifest:** `files: '^$'` (matches nothing, so users must set `files:`); name `file contents sorter`.
- **CLI** (`file_contents_sorter.py:54-72`):
  - `filenames nargs='+'` (**required**).
  - Mutually exclusive group (since v6.0.0): `--ignore-case` (`store_const` `bytes.lower`) | `--unique` (`store_true`).
  - Errors ✓:
    - both flags given: `file-contents-sorter: error: argument --ignore-case: not allowed with argument --unique`, exit 2. The message names the *later* argument first: argparse reports "argument X: not allowed with argument Y", where X is the one being parsed when the conflict is detected, i.e. the later one in argv;
    - no files (e.g. user `always_run: true` with no matching files ✓): `file-contents-sorter: error: the following arguments are required: filenames`, exit 2.
- **Algorithm** (`:25-51`):
  1. `before = list(f)`: binary lines split after `\n`.
  2. `lines = [l.rstrip(b'\n\r') for l in before if l.strip()]`:
     - lines that are empty or ASCII-WS-only are dropped;
     - **all** trailing CR/LF bytes are stripped;
     - other trailing whitespace is kept (`c \n` becomes `c `).
  3. If `--unique`: `set(lines)` (exact byte dedupe).
  4. `sorted(lines, key=None | bytes.lower)`: bytewise lexicographic order (= Rust `[u8]` `Ord`). The sort is **stable**. `bytes.lower` maps only ASCII `A-Z` ✓ (`É`/`é` are not folded).
  5. `after = b'\n'.join(sorted)`, plus `b'\n'` if non-empty.
  6. If `b''.join(before) == after`, PASS. Otherwise seek(0), write, truncate; print `Sorting {filename}`; FAIL.
- **Verified ✓:**

| input | args | result |
|---|---|---|
| `b\r\nA\n\n  \nc \na\nB` | default | `A\nB\na\nb\nc \n` (CRLF normalised to LF; blank lines removed) |
| `a\rc\rb\r` | default | one "line", gives `a\rc\rb\n` |
| `b\nB\nA\na\n` | `--ignore-case` | `A\na\nb\nB\n` (stable) |
| `b\nb\r\nA\na\na \n` | `--unique` | `A\na\na \nb\n` |
| empty file | – | pass |
| `\n\n` | – | becomes empty, `Sorting` |

- **Exit:** 0 or 1; 2 on argparse error.
- **Epochs:**
  - **E1** v3.0.0–v4.4.0: `after = b'\n'.join(after) + b'\n'` always, so **an empty or whitespace-only file becomes `"\n"`** and fails. No mutual exclusion. `--ignore-case` exists since v3.3.0 and `--unique` since v3.4.0. Before those versions the flags are argparse errors.
  - **E2** v4.5.0–v5.0.0: empty-file fix. Both flags are allowed together (`set` then sort by `lower`).
  - **E3** v6.0.0+: mutually exclusive.
- **Native notes:** collect `&[u8]` slices into a Vec, `sort_by` (stable) with an ASCII-lowercase comparator (`a.iter().map(u8::to_ascii_lowercase)` lexicographic). Dedupe with a `HashSet<&[u8]>`; order does not matter because the result is sorted afterwards.

#### `requirements-txt-fixer` — NATIVE (P1)

- **Manifest:** `files: (requirements|constraints).*\.txt$` (v4.5.0+; before that `requirements.*\.txt$`); name `fix requirements.txt`. No `types` filter (default `[file]`).
- **CLI:** `filenames *`.
- **Data model** (`requirements_txt_fixer.py:13-65`): a `Requirement` has `value: bytes|None` and `comments: list[bytes]` (whole lines, including their line endings).
  - `UNTIL_COMPARISON = re.compile(b'={2,3}|!=|~=|>=?|<=?')`
  - `UNTIL_SEP = re.compile(rb'[^;\s]+')`. Python bytes `\s` is ASCII-WS.
  - **`name`** (:21-37):
    1. `n = value.lower()`. This is ASCII-only lowering, and the value includes the line ending.
    2. If `b'#egg='` or `b'&egg='` is in the original value: return `n.partition(egg)[2]`, i.e. everything after the first occurrence, **including the trailing newline**.
    3. Else `m = UNTIL_SEP.match(n)`. **If `m` is None, `assert` fails, giving `AssertionError` (crash) ✓.** This happens for a value starting with whitespace or `;` (e.g. an indented requirement `  flask==1`).
    4. `n = m.group()`; `c = UNTIL_COMPARISON.search(n)`; return `n[:c.start()]` if found, else `n`.
  - **`__lt__`** (:39-53):
    - if `self.value == b'\n'` (header marker): True;
    - elif `other.value == b'\n'`: False;
    - elif the names are equal: `bool(self.comments) > bool(other.comments)` (commented entries first);
    - else `self.name < other.name` (bytes).
    
    Equivalent to a stable sort by the key `(not is_header, name, not has_comments)`. That is a valid total preorder, so any stable sort reproduces Python's `sorted`.
  - `is_complete()` = `value is not None and not value.rstrip(b'\r\n').endswith(b'\\')`.
  - `append_value(v)` concatenates bytes; this is how continuations are joined.
- **`fix_requirements`** (:68-142):
  1. `before = list(f)`; `before_string = b''.join(before)` (the original).
  2. If the last line lacks `\n`, append `b'\n'` **to the working copy only**. The comparison uses the original, so a missing final newline always triggers a rewrite ✓.
  3. If `before_string.strip() == b''` (empty or ASCII-WS only): PASS, untouched ✓.
  4. For each line:
     - if there are no requirements, or the last one is complete, push a new `Requirement`; `r` is the last one;
     - if `len(requirements) == 1` and `line.strip() == b''`: if `r.comments` is non-empty **and** `r.comments[0].startswith(b'#')` (the first line of the file is a comment in column 0), set `r.value = b'\n'` (**header block**); else append the line to `r.comments`;
     - elif `line.lstrip().startswith(b'#')` (v4.0.0+; before that `line.startswith(b'#')`) or `line.strip() == b''`: append to `r.comments`;
     - else `r.append_value(line)`.
  5. If the last requirement has `value is None`, its comments become `rest`: trailing comments, kept at the end.
  6. Drop requirements whose value is exactly `b'pkg-resources==0.0.0\n'` or `b'pkg_resources==0.0.0\n'`. **LF only**: the CRLF variant is kept ✓. The dropped entry's comments go with it. `pkg_resources` is v5.0.0+; before that only `pkg-resources`.
  7. Sort (above). Then for each requirement in order: extend `after` with its comments (**even for duplicates**); if `prev is None or value != prev.value`, append the value and set `prev` (v4.6.0+ dedupe; before that, always append).
  8. Extend with `rest`. If `b''.join(after) == before_string`, PASS. Otherwise rewrite (seek 0, write, truncate), print `Sorting {filename}`, FAIL.
- **Verified ✓:**
  - Input:
    ```
    # top comment\n\nzope==1\n# about flask\nFlask>=2  # x\nflask==1\nDjango[bcrypt]~=4\npkg-resources==0.0.0\nrequests @ https://x/r.zip\n-e git+https://g/x.git#egg=Alpha\nnumpy; python_version>"3"\nfoo \\\n  --hash=sha256:abc\nbar\n# trailing\n
    ```
    becomes
    ```
    # top comment\n\n-e git+https://g/x.git#egg=Alpha\nbar\nDjango[bcrypt]~=4\n# about flask\nFlask>=2  # x\nflask==1\nfoo \\\n  --hash=sha256:abc\nnumpy; python_version>"3"\nrequests @ https://x/r.zip\nzope==1\n# trailing\n
    ```
    (The egg name is `alpha\n`, which sorts first.)
  - `a==1\nb\na==1\n# c\nb\n` → `a==1\n# c\nb\n`.
  - `\nb\na\n` → `a\n\nb\n`: the leading blank line is not a header because comments[0] is blank, so it attaches to `b`.
  - `# c1\n# c2\nb\n\na\n` → `\na\n# c1\n# c2\nb\n`: no blank line after the comments, so they are not a header.
  - `# hdr\n# hdr2\n\n# for z\nz\n\na\n# trailing\n\n` → `# hdr\n# hdr2\n\n\na\n# for z\nz\n# trailing\n\n`.
  - `a \\\n\n  --hash=x\nB\n` → `\na \\\n  --hash=x\nB\n`: a blank line inside a continuation becomes a comment.
  - `b\r\npkg-resources==0.0.0\r\na\r\n` → `a\r\nb\r\npkg-resources==0.0.0\r\n`.
  - `b\n  flask==1\n` → **AssertionError crash**, exit 1.
- **Exit:** 0 or 1.
- **Epochs:**
  - E1 v3.1.0–v3.4.0: operators set as above; comment test `line.startswith(b'#')`.
  - E2 v4.0.0–v4.5.0: `lstrip()` comment test.
  - E3 v4.6.0: dedupe plus comment-first tie-break.
  - E4 v5.0.0+: `pkg_resources` too.
  - v3.0.0 has an older `UNTIL_COMPARISON`; delegate.
- **Native notes:**
  - Byte-exact port.
  - Keep the comparator exactly as specified: names are compared as bytes, including the trailing `\n` in the egg case.
  - The crash path must abort the partition.

#### `sort-simple-yaml` — NATIVE (P2)

- **Manifest:** `files: '^$'` (opt-in); name `sort simple yaml files`.
- **Algorithm** (`sort_simple_yaml.py:29-121`):
  1. Open the file **text** `r+`, locale encoding, universal newlines.
  2. `lines = [l.rstrip() for l in f.readlines()]`. `str.rstrip()` strips PY-STR-WS: NBSP, `\x1c` and so on ✓ (`a: 2\xa0` becomes `a: 2` when rewritten).
  3. `new = sort(lines)`:
     - `header = parse_block(lines, header=True)`: pop leading lines while the line is non-empty and starts with `#`;
     - `blocks = parse_blocks(rest)`: skip `''` lines; each block is a maximal run of non-empty lines;
     - stable sort the blocks by `first_key(block)` = the first line not starting with `#`, with one leading `'` or `"` removed (`line[1:]`), else `''`. Python str order is by code point (= Rust `str` byte order);
     - `new = header`; for each block: if `new` is non-empty, append `''`; then extend with the block.
  4. If `lines != new`: print ``Fixing file `{filename}` `` (with backticks); `seek(0)`; write `'\n'.join(new) + '\n'` (text mode: `\r\n` on Windows); `truncate()`; rc=1.
- **Consequences ✓:**
  - Trailing whitespace alone does not trigger a rewrite, because both sides are rstripped: `b: 1  \n` is untouched.
  - A file that is exactly `\n` gives `lines=['']`, `new=[]`, so the hook **rewrites `"\n"` and fails on every run (perpetual failure)**. `\n\n` becomes `\n`. An empty file passes.
  - A comment directly above the first key, with no blank line between, stays in the header: `# c\nb: 1\n\na: 2` becomes `# c\n\na: 2\n\nb: 1\n`.
  - CRLF input that needs no reorder is untouched. When rewritten it becomes LF on POSIX.
  - Invalid UTF-8 under a UTF-8 locale crashes.
- **Epochs:** identical v3.0.0 → v6.0.0.
- **Native notes:**
  - Decode as UTF-8. On Windows with CPython < 3.15 and without UTF-8 mode, Python would use the ANSI code page: documented caveat (§10 Q6).
  - Universal newline split: `\r\n`, `\r` and `\n` all end a line.
  - Strip PY-STR-WS on the right.
  - Write with `\n` on POSIX. On Windows, write `\r\n` (parity).

### 4.C Syntax checkers / formatters

#### `check-json` — NATIVE (P0) (requires a CPython-`json`-compatible decoder, §9.4)

- **Manifest:** `types: [json]`; name `check json`.
- **Algorithm** (`check_json.py:9-34`): for each file, open it `rb` and call `json.load(f, object_pairs_hook=raise_duplicate_keys)`. On `ValueError` (this includes `JSONDecodeError`, `UnicodeDecodeError`, the int-digit-limit `ValueError` and the duplicate error), print `{filename}: Failed to json decode ({exc})` and set rc=1.
- **Decoding of bytes** (`json.loads(bytes)`):
  1. `detect_encoding(b)`:
     - BOM UTF-32 BE/LE gives `utf-32`;
     - BOM UTF-16 BE/LE gives `utf-16`;
     - BOM UTF-8 gives `utf-8-sig`;
     - else if `len ≥ 4`: `b[0]==0` gives `utf-16-be` if `b[1]` else `utf-32-be`; `b[1]==0` gives `utf-16-le` if `b[2] or b[3]` else `utf-32-le`;
     - elif `len == 2`: `b[0]==0` gives `utf-16-be`; `b[1]==0` gives `utf-16-le`;
     - else `utf-8`.
  2. Decode with the **`surrogatepass`** error handler. UTF-8 bytes `ED A0 80`…`ED BF BF` (encoded surrogates) are **accepted** inside strings ✓.
  - Decode errors become `ValueError` with Python's codec message ✓:
    - `'utf-8' codec can't decode byte 0xff in position 7: invalid start byte`
    - `… byte 0xc3 in position 1: invalid continuation byte`
    - `… bytes in position 0-1: unexpected end of data`
  - ✓ The UTF-8 BOM is accepted; UTF-16-LE with a BOM is accepted.
- **Grammar** (CPython `json` with the C scanner):
  - Whitespace is `[ \t\n\r]*` only.
  - Values: object, array, string, number `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, `true`, `false`, `null`, **`NaN`, `Infinity`, `-Infinity`** (accepted ✓; `-NaN` rejected).
  - Strings: control characters < 0x20 are rejected ("Invalid control character at"). Escapes are `\" \\ \/ \b \f \n \r \t \uXXXX`. Lone surrogates via `\ud800` are **accepted** ✓. Surrogate pairs combine.
  - **Integers are arbitrary precision**, but **CPython ≥3.11 rejects integer literals with > 4300 digits**: `Exceeds the limit (4300 digits) for integer string conversion: value has 5000 digits; use sys.set_int_max_str_digits() to increase the limit` ✓. This is a ValueError, so it counts as a failure, not a crash. Floats with a fraction or exponent go through `float()`, so `1e999` is `inf` and accepted.
  - Trailing data gives `Extra data`.
- **Error message format:** `"{msg}: line {lineno} column {colno} (char {pos})"`.
  - `pos` is the **code-point index** into the decoded string (not bytes) ✓.
  - `lineno = count('\n', 0, pos) + 1`; `colno = pos - rfind('\n', 0, pos)` (1-based). Only `\n` counts as a newline.
- **Message catalogue** (CPython 3.12; ✓ each):

| input | message |
|---|---|
| `` (empty) | `Expecting value: line 1 column 1 (char 0)` |
| `  ` | `Expecting value: line 1 column 3 (char 2)` |
| ` \n ` | `Expecting value: line 2 column 2 (char 3)` |
| `{"a": 1} {}` | `Extra data: line 1 column 10 (char 9)` |
| `01` / `1.` / `1e` | `Extra data: line 1 column 2 (char 1)` |
| `.5` / `-` / `tru` / `-NaN` | `Expecting value: line 1 column 1 (char 0)` |
| `{"a": 1, "a": 2` | `Expecting ',' delimiter: line 1 column 16 (char 15)` |
| `{\n  "a": 1\n  "b": 2\n}` | `Expecting ',' delimiter: line 3 column 3 (char 13)` |
| `[1 2]` | `Expecting ',' delimiter: line 1 column 4 (char 3)` |
| `{"a"  1}` | `Expecting ':' delimiter: line 1 column 7 (char 6)` |
| `{"a"` | `Expecting ':' delimiter: line 1 column 5 (char 4)` |
| `{'a': 1}` | `Expecting property name enclosed in double quotes: line 1 column 2 (char 1)` |
| `{` | `Expecting property name enclosed in double quotes: line 1 column 2 (char 1)` |
| `{"a": 1,}` | 3.12: `Expecting property name enclosed in double quotes: line 1 column 9 (char 8)`; **3.13+: `Illegal trailing comma before end of object: line 1 column 8 (char 7)`** ✓ |
| `[1,]` | 3.12: `Expecting value: line 1 column 4 (char 3)`; **3.13+: `Illegal trailing comma before end of array: line 1 column 3 (char 2)`** ✓ |
| `{"a":}` | `Expecting value: line 1 column 6 (char 5)` |
| `"abc` | `Unterminated string starting at: line 1 column 1 (char 0)` (pos = the opening quote) |
| `{"a": "x` | `Unterminated string starting at: line 1 column 7 (char 6)` |
| `{"a": "x<TAB>y"}` | `Invalid control character at: line 1 column 9 (char 8)` |
| `{"a":\n "éé<LF>` | `Invalid control character at: line 2 column 5 (char 10)` (code points) |
| `"\x"` | `Invalid \escape: line 1 column 2 (char 1)` (pos = the backslash) |
| `"\u12"`, `"\uZZZZ"` | `Invalid \uXXXX escape: line 1 column 3 (char 2)` (pos = the `u`) |

- **Duplicate keys:** `raise_duplicate_keys` runs when an object is **closed** (inner objects first). The message is `Duplicate key: {key}`, where key is the decoded str printed raw:
  - ✓ `{"a": {"b": 1, "b": 2}, "a": 3}` gives `Duplicate key: b`;
  - `{"é":1,"é":2}` gives `Duplicate key: é`.
  
  A syntax error *before* the object closes wins: `{"a": 1, "a": 2` reports the delimiter error. The native decoder must check duplicates at object close, not at key insertion.
- **Recursion:** the C scanner hits the C recursion limit. On 3.12 ✓, arrays nested ≥ 9998 deep, and objects ≥ ~9996 deep with the hook, give `RecursionError` (not a ValueError), i.e. a **crash** (exit 1, abort). The native decoder must be iterative and must not overflow the stack. Recommendation: treat depth > 9997 as the crash path (message `"{prog}: error: maximum nesting depth exceeded"`).
- **Exit:** 0 or 1.
- **Epochs:** E1 v3.0.0–v3.4.0 has no duplicate detection (plain `json.load`). E2 v4.0.0+ is as above.
- **Native notes:**
  - Port the CPython scanner state machine (`Modules/_json.c` `scan_once_unicode`, `_parse_object_unicode`, `_parse_array_unicode`, `scanstring_unicode`, `_match_number_unicode`) over a `Vec<char>`-free representation.
  - Decode to a "PyStr" = UTF-8 with surrogates allowed (WTF-8). Track char positions with a running code-point counter.
  - No DOM is needed for check-json: validate and track keys per open object in a `HashSet<Box<[u8]>>` (WTF-8 bytes), and test on close in insertion order. The first duplicate in **pair order** is the reported one: `object_pairs_hook` iterates the pairs in order and raises on the first key already seen.
  - Python-version-dependent messages: emulate one CPython (§10 Q2).

#### `pretty-format-json` — NATIVE (P1) (needs the json decoder, a CPython-compatible `json.dumps` and a `difflib.unified_diff` port)

- **Manifest:** `types: [json]`; name `pretty format json`.
- **CLI** (`pretty_format_json.py:58-99`):

| option | default | parsing |
|---|---|---|
| `--autofix` | False | |
| `--indent INDENT` | `'2'` | `parse_num_to_int`: `int(s)` if it parses (Python `int()` rules, Appendix B), else the raw string |
| `--no-ensure-ascii` | False | |
| `--no-sort-keys` | False | |
| `--top-keys TOP_KEYS` | `[]` | `s.split(',')`. The **last** occurrence wins (store, not append) |
| `filenames` | `*` | |

- **Algorithm** (`:102-133`), for each file:
  1. `contents = open(f, encoding='UTF-8').read()`: **text mode, universal newlines** (CRLF/CR become LF), BOM **kept** as U+FEFF. Invalid UTF-8 raises `UnicodeDecodeError` **outside** the try, so it **crashes** ✓.
  2. `pretty = _get_pretty_format(contents, indent, ensure_ascii=not no_ensure_ascii, sort_keys=not no_sort_keys, top_keys)`:
     - `json.loads(contents, object_pairs_hook=pairs_first)`. For **every** object (recursively): `before = [p for p in pairs if p.key in top_keys]`, stable-sorted by `top_keys.index(key)`; `after = [p for p in pairs if p.key not in top_keys]`, `after.sort()` if sort_keys. Then `dict(before + after)`.
       - `after.sort()` sorts **(key, value) tuples**. With duplicate keys it compares the *values*:
         - comparable values: sorted by value, and `dict()` keeps the **last**, so the larger value survives. ✓ `{"a": 1, "a": 2}` gives `"a": 2`; `{"x": 1, "x": 0}` gives `"x": 1`.
         - incomparable values (e.g. dict vs int): **`TypeError` crash** ✓ (`'<' not supported between instances of 'int' and 'dict'`).
       - With `--no-sort-keys`, `dict()` keeps the first position with the **last** value.
       - A str with a BOM gives `JSONDecodeError("Unexpected UTF-8 BOM (decode using utf-8-sig)")`, a ValueError, i.e. "not a valid JSON" ✓.
     - `json.dumps(obj, indent=indent, ensure_ascii=…)`, plus `'\n'`. Formatting rules (CPython `json.encoder`), with an indent the separators are `(',', ': ')`:
       - indent int `n` means `' '*n` per level; `n ≤ 0` means newlines with no indentation ✓ (`--indent=0`, `--indent=-2`);
       - indent str is used literally per level (`'\t'` ✓, `''` ✓ means newlines only);
       - empty containers are `[]` and `{}` on one line ✓;
       - otherwise `[\n` + indent*(lvl+1) + items joined by `,\n`+indent… + `\n` + indent*lvl + `]`;
       - strings with `ensure_ascii=True`: escape `"`→`\"`, `\`→`\\`, `\n \r \t \b \f` as two-character escapes, and **every other character outside 0x20–0x7E** as `\uXXXX` (lowercase hex), non-BMP as a surrogate pair ✓ (`\u007f`, `é`→`\u00e9`, `😀`→`\ud83d\ude00`, lone `\ud800` preserved). `/` is not escaped;
       - with `ensure_ascii=False`: only `"`, `\`, `\n \r \t \b \f` and the other control characters < 0x20 (`\u00XX`) are escaped. DEL and U+2028 are raw. A lone surrogate then fails to encode at write or print time, which is a crash;
       - numbers: ints in decimal (arbitrary size); floats by `float.__repr__` (shortest round-trip; scientific when exp < -4 or ≥ 16 with a 2+-digit exponent: `1e+16`, `1.5e-07`, `2.5`, `-0.0`, `1e+22`). `inf`/`-inf`/`nan` become `Infinity`/`-Infinity`/`NaN` ✓ (`1E400` becomes `Infinity`).
  3. On `ValueError`: print `Input File {json_file} is not a valid JSON, consider using check-json`; status=1; continue (v5.0.0+; **≤v4.6.0 returns 1 immediately**, so later files are not processed).
  4. Else, if `contents != pretty` (compared after universal-newline translation, **so CRLF files that are otherwise pretty pass** ✓):
     - with `--autofix`: print `Fixing file {filename}`, then write with `open(f, 'w', encoding='UTF-8')`. **Text mode: Windows writes `\r\n`**;
     - else write `get_diff(contents, pretty, filename).encode()` (UTF-8) to `sys.stdout.buffer`.
     - status=1 either way.
- **Diff format** = `difflib.unified_diff(src.splitlines(True), dst.splitlines(True), fromfile=f, tofile=f)` joined:
  - header `--- {f}\n+++ {f}\n`, with no timestamps and **no trailing tab**;
  - hunks `@@ -a,b +c,d @@\n` with n=3 context lines. A range `1` is printed as `-1` (no `,1`); an empty range is printed as `-0,0` with start-1;
  - **lines lacking a final newline are emitted as is, with no `\ No newline at end of file`**, so a one-line file gives ✓ `-{"a":1}+{\n+  "a": 1\n+}\n`;
  - `str.splitlines(True)` splits on PY str line boundaries (`\n \r \r\n \x0b \x0c \x1c \x1d \x1e \x85 \u2028 \u2029`). After universal-newline reading only `\n` and the exotic ones remain; a raw U+2028 inside a JSON string splits the diff line.
  - **Hunk grouping must use CPython's `SequenceMatcher`** (Ratcliff–Obershelp, `autojunk=True`: in `b` sequences of ≥ 200 elements, elements occurring more than `1 + len(b)//100` times are "popular" and ignored as junk). Port `get_matching_blocks`, `get_opcodes` and `get_grouped_opcodes(3)` exactly. `similar`'s Myers diff does not give identical hunks.
- **Output ordering** (non-color mode): see §2.3. ✓ All diffs (via buffer) appeared before the `Input File …` lines (via print). Color/pty mode: program order.
- **Exit:** 0 or 1.
- **Epochs:**
  - v4.0.0–v4.6.0: stop at the first invalid file (`return 1`).
  - v5.0.0+: continue.
  - <v4.0.0: diff printed via text `print` (encoding issues): delegate.
- **Native notes:**
  - Implement a `PyJson` value (`Int(BigInt-as-decimal-string)`, `Float(f64)`, `Str(Wtf8)`, …) preserving the object pair order after the `pairs_first` transform.
  - The tuple sort comparator must emulate Python ordering across types: int/float/bool compare numerically (bool is an int); str with str; list with list (lexicographic); `None` is not comparable; dict is not comparable. Incomparable pairs go to the crash path.
  - prek ships this only as `repo: builtin` (not in its fast path) because parity is hard. snp should differential-fuzz before enabling it (§9.9).

#### `check-yaml` — NATIVE-WITH-CAVEATS (P0; second most used hook)

- **Manifest:** `types: [yaml]`; name `check yaml`.
- **CLI:** `-m`/`--multi`/`--allow-multiple-documents` (store_true); `--unsafe`; `filenames *`.
- **Mode table** (`check_yaml.py:32-37`), with `yaml = ruamel.yaml.YAML(typ='safe')`:

| multi | unsafe | function | stages run |
|---|---|---|---|
| F | F | `yaml.load(f)` | reader → scanner → parser → composer (**exactly one document**) → SafeConstructor |
| T | F | `yaml.load_all(f)`, exhausted | all documents composed and constructed |
| any | T | `yaml.parse(f)`, exhausted | reader → scanner → parser only (events). No alias resolution, no tags, no duplicate keys |

- **Input:**
  - `open(filename, encoding='UTF-8')`, text mode with universal newlines.
  - Invalid UTF-8 is a **crash** ✓ (`UnicodeDecodeError`, not caught).
  - The BOM is read as U+FEFF and skipped by the scanner at stream start ✓.
  - CRLF becomes LF before parsing, so marks count lines correctly.
- **Errors caught:** `ruamel.yaml.YAMLError`. The hook prints `str(exc)` (**no filename prefix**; the marks contain it) and sets rc=1. Other exceptions are crashes ✓:
  - `!!int "abc"` gives `ValueError: invalid literal for int() with base 10: 'abc'`;
  - `!!float abc`;
  - invalid dates such as `a: 2001-13-45` give `ValueError: month must be in 1..12`. This happens even for **implicit** timestamps: the resolver regex accepts it, then the constructor fails;
  - `!!bool maybe` gives `KeyError`;
  - `%YAML 1.3` gives `AssertionError: version minor part can only be 2 or 1, got (1, 3)`;
  - `!!omap` with a duplicate key gives `AssertionError`.
- **Message format** (`MarkedYAMLError.__str__`): lines joined by `\n`, in this order:
  - `context`, if set;
  - the context mark, if set **and** not at the same name/line/col as the problem mark;
  - `problem`;
  - the problem mark;
  - `note`, if set.
  
  Marks:
  - FileMark: `  in "{filename}", line {L}, column {C}` (1-based, counted after newline translation). **No source snippet**, because the input is a file stream ✓;
  - ReaderError: `unacceptable character #x{cp:04x}: special characters are not allowed\n  in "{filename}", position {index}` (the code-point index).
- **Default-mode catalogue** (✓ ruamel 0.19.1 pure):

| input | output (`str(exc)`) |
|---|---|
| `a: 1\na: 2\n` | `while constructing a mapping\n  in "02.yaml", line 1, column 1\nfound duplicate key "a" with value "2" (original value: "1")\n  in "02.yaml", line 2, column 1\n\nTo suppress this check see:\n    https://yaml.dev/doc/ruamel.yaml/api/#Duplicate_keys\n` |
| `a: 1\n---\nb: 2\n` (single mode) | `expected a single document in the stream\n  in "03.yaml", line 1, column 1\nbut found another document\n  in "03.yaml", line 2, column 1` |
| `a: 1\n---\n` (a 2nd empty document) | same as above. `a: 1\n...\nb: 2\n` reports the 2nd document at line 3 |
| `a: !foo 1` | `could not determine a constructor for the tag '!foo'\n  in "04.yaml", line 1, column 4` |
| `a: !!python/object:os.system x` | `could not determine a constructor for the tag 'tag:yaml.org,2002:python/object:os.system'` + mark |
| `%TAG ! tag:x,2000:\n---\na: !foo 1` | `… for the tag 'tag:x,2000:foo'` + mark at line 3, column 4 |
| `a: =` | `could not determine a constructor for the tag 'tag:yaml.org,2002:value'` (the plain scalar `=` resolves to the YAML 1.1 "value" tag in both 1.1 and 1.2 modes) |
| `a: [1, 2\n` | `while parsing a flow sequence\n  in "05.yaml", line 1, column 4\nexpected ',' or ']', but got '<stream end>'\n  in "05.yaml", line 2, column 1` |
| `a: b: c` | `mapping values are not allowed here\n  in "06.yaml", line 1, column 5` |
| `a: "\x07"` | `unacceptable character #x0007: special characters are not allowed\n  in "08.yaml", position 4` |
| `a: 1\n\x00` | `unacceptable character #x0000: … position 5` |
| `a: *nope` | `found undefined alias 'nope'\n  in "10.yaml", line 1, column 4` |
| `key: value\n\tbad: tab` | `while scanning for the next token\nfound character '\t' that cannot start any token\n  in "12.yaml", line 2, column 1` (the context mark is absent) |
| `a: \|\n  x\n b: 1` | `while parsing a block mapping\n  in …, line 1, column 1\nexpected <block end>, but found '<block mapping start>'\n  in …, line 3, column 2` |
| `"\x"` | `while scanning a double-quoted scalar\n  in …, line 1, column 1\nexpected escape sequence of 2 hexdecimal numbers, but found '"'\n  in …, line 1, column 4` |
| `a:\n  - b\n  c: d` | `while parsing a block collection\n  in …, line 2, column 3\nexpected <block end>, but found '?'\n  in …, line 3, column 3` |
| `<<: [1]` | `while constructing a mapping\n  in …, line 1, column 1\nexpected a mapping for merging, but found scalar\n  in …, line 1, column 6` |
| `<<: {a: 1}\n<<: {b: 2}` | `while constructing a mapping\n  in …, line 1, column 1\nfound duplicate merge key "<<"\n  in …, line 2, column 1\nDuplicate merge keys are never allowed, not even when\n`.allow_duplicate_keys` is set to True` ✓ (trailing newlines are invisible after pre-commit's `strip()`) |
| `{a: 1}: 2` | `while constructing a mapping\nfound unhashable key\n  in …, line 1, column 1` |
| `!!map` (scalar) | `expected a mapping node, but found scalar\n  in …, line 1, column 1` |
| `a: !!omap [1, 2]` | `while constructing an ordered map\n  in …, line 1, column 4\nexpected a mapping of length 1, but found scalar\n  in …, line 1, column 12` |
| `%YAML 2.0\n---\na: 1` | `found incompatible YAML document (version 1.* is required)\n  in …, line 1, column 1` |
| `--- !!set\n? a\n? a` | `while constructing a mapping … found duplicate key "a" with value "None" (original value: "None")` |

  Passing ✓: empty file, BOM, CRLF, `? [1, 2]`, `[1, 2]: x` (list keys become tuples), `%YAML 1.1` with `yes`, `0x1G`, `!!binary "!!!"`, `!!str` with no value, `!!null x`, `---\n...\n`, duplicate anchors (`&x` twice), `%FOO bar` (unknown directive), `\xc2\x85` (NEL) inside quotes, and the merge pattern `<<: *b`.
- **Duplicate-key semantics = Python dict membership** after construction (`check_mapping_key`, `ruamel/yaml/constructor.py:256-290`):
  - **`1`, `1.0`, `true` and `0x01` are all the same key** ✓ (`found duplicate key "1.0"`, `"True"`). `0`, `0.0`, `false` and `-0.0` likewise.
  - `.inf`/`.Inf` are equal. **`.nan` keys are duplicates of each other** ✓: ruamel returns one shared NaN object, so identity applies.
  - `~` and `null` are both `None`.
  - In 1.2 mode: `017` == `17` (decimal), `0o17` == `15`, `1_000` == `1000`, and `1:20` is a **string**. Under `%YAML 1.1`, `1:20` is the sexagesimal `80` ✓ and `yes == true`.
  - date `2001-01-01` ≠ datetime `2001-01-01 00:00:00` ✓.
  - Sequence keys become tuples, printed `(1,)`. Mapping or set keys give "found unhashable key".
  - **Printed key and values use Python `str()` of the constructed object.** Containers are printed **as constructed so far, which is empty** ✓: `"[]"`, `"{}"`, `"set()"`. Further formats: binary as `b'hi'`; floats as `repr` (`1500.0`, `1e+16`, `inf`, `-inf`, `nan`); bools as `True`/`False`; null as `None`; datetimes as `str()` (`2001-01-01 10:00:00`); dates as `2001-01-01`. Strings are raw and may contain newlines and quotes ✓ (`"it's\n"`).
- **Implicit resolvers**: ruamel `VersionedResolver`. The default is **YAML 1.2** unless a `%YAML 1.1` directive is present. Port the 1.1 and 1.2 regex tables from `ruamel/yaml/resolver.py`: bool, int (dec, `0o`, `0x`, `0b`, sexagesimal in 1.1, `_` separators), float, null, timestamp, merge `<<`, value `=`. ruamel 1.2 quirks include `017` as decimal; port them, do not guess.
- **`--multi`:** as default, but any number of documents. Each document is constructed; the first error stops processing of that file.
- **`--unsafe`:** only scanner and parser errors are reported ✓. Undefined aliases, unknown tags, duplicate keys, `=` and invalid timestamps all **pass**.
- **Exit:** 0 or 1.
- **Version caveats:**
  - pre-commit-hooks pins `ruamel.yaml>=0.15` (unpinned), so behaviour depends on when the user's env was built.
  - ruamel ≤0.18 on CPython with `ruamel.yaml.clib` installed (the default then) used the **libyaml C parser**, whose messages differ (e.g. `did not find expected key`).
  - Target: **ruamel ≥0.19 pure-Python semantics** (what fresh envs get today).
- **Epochs:** identical semantics v2.2.1 → v6.0.0; only the help text typo changed in v4.5.0.
- **Native plan (two stages):**
  1. **Acceptance parity (ship in `auto` mode).**
     - A YAML 1.2/1.1 parser producing events. Candidates: `saphyr-parser` 0.1 (YAML 1.2, yaml-test-suite-driven) or `yaml-rust2` 0.13.
     - snp's own composer: anchors and aliases with the "found undefined alias" error. **Never expand aliases** (billion-laughs safe); keep node ids.
     - A "SafeConstructor-checker":
       - a tag table, with "could not determine a constructor" for unknown tags;
       - scalar validators reproducing the **crash** paths (int/float/bool/timestamp/binary/omap); the crash path gives exit 1 and aborts;
       - duplicate keys with the Python equality model above;
       - merge-key rules;
       - unhashable keys.
     - Emit ruamel-formatted messages, with the context/problem texts mapped as closely as possible.
     - Parser-level message texts may differ: this is the documented caveat.
  2. **Message parity (`exact` mode).** Port ruamel's pure scanner, parser and composer (`scanner.py`, `parser.py`, `composer.py`, `reader.py`, about 4.5k Python lines) to Rust. Then all parser error texts and marks match.

#### `check-toml` — NATIVE-WITH-CAVEATS (P1)

- **Manifest:** `types: [toml]`; name `check toml`.
- **Algorithm** (`check_toml.py:13-26`): `tomllib.load(open(f,'rb'))` (`tomli` on Python < 3.11). On `TOMLDecodeError`, print `{filename}: {exc}` and set rc=1.
- **Input handling:** `tomllib` does `b.decode()`, which is strict UTF-8 → `UnicodeDecodeError` **crash** ✓.
- **Error format:** `{msg} (at line {L}, column {C})` or `{msg} (at end of document)`. Line and column are 1-based, the column counted in code points.
- **Catalogue** (✓ CPython 3.12, TOML 1.0):

| input | message |
|---|---|
| `a = 1\na = 2` | `Cannot overwrite a value (at line 2, column 6)` |
| `a.b = 1\na = 2` | `Cannot overwrite a value (at line 3, column 6)` |
| `a = ` | `Invalid value (at line 1, column 5)` |
| `x = [1,\n2,\n` | `Invalid value (at end of document)` |
| BOM + `a = 1` | `Invalid statement (at line 1, column 1)` (**the BOM is rejected**) |
| `a = { b = 1, }` | `Invalid initial character for a key part (at line 1, column 14)` (TOML 1.0; allowed in 1.1) |
| `a = {\n b = 1 }` | `Invalid initial character for a key part (at line 1, column 6)` (1.0; allowed in 1.1) |
| `a = 1 # \x07` | `Found invalid character '\x07' (at line 1, column 9)` |
| `[a]\n[a]` | `Cannot declare ('a',) twice (at line 2, column 3)` |
| `a = 1979-05-27T07:32` | `Expected newline or end of document after a statement (at line 1, column 15)` (1.0; valid in 1.1) |
| `a = 12:30` | `… (at line 1, column 7)` (1.0; valid in 1.1) |
| `a = 1979-13-27` / `a = 0x` | `Expected newline or end of document after a statement (at line 1, column 9 / 6)` |
| `a = "\e"` | `Unescaped '\' in a string (at line 1, column 8)` (1.0; `\e` is valid in 1.1) |
| `a = 1\rb = 2` (bare CR) | `Expected newline or end of document after a statement (at line 1, column 6)` |

  Passing ✓: empty file, CRLF, and `a = 99999999999999999999`. **tomllib accepts arbitrary-size integers**, while spec-strict parsers reject values beyond i64.
- **Version caveat (pass/fail-affecting):** CPython ≤3.14 `tomllib` implements **TOML 1.0.0**; **CPython 3.15 `tomllib` implements TOML 1.1.0** ([tomllib docs, 3.15](https://docs.python.org/ru/3.15/library/tomllib.html)). Rust `toml`/`toml_parser` ≥1.1 implement spec 1.1.0, matching 3.15.
- **Epochs:** E1 v3.0.1–v4.2.0 uses the `toml` PyPI package (different grammar and messages): **DELEGATE**. E2 v4.3.0+ uses tomllib/tomli: native.
- **Native notes:**
  - Use `toml_parser` (event/lexer level, `+spec-1.1.0`) with snp's own semantic layer:
    - duplicate keys and table redefinition (`Cannot overwrite a value`, `Cannot declare (…) twice`, `Cannot mutate immutable namespace (…)`);
    - date validity (`Invalid date or datetime`);
    - **ints not limited to i64**.
  - Map the common errors to tomllib message texts; positions are best effort. A setting `toml_spec = "1.0" | "1.1"` (default `1.1`) rejects 1.1-only syntax when 1.0 parity is wanted.

#### `check-xml` — NATIVE-WITH-CAVEATS (P2)

- **Manifest:** `types: [xml]`; name `check xml`.
- **Algorithm** (`check_xml.py:8-22`): `xml.sax.parse(open(f,'rb'), ContentHandler())`. On `xml.sax.SAXException`, print `{filename}: Failed to xml parse ({exc})` and set rc=1.
- **`str(SAXParseException)`** is `{systemId}:{line}:{col}: {msg}`:
  - systemId = the filename;
  - line is 1-based; **col is 0-based and counts code points**, not bytes ✓ (`<a>ééé<b></a>` gives `1:11`; `<a>😀</b>` gives `1:6`);
  - CR, LF and CRLF all count as line breaks ✓.
- **Parser properties** (pyexpat via `xml.sax.expatreader`):
  - non-validating; **namespace processing off** (an undeclared prefix `<y:c/>` passes ✓);
  - **external general and parameter entities are not fetched** (`feature_external_ges` False, Python ≥3.7.1);
  - an external DTD `SYSTEM` reference passes ✓;
  - internal DTD entities are expanded, with **expat ≥2.4 amplification limits** ✓ (billion laughs gives `limit on input amplification factor (from DTD and entities) breached` at `1:649`).
- **Encodings:**
  - auto-detected UTF-8, UTF-16 (BOM ✓), the declared `encoding=`;
  - built-in UTF-8, UTF-16, ISO-8859-1, US-ASCII;
  - **plus any Python single-byte codec through pyexpat's unknown-encoding handler** (`windows-1252` ✓);
  - multibyte non-UTF codecs (`shift_jis`) raise `ValueError: multi-byte encodings are not supported`, a **crash** ✓.
- **Message catalogue** (✓ expat 2.6.1):

| input | output |
|---|---|
| `` | `01.xml: Failed to xml parse (01.xml:1:0: no element found)` |
| `<a>` | `(…:1:3: no element found)` |
| `<a></b>` | `(…:1:5: mismatched tag)` |
| `<a/><b/>` | `(…:1:4: junk after document element)` |
| `<a>\xff</a>` | `(…:1:3: not well-formed (invalid token))` |
| `<a>&nope;</a>` | `(…:1:3: undefined entity)` |
| `<a b="1" b="2"/>` | `(…:1:9: duplicate attribute)` |
| `\n<?xml version="1.0"?><a/>` | `(…:2:0: XML or text declaration not at start of entity)` |
| `<a>&#0;</a>` | `(…:1:3: reference to invalid character number)` |
| `<a>]]></a>`, `<!-- c -- d -->` | `not well-formed (invalid token)` |
| `<!-- only comment -->` | `(…:1:21: no element found)` |
| `<a/>\x00` | `(…:1:4: not well-formed (invalid token))` |

  Passing ✓: leading whitespace before the root, BOM, `encoding="latin-1"` with byte `\xe9`, internal entities, `&#x10FFFF;`, `<?xml version="1.1"?>`, CDATA.
- **Epochs:** identical v3.0.0 → v6.0.0. The behaviour depends on the system expat version (amplification thresholds, error texts).
- **Native notes:**
  - Build a non-validating well-formedness checker on `xmlparser` 0.13 (tokens, DTD declarations) plus snp's own logic: tag stack, attribute uniqueness, entity table with expat-like amplification accounting, char validity, single root, junk-after-root, declaration-at-start.
  - Encodings come from `encoding_rs` (single-byte families). Treat multibyte legacy encodings as the crash path.
  - Do **not** use `roxmltree`: it enforces namespaces and would reject the undeclared prefixes that expat accepts.
  - Messages use expat's phrases. Column = code points.
  - Ship as `auto` only after differential fuzzing (§9.9).

### 4.D Content scanners

#### `check-merge-conflict` — NATIVE (P0)

- **Manifest:** `types: [text]`; name `check for merge conflicts`.
- **CLI:** `--assume-in-merge` (store_true); `filenames *`.
- **Algorithm** (`check_merge_conflict.py:10-52`):
  1. `is_in_merge()` is **always evaluated first**: `git_dir = git rev-parse --git-dir` (crash outside a repo). In a merge if `exists(git_dir/MERGE_MSG) and (exists(git_dir/MERGE_HEAD) or exists(git_dir/rebase-apply) or exists(git_dir/rebase-merge))`.
     - ✓ `MERGE_MSG` + `CHERRY_PICK_HEAD` (cherry-pick conflict) is **not** detected, so the hook passes. Revert behaves the same way.
     - ✓ `MERGE_MSG` + `rebase-merge/` is detected.
     - In worktrees the git dir is `.git/worktrees/<name>` (v4.1.0+; before that the literal `.git`).
  2. If not in a merge and `--assume-in-merge` was not given: return 0 (no output) ✓.
  3. For each file, iterate the lines in binary (split after `\n`; a CR-only file is one line), numbered from 1. For each pattern in order `b'<<<<<<< '`, `b'======= '`, `b'=======\r\n'`, `b'=======\n'`, `b'>>>>>>> '`: if `line.startswith(pattern)`, print `{filename}:{i}: Merge conflict string {marker!r} found`, where marker = `pattern.strip().decode()`. The output is therefore `'<<<<<<<'`, `'======='` or `'>>>>>>>'` in **single quotes**. Then rc=1.
- **Verified ✓** with `--assume-in-merge`:
  - `<<<<<<< HEAD`, `=======\r\n`, `======= `, `>>>>>>> b` and `=======\n` are each reported.
  - `<<<<<<<` with no trailing space, `=======` at EOF without a newline, and `||||||| base` (the diff3 marker) are **not** reported.
  - Output example: `a.txt:1: Merge conflict string '<<<<<<<' found`.
- **Exit:** 0 or 1.
- **Epochs:**
  - E1 v3.0.0–v4.0.1: literal `.git` dir. Message `Merge conflict string "{pattern.decode()}" found in {filename}:{i+1}`. The pattern is **not stripped**, so the output contains the trailing space, or **a raw newline** for `=======\n`. There is no `=======\r\n` pattern.
  - E2 v4.1.0: `rev-parse --git-dir`, old message.
  - E3 v4.2.0+: as specified above.
- **Native notes:**
  - Compute the merge state once per run (git dir via gix or `git rev-parse` with the inherited env) and cache it.
  - Line scan with `memchr` over the whole file read in chunks, carrying the partial line over. Only the first 9 bytes of each line matter, so keep at most 9 bytes of a line prefix plus the line counter. Memory is O(chunk).

#### `detect-private-key` — NATIVE (P0)

- **Manifest:** `types: [text]`; name `detect private key`.
- **Algorithm** (`detect_private_key.py:6-38`):
  1. Read the whole file in binary.
  2. Flag the file if **any** of these 10 byte strings occurs anywhere:
     - `BEGIN RSA PRIVATE KEY`
     - `BEGIN DSA PRIVATE KEY`
     - `BEGIN EC PRIVATE KEY`
     - `BEGIN OPENSSH PRIVATE KEY`
     - `BEGIN PRIVATE KEY`
     - `PuTTY-User-Key-File-2`
     - `BEGIN SSH2 ENCRYPTED PRIVATE KEY`
     - `BEGIN PGP PRIVATE KEY BLOCK`
     - `BEGIN ENCRYPTED PRIVATE KEY`
     - `BEGIN OpenVPN Static key V1`
  3. **After all files** are scanned, print `Private key found: {filename}` for each flagged file, in processing order ✓. rc=1.
  
  If a later file crashes (e.g. it is unreadable), nothing is printed for the earlier hits.
- **Exit:** 0 or 1.
- **Epochs:** E1 v3.0.0–v4.0.1 has the first 8 patterns only. E2 v4.1.0+ has all 10 (`ENCRYPTED PRIVATE KEY`, `OpenVPN Static key V1` added).
- **Native notes:**
  - `aho_corasick::AhoCorasick` (leftmost-first, `MatchKind::Standard`) built once per profile.
  - Stream with `stream_find_iter` over a buffered reader, so memory is O(buffer) and the scan stops at the first match.
  - Buffer the messages and emit them at the end.

#### `detect-aws-credentials` — NATIVE-WITH-CAVEATS (P2; INI semantics)

- **Manifest:** `types: [text]`; name `detect aws credentials`.
- **CLI** (`detect_aws_credentials.py:92-113`):
  - `filenames nargs='+'` (required).
  - `--credentials-file CREDENTIALS_FILE`: `action='append'` with default `['~/.aws/config', '~/.aws/credentials', '/etc/boto.cfg', '~/.boto']`. **User values are appended to the defaults**, an argparse quirk, so the defaults are always scanned.
  - `--allow-missing-credentials`.
- **Algorithm:**
  1. `credential_files = set(args.credentials_file) ∪ {os.environ[v] for v in (AWS_CONFIG_FILE, AWS_CREDENTIAL_FILE, AWS_SHARED_CREDENTIALS_FILE, BOTO_CONFIG) if v in env}`. Present-but-empty variables are included as `''`.
  2. For each file `p`, `expanduser(p)`:
     - `~` expands via `HOME` on POSIX and `USERPROFILE` on Windows;
     - if the path does not exist, skip it;
     - `ConfigParser().read(path)` (locale encoding; unreadable files are silently skipped by `read()`);
     - **`MissingSectionHeaderError` is caught**, giving no keys from that file ✓.
     - Other parse errors **crash** ✓: `DuplicateSectionError` (`While reading from 'creds.ini' [line  3]: section 'a' already exists`), `DuplicateOptionError`, `ParsingError`.
     - For every section in `parser.sections()` (excludes `DEFAULT`, but lookups **fall back to `[DEFAULT]`** ✓) and each of `aws_secret_access_key`, `aws_security_token`, `aws_session_token`: `parser.get(section, var).strip()`. Option names are case-insensitive ✓ (`AWS_SECRET_ACCESS_KEY: ab` matched); `:` and `=` are both delimiters.
     - **BasicInterpolation** applies: a value containing `%` that is not `%%` or `%(name)s` gives `InterpolationSyntaxError` (**crash** ✓: `'%' must be followed by '%' or '(', found: '%b'`).
     - Non-empty values are added to the key set.
     - Multi-line continuation values are joined with `\n`. Inline `#`/`;` comments are **not** stripped (only full-line comments are). Empty lines are allowed inside values.
  3. Add the **non-empty** values (not stripped) of `AWS_SECRET_ACCESS_KEY`, `AWS_SECURITY_TOKEN` and `AWS_SESSION_TOKEN`.
  4. If there are no keys: with `--allow-missing-credentials` return 0. Else print `No AWS keys were found in the configured credential files and environment variables.\nPlease ensure you have the correct setting for --credentials-file` and **return 2** ✓.
  5. For each file (the whole file in binary) and each key (UTF-8 bytes), if `key in content`: record `(filename, key.decode()[:4].ljust(28, '*'))`, i.e. the first 4 characters padded with `*` to 28 characters ✓ (`SECR************************`; a 2-character key gives `ab**************************`).
  6. Print `AWS secret found in {filename}: {hidden}` for every finding; return 1.
- **Nondeterminism:** the key iteration order is Python **set order (hash-randomised per process)**, so the order of several findings within one file varies run to run in Python. snp: emit findings per file sorted by the key bytes. This is deterministic and does not affect pass/fail.
- **Exit:** 0, 1, or **2** (no keys).
- **Epochs:** identical v3.0.0 → v6.0.0.
- **Native notes:**
  - A `configparser` subset port: sections `[name]`; `DEFAULT` fallback; `optionxform = lower`; delimiters `=` and `:`; comment prefixes `#` and `;` at line start (after whitespace); continuation lines (indented); `strict` duplicate checks; BasicInterpolation validation **only for the 3 looked-up options** (only `get` interpolates).
  - Encoding: UTF-8, with the locale caveat on Windows. Read errors skip the file.
  - Scan files with Aho–Corasick over the key set.
  - Never print full secrets. Keep secrets in `zeroize`d buffers (optional).

#### `check-vcs-permalinks` — NATIVE (P2)

- **Manifest:** `types: [text]`; name `check vcs permalinks`.
- **CLI:** `--additional-github-domain ADDITIONAL_GITHUB_DOMAINS` (`append`, default `['github.com']`, so github.com is always included); `filenames *`.
- **Pattern per domain** (`check_vcs_permalinks.py:10-15`):
  ```
  rb'https://' + DOMAIN + rb'/[^/ ]+/[^/ ]+/blob/(?![a-fA-F0-9]{4,64}/)([^/. ]+)/[^# ]+#L\d+'
  ```
  - The **domain is inserted unescaped**, so `.` matches any byte ✓ (`https://githubXcom/...` is flagged).
  - It is a bytes regex: `\d` is ASCII, and `[^/ ]` matches any byte, including `\n`.
- **Algorithm:**
  - For each file, for each binary line (split after `\n`), numbered from 1, for each pattern **in domain order**: if `pattern.search(line)`:
    1. write `{filename}:{i}:` to stdout (text) and flush;
    2. write the **raw line bytes**, including the newline if any, to `stdout.buffer`.
  - A line matching two patterns (e.g. `--additional-github-domain github.com` duplicates the default) is **printed twice** ✓.
  - After all files, if any match: print an empty line, then `Non-permanent github link detected.`, then `On any page on github press [y] to load a permalink.`.
  - A last line without a newline is printed without one, so the next output continues on the same line ✓.
- **Matching semantics ✓:**
  - `blob/main/x.py#L1` is flagged.
  - `blob/abc1234/…` (4–64 hex characters) is not flagged.
  - `blob/v1.0/…` is not flagged, because a `.` cannot appear in the ref segment.
  - `blob/abc/…` (3 hex characters) is flagged.
  - `#Lx` and `/tree/` are not flagged.
- **Exit:** 0 or 1.
- **Epochs:**
  - E1 v3.0.0–v3.3.0: a single fixed regex `https://github.com/[^/ ]+/[^/ ]+/blob/master/[^# ]+#L\d+` (only `master` is flagged), no option.
  - E2 v3.4.0: domains option, `master` only.
  - E3 v4.0.0+: as specified above.
- **Native notes:**
  - `regex::bytes` has no look-around. Implement as: prefilter with `memchr::memmem("https://")`; at each candidate start, run an **anchored** match of the look-around-free regex `https://DOMAIN/[^/ ]+/[^/ ]+/blob/([^/. ]+)/[^# ]+#L[0-9]+` (with `(?-u)`), then reject when capture 1 matches `^[a-fA-F0-9]{4,64}$`.
  - This is equivalent because, for a literal domain, the match from a given start position is unique up to the `[^# ]+#L\d+` tail, which does not affect the capture.
  - If a user domain contains regex metacharacters other than `.`, use `fancy-regex` on a Latin-1 mapping of the line (byte b becomes char U+00b, a bijection; the pattern bytes are mapped the same way; classes stay ASCII via `(?-u:…)`). If even that is unsupported, DELEGATE.

### 4.E Git / filesystem checks

#### `check-added-large-files` — NATIVE (P0)

- **Manifest:** no `types` (default `[file]`, so symlinks and submodules are never passed); stages `[pre-commit, pre-push, manual]`; min 3.2.0; name `check for added large files`.
- **CLI:** `--enforce-all`; `--maxkb MAXKB` (`type=int`, default 500; Python `int()` parsing: `1_0` gives 10 ✓, ` 12 `, `+5`, `-1`, and Unicode digits are accepted); `filenames *`.
  - Bad int ✓: `check-added-large-files: error: argument --maxkb: invalid int value: 'abc'`, exit 2.
- **Algorithm** (`check_added_large_files.py:13-54`):
  1. `S = set(filenames)`.
  2. If `S` is non-empty, run `git check-attr filter -z --stdin` with `\0`-joined names. Remove the names whose `filter` attribute value is `lfs` ✓. A failure is a crash.
  3. Unless `--enforce-all`: `S &= added_files()`. This is the C-quoted, rename-aware staged-added list (§3). ✓ **Non-ASCII names are never checked** (`café.bin` of 2 KB passed with `--maxkb=1`; flagged with `--enforce-all`). ✓ A modified (not added) large file is not checked.
  4. For each f in `S` (**Python set iteration order, effectively random per run**): `kb = ceil(stat(f).st_size / 1024)` (stat follows symlinks). If `kb > maxkb`, print `{f} ({kb} KB) exceeds {maxkb} KB.` and set rc=1.
- **Verified ✓:**
  - 1024 bytes passes at `--maxkb=1`; 1025 bytes gives `(2 KB)`.
  - `--maxkb=-1` with an empty file gives `empty (0 KB) exceeds -1 KB.`
  - LFS-attributed `big.dat` is skipped.
- **Exit:** 0 or 1.
- **Epochs:**
  - E1 v3.0.0–v3.2.0: no `--enforce-all`; LFS via `git lfs status --json` (falls back to none when git-lfs is missing).
  - E2 v3.3.0–v4.0.1: `--enforce-all`, still `git lfs status`.
  - E3 v4.1.0+: `check-attr`.
  - Native: E3. Delegate E1 and E2, or emulate with `git lfs status --json` (§10 Q5).
- **Native notes:**
  - Only metadata is needed. `kb = size.div_ceil(1024)` (u64; equals Python's float ceil below 2^53).
  - Emit in input (shuffled) order. The Python order is nondeterministic anyway.
  - Share the cached `added_files` result across hooks in a run (§9.6).

#### `check-case-conflict` — NATIVE (bug-compat; P1)

- **Manifest:** default types; name `check for case conflicts`.
- **Algorithm** (`check_case_conflict.py:12-56`):
  1. `repo = set(git ls-files .splitlines())` (C-quoted, non-`-z`).
  2. `repo |= parents(repo)`. Parents of `a/b/c` are `a/b` and `a`, split on `/`.
  3. `rel = set(filenames) | added_files()`, then `rel |= parents(rel)`. **Staged-added files are included even when they were not passed** ✓.
  4. `repo -= rel`.
  5. `conflicts = lower(repo) ∩ lower(rel)`. `lower` is Python **`str.lower()`**: full Unicode lowercase, final-sigma aware. Rust `str::to_lowercase` implements the same mapping (§10 Q8 on Unicode version).
  6. Duplicates within `rel`: iterate; the second item with an already-seen lowercase form is added to `conflicts`.
  7. If there are conflicts: for each `x` in `sorted(x for x in repo ∪ rel if x.lower() in conflicts)` (code-point order), print `Case-insensitivity conflict found: {x}`; rc=1.
- **Verified ✓:**
  - Committed `Dir/x`, `README`; added `readme`, `dir/y`, `new/A.txt`, `new/a.txt`, `NEW/b` → the lines `Dir`, `NEW`, `README`, `dir`, `new`, `new/A.txt`, `new/a.txt`, `readme`, in this order.
  - Committed `Ä.txt` plus added `ä.txt`: **passes** with the default `core.quotePath`; fails with `quotePath=false` (`Ä.txt`, `ä.txt`).
- **Exit:** 0 or 1.
- **Partition note:** every partition sees all staged-added files, so conflicts among added files are printed **once per partition** ✓ (9× in the probe).
- **Epochs:** E1 v3.0.0–v3.4.0 does not include directories (no `parents`). E2 v4.0.0+.
- **Native notes:** run the same two git commands (cached per run). Lowercase via `str::to_lowercase` on the *decoded* output (strict UTF-8 decode; invalid output is a crash in Python). Use `BTreeSet<String>` for sorted output.

#### `check-executables-have-shebangs` — NATIVE (P0/P1)

- **Manifest:** `types: [text, executable]`. identify: `executable` = `os.access(X_OK)`, which is **always true on Windows**. Stages `[pre-commit, pre-push, manual]`; min 3.2.0; name `check that executables have shebangs`.
- **Algorithm** (`check_executables_have_shebangs.py:17-73`):
  1. `v = git config core.fileMode` (stdout `.strip()`, exit code ignored).
  2. **If `v == 'false'`** (exact string; the default on Windows clones): for each entry of `git ls-files -z --stage -- <paths>`, parse `"<mode> <sha> <stage>\t<path>"`. If any of the last 3 digits of the mode is in `{1,3,5,7}` (index mode executable) and the file does not start with `#!`, print the message for **every ls-files entry**: an unmerged path can be printed up to 3×. Untracked paths and pathspec-glob surprises are as in §3.
  3. Otherwise: for every path, if the file does not start with `#!` (first 2 bytes; files shorter than 2 bytes fail), print the message.
  4. rc=1 if any.
- **Message** (to **stderr**; `shlex.quote` POSIX quoting even on Windows) ✓:
  ```
  {path}: marked executable but has no (or invalid) shebang!
    If it isn't supposed to be executable, try: `chmod -x {q}`
    If on Windows, you may also need to: `git add --chmod=-x {q}`
    If it is supposed to be executable, double-check its shebang.
  ```
  `q = shlex.quote(path)`, e.g. `'c d'"'"'s.sh'` ✓. With fileMode false, `m.sh` (index 644, filesystem +x) is not reported ✓.
- **Exit:** 0 or 1.
- **Epochs:**
  - E1 v3.1.0–v4.0.1: `sys.platform == 'win32'` selects the git path; message without the "If on Windows" line.
  - E2 v4.1.0–v4.2.0: platform check; message with the Windows line.
  - E3 v4.3.0+: `core.fileMode`.
  - v3.0.0: no git path at all.
  - Each epoch is reproducible: the win32 selector becomes `cfg!(windows)`.
- **Native notes:**
  - Read 2 bytes only.
  - Batch `git ls-files` calls with `--stdin`-free chunking (Windows command-line limit; prek hit "command line too long"). snp invokes per partition, which is already length-limited.
  - Parity of pathspec magic: pass the paths exactly as Python does (no `:(literal)`).

#### `check-shebang-scripts-are-executable` — NATIVE (P1)

- **Manifest:** `types: [text]`; stages as above; min 3.2.0; name `check that scripts with shebangs are executable`.
- **Algorithm:** **always** the git path: `git ls-files -z --stage -- <paths>`. For each entry: if not index-executable and the file starts with `#!`, print the message (stderr) per entry. rc=1 if any. Untracked files (e.g. `git rm --cached`) are ignored ✓.
- **Message v6.0.0+** ✓ (note the trailing `\n` inside the string, plus print's newline, which gives a blank line between entries):
  ```
  {path}: has a shebang but is not marked executable!
    If it is supposed to be executable, try: `chmod +x {q}`
    If on Windows, you may also need to: `git add --chmod=+x {q}`
    If it is not supposed to be executable, double-check its shebang is wanted.

  ```
- **Epochs (message only):**
  - v4.0.0–v4.2.0: no "If on Windows" line, and `If it not supposed…` (sic).
  - v4.3.0–v5.0.0: Windows line plus `If it not supposed…` (sic).
  - v6.0.0+: `If it is not supposed…`.

#### `check-symlinks` — NATIVE (P1)

- **Manifest:** `types: [symlink]`; name `check for broken symlinks`.
- **Algorithm:** for each file, if `os.path.islink(f) and not os.path.exists(f)`, print `{f}: Broken symlink`; rc=1. `exists` is False for *any* stat error, so symlink loops count as broken ✓ (`loop1: Broken symlink`).
- **Native:** `symlink_metadata(f).is_ok_and(|m| m.file_type().is_symlink()) && fs::metadata(f).is_err()`. On Windows, use the same calls (symlinks need privileges or developer mode; junctions are not symlinks to Python ≤3.11. Python 3.12 `islink` is still False for junctions.)
- **Epochs:** identical.

#### `destroyed-symlinks` — NATIVE (P1)

- **Manifest:** `types: [file]`; stages `[pre-commit, pre-push, manual]` (v5.0.0+); name `detect destroyed symlinks`.
- **Algorithm** (`destroyed_symlinks.py:16-66`):
  1. If no files are given: return [].
  2. Run `git status --porcelain=v2 -z -- <files>` and `zsplit` it.
  3. For records `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path…>`:
     - split on spaces and rejoin the tail with spaces, so paths with spaces work ✓;
     - consider it if `mH == '120000' and mI not in ('120000','000000')`;
     - if `hH == hI`: destroyed;
     - else compare sizes `si = git cat-file -s hI` and `sH = git cat-file -s hH`. If `si <= sH + 2` and `cat-file -p hH`.rstrip() == `cat-file -p hI`.rstrip() (bytes rstrip, ASCII-WS): destroyed.
     
     Rename records (`2 …`) produce an extra NUL field containing the original path. That field is then mis-parsed as a separate record, which matters only if it begins with `1 `.
  4. If any are destroyed, print ✓:
     ```
     Destroyed symlinks:
     - {path}            (one line per path)
     You should unstage affected files:
     \tgit reset HEAD -- {shlex.join(paths)}
     And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:
     \tgit config core.symlinks false
     ```
     and return 1. `\t` is a real tab.
- **Native:** equivalent to comparing **HEAD tree vs index** entries for the given paths. Use gix (HEAD tree lookup plus index entry). For blob reads, use gix object reads; the result is the same as `cat-file`. **Pathspec semantics:** run the same `git status` to preserve glob matching, or use gix with literal paths (documented divergence only for glob characters in names; prefer parity: use git).
- **Epochs:** identical v3.4.0+.

#### `forbid-new-submodules` — NATIVE (P2)

- **Manifest:** `types: [directory]` (v4.4.0+). identify tags a gitlink/submodule dir as `directory`. Name `forbid new submodules`.
- **Algorithm** (`forbid_new_submodules.py:10-44`):
  1. `diff_arg` = `{PRE_COMMIT_FROM_REF}...{PRE_COMMIT_TO_REF}` if **both** env vars are present (the hook's environment, including user-exported ones), else `--staged`.
  2. Run `git diff --diff-filter=A --raw <diff_arg> -- <files>`.
  3. For each line `":000000 160000 0000000 abc1234 A\tpath"`: if `split(' ')[1] == '160000'`, print `{path}: new submodule introduced` (a quoted path stays quoted).
  4. If any, print ✓:
     ```

     This commit introduces new submodules.
     Did you unintentionally `git add .`?
     To fix: git rm {thesubmodule}  # no trailing slash
     Also check .gitmodules
     ```
     (a leading empty line; `{thesubmodule}` is literal text). Return 1.
- **Epochs:** E1 v3.0.0–v4.0.1: `git diff --staged --diff-filter=A --raw` with **no** pathspec and no ref env; argv ignored. E2 v4.1.0+: as above.

#### `no-commit-to-branch` — NATIVE (P0)

- **Manifest:** `pass_filenames: false`, `always_run: true` (so `files`/`types` are ignored unless the user sets `always_run: false`); name `don't commit to branch`.
- **CLI:** `-b/--branch` (`append`; default when absent `{'master','main'}`); `-p/--pattern` (`append`, regex). There is no positional: **any extra token** (e.g. filenames, if the user sets `pass_filenames: true`) gives `error: unrecognized arguments: …`, exit 2.
- **Algorithm:**
  1. `ref = git symbolic-ref HEAD`. Failure (detached HEAD) means not protected, return 0 ✓.
  2. `branch = '/'.join(ref.strip().split('/')[2:])` (`refs/heads/feature/x` gives `feature/x`).
  3. Return `int(branch in protected or any(re.match(p, branch) for p in patterns))`.
  - **`re.match` is anchored at the start only** (prefix match): `--pattern rel` matches `release-1/x` ✓.
  - Python `re` syntax and Unicode semantics.
  - Patterns are compiled lazily, and only when the name is not already in `protected` (short-circuit). An invalid regex is therefore a crash only when reached.
- **Output: none.** Exit 1 on a protected branch (pre-commit shows only `- exit code: 1`) ✓. An unborn branch on `main` gives 1.
- **Epochs:** E1 v3.0.0–v3.4.0: default `{'master'}`. E2 v4.0.0+: `{'master','main'}`. `--pattern` exists since v2.2.0.
- **Native notes:**
  - Read HEAD via gix, honouring `GIT_DIR` and worktrees: symbolic target or detached.
  - Regex: `fancy-regex` (Python-like backtracking, look-around, backrefs) compiled as `\A(?:{p})`. If the pattern uses syntax that fancy-regex rejects but Python accepts (e.g. `(?P=name)`, possessive quantifiers before 3.11, `\Z` vs `\z` differences), DELEGATE the hook.

#### `name-tests-test` — NATIVE (P2)

- **Manifest:** `files: (^|/)tests/.+\.py$`; name `python tests naming`.
- **CLI** (v4.3.0+): a mutually exclusive group writing `dest=pattern`:
  - `--pytest` (const and default `.*_test\.py`);
  - `--pytest-test-first` (`test_.*\.py`);
  - `--django`/`--unittest` (`test.*\.py`).
  
  Mutex error ✓: `name-tests-test: error: argument --django/--unittest: not allowed with argument --pytest-test-first`.
- **Algorithm:**
  - `base = os.path.basename(f)` (`/`; also `\` on Windows).
  - If `not re.fullmatch(pattern, base)` and `base not in ('__init__.py', 'conftest.py')`, print `{f} does not match pattern "{pattern}"` (the raw regex text, e.g. `.*_test\.py`) ✓; rc=1.
  - `.` does not match `\n`.
- **Epochs:** E1 v3.0.0–v4.2.0: only `--django` (store_true); **`re.match` (prefix)**, so `foo_test.pyc` would pass. E2 v4.3.0+: `fullmatch`.
- **Native:** hand-coded matchers. These three are fixed patterns, so no regex engine is needed:
  - `.*_test\.py` = no `\n` and ends with `_test.py`;
  - `test_.*\.py` = starts with `test_`, ends with `.py`, length ≥ 8, no `\n`;
  - `test.*\.py` = starts with `test`, ends with `.py`, length ≥ 7, no `\n`.
  - For `re.match` (E1), prefix semantics.

### 4.F `language: fail` hooks (no Python; only the manifest is needed)

These run in snp's generic `fail` language (see the languages spec). The output format there is the entry, a blank line, then one filename per line ✓.

- **`check-illegal-windows-names`** ✓. Output:
  ```
  Illegal Windows filenames detected

  COM¹
  a/aux/b
  con.txt
  trail.
  x:y
  ```
  - The `files` regex must be evaluated with **Python `re` semantics** (str pattern): `(?i)` uses Unicode simple case folding; `\d` is Unicode Nd; `\s` is PY-STR-WS (it also includes `\x1c-\x1f`; those are covered by `[\x00-\x1F]` anyway).
  - `¹²³` are matched literally. `COM¹` is flagged ✓.
  - Quirk: `dir /f` (a top-level directory ending in a space) is **not** flagged, because `/[^/]*[\.\s]/` needs a leading `/`.
  - Rust `regex` (Unicode mode) gives equivalent results for this pattern; the engine choice belongs to the config spec.
- **`forbid-submodules`** (`types: [directory]`). Output ✓:
  ```
  submodules are not allowed in this repository:

  sub
  ```

With Tier A (§6), these run without cloning the repo.

### 4.G DELEGATE: hooks that need CPython's parser or tokenizer

They are documented for completeness; snp runs the upstream hook. Reasons to delegate: the acceptance set and positions depend on the **user's CPython version grammar** (3.8–3.15 differ: PEP 701 f-strings, `match`, `type` statements, …), and the output embeds CPython tracebacks.

| id | prog | algorithm | output | exit |
|---|---|---|---|---|
| `check-ast` | `check-ast` | `ast.parse(bytes, filename)` | On SyntaxError: `{f}: failed parsing with {impl} {ver}:` (e.g. `CPython 3.12.3`), then `\n` + the traceback indented by 4 spaces ✓ | 0/1 |
| `check-builtin-literals` | `check-builtin-literals` | AST visit: `Call` whose `func` is a `Name` in `{complex,dict,float,int,list,str,tuple}` minus `--ignore` (comma set), with no positional args; `dict` with kwargs allowed unless `--no-allow-dict-kwargs`. Main (unreleased): nested calls are visited too | `{f}:{line}:{col}: replace {name}() with {literal}` (literals `0j {} 0.0 0 [] '' ()`; col = UTF-8 byte offset) ✓. SyntaxError is a crash | 0/1 |
| `check-docstring-first` | `check-docstring-first` | tokenize: a STRING token at col 0 after code or after another docstring | `{f}:{line}: Module docstring appears after code (code seen on line {n}).` / `{f}:{line}: Multiple module docstrings (first docstring on line {n}).` ✓ | 0/1 |
| `debug-statements` | `debug-statement-hook` | AST: `import`/`from` of `{bpdb, ipdb, pdb, pdbr, pudb, pydevd_pycharm, q, rdb, rpdb, wdb}` (`bpdb` since v4.5.0); `breakpoint()` calls | `{f}:{line}:{col}: {name} imported` / `… breakpoint called` ✓. Parse failure: `{f} - Could not parse ast`, a blank line, the traceback indented with a tab, a blank line ✓ (exit 1, continues) | 0/1 |
| `double-quote-string-fixer` | same | tokenize; rewrites `"x"` as `'x'` unless the body has quotes or the string is triple-quoted; skips f-strings on 3.12+ (FSTRING_START); UTF-8 text `newline=''` | `Fixing strings in {f}` ✓ | 0/1 |

Future option (§10 Q10): a NATIVE-WITH-CAVEATS `debug-statements`/`check-builtin-literals` built on a Rust Python parser (e.g. `ruff_python_parser`, which is not published on crates.io, so it would have to be vendored). Not recommended for v1.

### 4.H Removed and stub hooks

- **`pre-commit-hooks-removed`** (`removed.py:7-12`): `hookid, new_hookid, url = argv[:3]`. It raises `SystemExit` with the message below, which goes to **stderr** with exit **1** ✓. Any extra `args` and filenames are ignored. Fewer than 3 argv items is a crash (`ValueError`).
  ```
  `{hookid}` has been removed -- use `{new_hookid}` from {url}
  ```
  - v6.0.0 ids: `check-byte-order-marker` gives `` `check-byte-order-marker` has been removed -- use `fix-byte-order-marker` from https://github.com/pre-commit/pre-commit-hooks ``; `fix-encoding-pragma` gives `` `fix-encoding-pragma` has been removed -- use `pyupgrade` from https://github.com/asottile/pyupgrade `` ✓.
  - v3.x stubs `autopep8-wrapper`, `flake8` and `pyflakes` use the same mechanism with `always_run: true`, `pass_filenames: false`.
  - NATIVE: trivial.
- **`check-byte-order-marker` ≤ v5.0.0** (real hook): for each file, if its first 3 bytes are `EF BB BF`, print `{f}: Has a byte-order marker`; rc=1. NATIVE.
- **`fix-encoding-pragma` ≤ v5.0.0** (real fixer, `pch.git` `v5.0.0:pre_commit_hooks/fix_encoding_pragma.py`). NATIVE (P3) or DELEGATE.
  - v4.6.0–v5.0.0 first print to **stderr**: `warning: this hook is deprecated and will be removed in a future release because py2 is EOL. instead, use https://github.com/asottile/pyupgrade`.
  - CLI: `--pragma` (default `# -*- coding: utf-8 -*-`; the value is `.encode().rstrip()`); `--remove`.
  - Byte algorithm:
    1. `first = readline()`, `second = readline()`, `rest = read()`; `ending = \r\n if first.endswith(b'\r\n') else \n`.
    2. A shebang is the first line if it starts with `#!`; then the pragma candidate is line 2, else line 1.
    3. Status: `True` if `candidate.rstrip(b'\r\n') == pragma`; `None` if `has_coding(candidate)` (non-blank, starts with `#` after lstrip, and contains `unicode`, `encoding`, `coding:` or `coding=`); else `False`. When False, the candidate goes back into `rest`.
    4. If `rest.strip()` is empty: if a pragma or shebang is present, truncate the file to **empty**, return 1; else return 0.
    5. If the status equals `not remove`, return 0.
    6. Otherwise rewrite `shebang + (pragma+ending unless remove) + rest`. Note: a mismatching pragma (`None`) is *replaced*.
  - Messages: `Added \`{pragma}\` to {f}` / `Removed encoding pragma from {f}`.

---

## 5. Version epochs (behaviour by tag) and the data snp must embed

### 5.1 Tag → commit (frozen-SHA pins such as `rev: 3e8a87…  # frozen: v6.0.0` must resolve without network)

| tag | commit | date |
|---|---|---|
| v3.0.0 | 66250ba9bf5ae48f660b7648d9feca8ed0b18e58 | 2020-05-14 |
| v3.0.1 | 0e2c0f75028c039052b637c17173833aaf5be97a | 2020-05-16 |
| v3.1.0 | ebc15addedad713c86ef18ae9632c88e187dd0af | 2020-05-20 |
| v3.2.0 | e1668fe86af3810fbca72b8653fe478e66a0afdc | 2020-07-30 |
| v3.3.0 | 9136088a246768144165fcc3ecc3d31bb686920a | 2020-10-20 |
| v3.4.0 | 6e2418c5521b7d606e72914dced3253f9ace1205 | 2020-12-15 |
| v4.0.0 | 9cc62e1a631dd72a054fb2dccda6fdebd99aacab | 2021-05-14 |
| v4.0.1 | 38b88246ccc552bffaaf54259d064beeee434539 | 2021-05-16 |
| v4.1.0 | 8fe62d14e0b4d7d845a7022c5c2c3ae41bdd3f26 | 2021-12-23 |
| v4.2.0 | db7346d375eda68a0174f2c057dd97f2fbffe030 | 2022-04-06 |
| v4.3.0 | 3298ddab3c13dd77d6ce1fc0baf97691430d84b0 | 2022-06-07 |
| v4.4.0 | f71fa2c1f9cf5cb705f73dffe4b21f7c61470ba9 | 2022-11-23 |
| v4.5.0 | c4a0b883114b00d8d76b479c820ce7950211c99b | 2023-10-07 |
| v4.6.0 | 2c9f875913ee60ca25ce70243dc24d5b6415598c | 2024-04-06 |
| v5.0.0 | cef0300fd0fc4d2a87a85fa2093c6b283ea36f4b | 2024-10-05 |
| v6.0.0 | 3e8a8703264a2f4a69428a0aa4dcb512790b2c8c | 2025-08-09 |

All tags are lightweight tags on commits. Generate this table, plus the manifests and blob ids, with an `xtask` from a full clone; do not hand-maintain it.

### 5.2 Behaviour epochs per native hook (v3.0.0 floor; anything older is DELEGATE)

| hook | epochs (inclusive tag ranges) and differences |
|---|---|
| trailing-whitespace | one epoch v3.0.0–v6.0.0 |
| end-of-file-fixer | one epoch |
| mixed-line-ending | one epoch (manifest `stages` added on main) |
| fix-byte-order-marker | one epoch (v3.3.0+; absent before) |
| check-byte-order-marker | real check v3.0.0–v5.0.0; removed stub v6.0.0+ |
| fix-encoding-pragma | real v3.0.0–v4.5.0; real plus stderr deprecation warning v4.6.0–v5.0.0; stub v6.0.0+ |
| file-contents-sorter | E1 v3.0.0–v4.4.0 (empty → `\n`; `--ignore-case` from v3.3.0, `--unique` from v3.4.0); E2 v4.5.0–v5.0.0; E3 v6.0.0+ (mutex) |
| requirements-txt-fixer | v3.0.0 DELEGATE; E1 v3.1.0–v3.4.0; E2 v4.0.0–v4.5.0; E3 v4.6.0; E4 v5.0.0+ (manifest `files` widened in v4.5.0) |
| sort-simple-yaml | one epoch |
| check-json | E1 v3.0.0–v3.4.0 (no duplicates); E2 v4.0.0+ |
| pretty-format-json | <v4.0.0 DELEGATE; E1 v4.0.0–v4.6.0 (abort on first invalid); E2 v5.0.0+ |
| check-yaml | one epoch (ruamel); messages depend on ruamel version |
| check-toml | v3.0.x–v4.2.0 DELEGATE (`toml` package); v4.3.0+ native |
| check-xml | one epoch |
| check-merge-conflict | E1 v3.0.0–v4.0.1; E2 v4.1.0; E3 v4.2.0+ (§4.D) |
| detect-private-key | E1 v3.0.0–v4.0.1 (8 patterns); E2 v4.1.0+ (10) |
| detect-aws-credentials | one epoch |
| check-vcs-permalinks | E1 v3.0.0–v3.3.0; E2 v3.4.0; E3 v4.0.0+ |
| check-added-large-files | E1 v3.0.0–v3.2.0 (**the sample-config pin**; `added ∩ files − lfs-status`); E2 v3.3.0–v4.0.1 (`--enforce-all`; lfs via `git lfs status --json`); E3 v4.1.0+ (`check-attr`) |
| check-case-conflict | E1 v3.0.0–v3.4.0; E2 v4.0.0+ |
| check-executables-have-shebangs | v3.0.0 (no git path); E1 v3.1.0–v4.0.1; E2 v4.1.0–v4.2.0; E3 v4.3.0+ |
| check-shebang-scripts-are-executable | v4.0.0–v4.2.0; v4.3.0–v5.0.0; v6.0.0+ (message only) |
| check-symlinks, destroyed-symlinks (v3.4.0+) | one epoch each |
| forbid-new-submodules | E1 v3.0.0–v4.0.1; E2 v4.1.0+ |
| no-commit-to-branch | E1 v3.0.0–v3.4.0 (`master`); E2 v4.0.0+ (`master`,`main`) |
| name-tests-test | E1 v3.0.0–v4.2.0 (`--django` only, `re.match`); E2 v4.3.0+ |

Post-v6.0.0 main (`ccfd994`, unreleased): check-builtin-literals nested-call fix (delegated anyway); check-docstring-first name; mixed-line-ending stages. A future tag automatically works in Tier B through blob matching, and is added to Tier A by the next snp data refresh.

### 5.3 Manifest differences that change the *displayed* output (Tier A must embed per-tag manifests)

- **Names:** v4.0.x and older use Title Case and different texts, e.g. `Trim Trailing Whitespace`, `Fix End of Files`, `Check Yaml`, `Check JSON`, `Check for added large files`, `Debug Statements (Python)`, `Tests should end in _test.py`, `Don't commit to branch`, `check BOM - deprecated: use fix-byte-order-marker` (v4.0.x). v4.1.0+ (PR #671/#673) use lowercase: `trim trailing whitespace`, `fix end of files`, …. v6.0.0 changed `check-byte-order-marker (removed)` and `fix python encoding pragma (removed)`. pre-commit prints `name` in the status line, so it must be exact.
- **stages:**
  - before v5.0.0, `[commit, push, manual]` (old stage names, mapped by snp's stage migration) is used by:
    - `trailing-whitespace`, `end-of-file-fixer` and `check-executables-have-shebangs` (already in v3.0.0);
    - `check-shebang-scripts-are-executable` (v4.0.0);
    - `check-added-large-files` (v4.3.0);
  - v5.0.0+ use `[pre-commit, pre-push, manual]` plus `minimum_pre_commit_version: 3.2.0`;
  - `destroyed-symlinks` gained stages in v5.0.0;
  - `mixed-line-ending` gained stages on main.
- **types/files:**
  - `forbid-new-submodules` `types: [directory]` and the new `forbid-submodules` hook appear in v4.4.0;
  - `requirements-txt-fixer` `files` widened in v4.5.0;
  - `check-illegal-windows-names` is new in v5.0.0;
  - `check-shebang-scripts-are-executable`, `destroyed-symlinks` and `fix-byte-order-marker` appear in the v4.0.0 manifest (the scripts existed earlier for some).

### 5.4 v6.0.0 module blob ids (seed of the Tier-B verified table; `git ls-tree v6.0.0 pre_commit_hooks/`)

| module | blob | module | blob |
|---|---|---|---|
| util.py | d6c90ae02f7b07f10232fb6d00fb6d2bf9e8784f | check_added_large_files.py | e67416238d28bd20be4a0d0406a6ca87ded486da |
| check_case_conflict.py | 475c91c4308e5c22f108f2280fb66d83f2a48863 | check_executables_have_shebangs.py | 707863b32f401cc50a112f003955fcad2082b034 |
| check_json.py | 612111c531d9a1413cdc4d62def2a8fb915092c2 | check_merge_conflict.py | 54a083ee5e45af91a43f23f642ccece4dda7c0d4 |
| check_shebang_scripts_are_executable.py | 937425b0843e5f42a2d4a8dfc751470e1f6850c4 | check_symlinks.py | be8a800e84969dce1e7cf3eb8cf45809f9174532 |
| check_toml.py | 2105b072e99cca264ace2884dd593dc1c1ab078d | check_vcs_permalinks.py | 108656aad58b84386efe84ce13da2e527a0c6da8 |
| check_xml.py | ff5536b5857a17cc290c40fbc7bb017ed6112d3c | check_yaml.py | c94ea7168082d4e6f4e85464d48fdaa84a2fbe30 |
| destroyed_symlinks.py | 9bc25898aaba42f1e15bac73c3efecd74fa47824 | detect_aws_credentials.py | 85822886e2bdfef344d77de3cc716a4011c582af |
| detect_private_key.py | 9ad703ae6a714a3f09322a270303b1d0950ff891 | end_of_file_fixer.py | a88425c6815d0e876ed3168ca93bd1f630b531b9 |
| file_contents_sorter.py | 4435d6a61ddc03b7f7c9b6c22150029df7b55197 (main: ee26d92640…, import-only change) | fix_byte_order_marker.py | 100ffeac338e71c8c8e59ec001e4abe5997e85ac |
| forbid_new_submodules.py | b7a63cd25df4a9295fa6835e2b8833fd0d0e16df | mixed_line_ending.py | 2fbf067fc96bd11623a9d69b33af652e7c1031d9 |
| no_commit_to_branch.py | b0b8b23803222c68abbc1ea28474fe58aba73c64 | pretty_format_json.py | 501f37f7f7a6d9384b1d81f55295d28d5d0a6c29 |
| removed.py | fb2b6d98c6c0823b7b03b69dfa52ee23b8bb6088 | requirements_txt_fixer.py | 8ce8ec643a233bffb209e3eb2c384314adb23a9a |
| sort_simple_yaml.py | 65e6b7a635b570d6c91227a41774b19e8bb78d1c | tests_should_end_in_test.py | 07af277dfd8be0c7d0f04c76b5b9bc7ca01377e6 |
| trailing_whitespace_fixer.py | dab8b14a83b50a0d6e327d95aa84e1e4fcc9a72e | `.pre-commit-hooks.yaml` | a98a1e574bb3b8d2255f7215a97d6b15e1210a2e |

Blob ids change with cosmetic edits: typing, `exit`→`raise SystemExit`, f-strings. Across v3.0.0–v6.0.0 each module has 3–9 distinct blobs (`tmp-builtin/blobs.txt`). The xtask maps **every** historical blob to its behaviour epoch, so that Tier B also covers forks that track upstream exactly.

Module import dependencies to verify together:
- `check_added_large_files` and `check_case_conflict` → `util`;
- `check_executables_have_shebangs` → `util`;
- `check_shebang_scripts_are_executable` → `check_executables_have_shebangs` (→ `util`);
- `check_merge_conflict`, `destroyed_symlinks`, `forbid_new_submodules`, `no_commit_to_branch` → `util`.

---
## 6. Substitution design (automatic fast path)

### 6.1 Goals and invariants

1. A config that works with pre-commit works unchanged, and produces the same pass/fail, the same file modifications and (in `exact` mode, for NATIVE hooks) the same bytes of output.
2. Substitution is **never** a security downgrade. Native code is snp's own vetted code, never code fetched from the network. Tier A runs nothing from the remote repo at all.
3. Deterministic and explainable: every decision has a reason, visible with `SNP_LOG=debug` or `snp doctor fast-path`.
4. Version-aware: the pinned `rev` selects a behaviour profile. An unknown or modified implementation is delegated.

### 6.2 Repo identity (`is_pre_commit_hooks(url)`)

Normalise the `repo:` string. This is for matching only; the clone URL stays verbatim:
1. Reject `local`, `meta` and `builtin`.
2. Parse one of these forms:
   - `scheme://[userinfo@]host[:port]/path`, with scheme ∈ {`https`, `http`, `ssh`, `git`, `git+ssh`};
   - scp-like `[user@]host:path` (no scheme; the host has no `/` before the `:`).
3. host: lowercase, strip a leading `www.`; it must equal `github.com`. Port: allow none, 443 for https, or 22 for ssh.
4. path:
   - strip a leading `/`, **one** trailing `/`, then **one** trailing `.git` (case-insensitive);
   - split into exactly 2 segments, compared **ASCII case-insensitively** to `pre-commit/pre-commit-hooks` (GitHub routes case-insensitively);
   - no query or fragment.
5. Never log userinfo (tokens).

Matches: `https://github.com/pre-commit/pre-commit-hooks`, `…-hooks.git`, `…-hooks/`, `git@github.com:pre-commit/pre-commit-hooks.git`, `ssh://git@github.com/pre-commit/pre-commit-hooks`, `https://GitHub.com/Pre-Commit/Pre-Commit-Hooks`.

Non-matches (they go to Tier B when cloned): forks, GitLab or Gitea mirrors, `file://` and local paths, `https://github.com/pre-commit/pre-commit-hooks/tree/main`.

### 6.3 Profiles: Tier A (no clone) and Tier B (verified clone)

```
resolve_profile(repo, rev):
  if is_pre_commit_hooks(repo.url):
      if rev in KNOWN_TAGS:                       -> TierA(profile[rev])         # exact string, e.g. "v6.0.0"
      if rev is 40-hex and rev in TAG_COMMITS:    -> TierA(profile[tag_of(rev)]) # frozen pins
  # anything else: abbreviated SHA, branch, unknown or new tag, fork, mirror, local path
  -> TierB (decided after the normal clone at `rev`)
```

- **Tier A data, embedded in the binary:** per tag, the full manifest (verbatim YAML, parsed with the normal manifest schema) and a map `hook id → (native impl, epoch)`.
- **Tier B check**, run after the clone that pre-commit would also perform (no venv yet). For each candidate hook id:
  1. Read `.pre-commit-hooks.yaml` from the clone. The hook's `entry` and `language` must equal a known `(entry, language)` pair.
  2. Map entry to module through the known console-script table (from `setup.cfg` `[options.entry_points]`, e.g. `trailing-whitespace-fixer` → `trailing_whitespace_fixer`). A clone whose `setup.cfg` maps the entry elsewhere is a mismatch, so DELEGATE.
  3. `git ls-tree <rev> pre_commit_hooks/` gives the blob ids. The module blob **and its import closure** (§5.4) must all be in `VERIFIED_BLOBS[module] → epoch`. This needs no checkout of file contents.
  4. All good: Native(epoch). Else DELEGATE (reason `implementation differs from verified upstream blobs`).
- Tier B also covers forks and mirrors that carry byte-identical upstream modules. The manifest comes from the clone, so renamed hooks and custom names are honoured.

### 6.4 Per-hook gating (all must hold for NATIVE)

| # | condition | on failure |
|---|---|---|
| G1 | fast path enabled: no `--no-fast-path`; `SNP_NO_FAST_PATH` unset/`0`/`false`; setting `fast_path != "off"`; id ∉ `fast_path_exclude` | DELEGATE "disabled by user" |
| G2 | profile resolved (Tier A, or Tier B verified) | DELEGATE "unknown implementation" |
| G3 | the hook's class is allowed by the mode: `exact` allows NATIVE only; `auto` (default) adds NATIVE-WITH-CAVEATS | DELEGATE "mode" |
| G4 | the user's hook config does **not** contain the key `language` (any value), `entry`, or a non-empty `additional_dependencies` | DELEGATE "user override (`language`/`entry`/`additional_dependencies`)" |
| G5 | `preflight(args)` returns Ok. Argparse *errors* are **not** a preflight failure: they are emulated with exit 2. Only unsupported features fail preflight, e.g. a regex the engine cannot compile under Python semantics | DELEGATE "unsupported argument" |
| G6 | the epoch is supported (≥ v3.0.0, and not one of the DELEGATE epochs in §5.2) | DELEGATE "old version" |

Notes:
- `language_version` (hook-level or `default_language_version.python`) does **not** disable the fast path. Python-version-dependent messages follow the emulated CPython version (§10 Q2).
- Runner-level keys apply as usual to native hooks: `files`, `exclude`, `types`, `types_or`, `exclude_types`, `args`, `stages`, `always_run`, `pass_filenames`, `require_serial` (disables the shuffle), `fail_fast`, `verbose`, `log_file`, `name`, `alias`.
- With `pass_filenames: false`, native hooks receive no filenames. For `nargs='+'` hooks that is the argparse error, as upstream.

### 6.5 Environment provisioning

- **Tier A, all selected hooks of the repo native, fail-language or stub:** **no clone, no venv, no network.** `install-hooks` has nothing to do for this repo.
- **Any hook of the repo needs DELEGATE:** clone and venv exactly as pre-commit would. Recommendation: do it lazily, the first time a delegated hook actually runs (it may be skipped by stage, `SKIP` or no files).
- `snp install-hooks --with-fallback` provisions the delegate envs ahead of time (useful for offline CI that might later flip `SNP_NO_FAST_PATH`).

### 6.6 Execution contract (runner ↔ native hook)

```rust
pub trait NativeHook: Send + Sync {
    /// argparse-compatible parse; Err => emulate exit 2 + usage text (Appendix A)
    fn parse(&self, epoch: Epoch, argv: &[OsString]) -> Result<Self::Args, ArgError>;
    fn preflight(&self, args: &Self::Args) -> Result<(), Unsupported>;
    /// Called once per xargs partition (or once if coalescing), files in shuffled order.
    fn run(&self, cx: &HookCx<'_>, args: &Self::Args, files: &[RepoPath]) -> Outcome;
}
pub struct Outcome {
    pub exit_code: i32,          // 0/1/2 as upstream; crash path => 1
    pub output: Vec<u8>,         // exact bytes as the merged stdout+stderr pipe would carry them
    pub modified: Vec<RepoPath>, // paths whose bytes this invocation actually changed
    pub crashed: Option<CrashInfo>,
}
```

- `HookCx` provides:
  - the repo root, the effective hook environment (inherited + `PRE_COMMIT_*`), the color/pty mode (for §2.3 ordering), the platform, the epoch;
  - a `GitFacts` cache (§9.6) and a byte-buffer pool.
- Partitions:
  - the runner computes the shuffled list and the xargs partitions (02 §11.6–11.7), calls `run` per partition (in parallel on the shared job pool), concatenates `output` in partition order, and takes the exit code with the max absolute value;
  - `fast_path_coalesce=true` makes a single call instead.
- Inside `run`, files are processed with per-file parallelism (rayon or scoped threads). **Effects are committed in input order** (output append and file write), and processing stops after the first crash, so later files stay untouched (Python semantics, §2.6).
  - Fixers: compute in parallel and write in order.
  - Stateful hooks (case-conflict, vcs-permalinks footer, private-key end-of-run printing, aws) run their global step after the per-file step.

### 6.7 "files were modified" without two `git diff` calls

pre-commit compares the unstaged `git diff` before and after each hook. For a native hook, snp sets `files_modified = true` iff:
- some path in `outcome.modified` is **tracked** (in the index) and not inside a submodule; **and**
- the change is visible to git. snp falls back to computing `git diff --no-ext-diff --no-textconv --ignore-submodules` before and after, **for that hook only**, if any of these hold:
  - `core.autocrlf != false`;
  - a `text`/`eol`/`working-tree-encoding`/`filter`/`ident` attribute applies to that path (`git check-attr -a`, cached);
  - `core.safecrlf` is set;
  - the index entry has `assume-unchanged` or `skip-worktree` set (`git diff` ignores such entries).

Untracked or ignored modified files (passed via `--files`) give `false`, which matches pre-commit (`git diff` does not show them). The retcode is still 1 from the hook itself.

### 6.8 Observability

- No visible output difference by default; `-v` output is identical to pre-commit.
- `SNP_LOG=debug`: `fast-path trailing-whitespace: native (pre-commit-hooks v6.0.0, tier A, epoch 1)` or `fast-path check-ast: delegate (python-ast hook)`.
- `snp doctor fast-path [--config FILE]` prints a table: repo, rev, hook, decision, reason, epoch, and whether a clone or venv is needed.
- The `hook-impl` path (git hooks) honours `SNP_NO_FAST_PATH` from the environment.

### 6.9 Safety review of substitution

- Tier A executes zero bytes from the network.
- Tier B reads only git metadata (tree and blob ids) from the clone and never executes it, unless it delegates (which is exactly pre-commit's trust model).
- Divergence risk is limited to snp bugs. It is mitigated by differential testing (§9.9) and the `exact`/`off` escape hatches.
- A moved upstream tag cannot inject code. At worst behaviour diverges: Tier A trusts the embedded profile.

---

## 7. `repo: builtin` — snp-native hooks

### 7.1 Syntax and validation

```yaml
repos:
  - repo: builtin          # no `rev` (error if present: "`rev` is not allowed for repo: builtin")
    hooks:
      - id: trailing-whitespace
        args: [--markdown-linebreak-ext=md]
      - id: check-yaml
        args: [--allow-multiple-documents]
```

- Allowed hook keys are the same as for `repo: local`, **except**:
  - `entry` is not allowed;
  - `language` may be omitted or set to `system` (prek-compatible);
  - `additional_dependencies` is not allowed.
- Defaults (name, types, files, stages, pass_filenames, always_run) come from snp's builtin manifest. That manifest equals the **pre-commit-hooks v6.0.0 manifest** for the shared ids, so moving between `builtin` and upstream needs no other edits.
- An unknown id gives the error `` `{id}` is not present in repository builtin.  Typo? Perhaps it is introduced in a newer version?  Often `snp autoupdate` fixes this. `` (the pre-commit wording, adapted).
- **Portability:** pre-commit rejects `repo: builtin` (it requires `rev`, then fails to clone). Documented clearly.
  - `snp migrate builtin`: rewrites `pre-commit-hooks` blocks to `builtin` where every hook is supported. It removes `rev` and keeps args, and adds a comment `# was: https://github.com/pre-commit/pre-commit-hooks@vX`.
  - `snp migrate upstream`: the reverse, pinning the latest known tag.

### 7.2 Semantics: "upstream-compatible, bugs fixed"

Builtin hooks accept **exactly** the upstream CLI (same flags, same messages, same exit codes) and may add new flags. They deliberately fix the following upstream defects. Each fix is listed in `snp hooks show <id>`.

| id | fixes vs upstream (substitution mode keeps the upstream behaviour) | extra flags |
|---|---|---|
| trailing-whitespace | a file named `x.` is not markdown by default | `--check` (report only) |
| end-of-file-fixer | read-only files that need no change pass | `--check` |
| mixed-line-ending | – | `--check` (alias of `--fix=no` with fixer exit semantics) |
| fix-byte-order-marker | byte-level; no UTF-8 validation crash | `--check` |
| file-contents-sorter | – | `--check` |
| requirements-txt-fixer | indented requirement lines no longer crash (lstrip before name parsing); CRLF `pkg-resources==0.0.0\r\n` also removed | `--check` |
| sort-simple-yaml | no perpetual rewrite of `\n`; UTF-8 always | `--check` |
| pretty-format-json | duplicate keys never crash (last wins, keeps first position); CRLF preserved when autofixing a CRLF file; LF on all platforms otherwise | `--check` = default |
| check-json | deep nesting has no crash (bounded only by memory); exact messages of the emulated CPython | – |
| check-yaml | invalid scalars (`!!int "abc"`, bad dates) are reported as errors, not crashes | – |
| check-added-large-files | `-z` git plumbing (non-ASCII names are checked); deterministic order; renames counted as added (`--diff-filter=AR`) behind `--include-renames` | `--include-renames` |
| check-case-conflict | `-z`; NFC/NFD and Unicode case folding option for macOS | `--unicode-normalize` |
| check-executables-have-shebangs / check-shebang-scripts-are-executable | literal pathspecs; one message per path even during merge conflicts | – |
| check-merge-conflict | also detects `CHERRY_PICK_HEAD` and `REVERT_HEAD` states | `--diff3` (also `\|\|\|\|\|\|\| `) |
| detect-aws-credentials | tolerant INI parsing (no crash on duplicates or `%`); deterministic order | – |
| no-commit-to-branch | – | – |
| check-vcs-permalinks | the domain is matched literally (escaped) | `--domain-regex` keeps the regex behaviour |
| destroyed-symlinks / check-symlinks / forbid-new-submodules / check-illegal-windows-names / forbid-submodules / name-tests-test / check-toml / check-xml / detect-private-key | same as substitution | – |

### 7.3 Candidate snp-only builtin ids (separate namespace, no collision with upstream ids)

Optional, P3. Each one must be justified by popularity.

| id | inspiration | behaviour |
|---|---|---|
| `forbid-crlf` / `remove-crlf` | Lucas-C/pre-commit-hooks | byte-level |
| `forbid-tabs` / `remove-tabs` (`--whitespaces-count N`) | Lucas-C | – |
| `check-utf8` | – | reject invalid UTF-8 in `types: [text]` |
| `forbid-bidi-controls` | sirosen/texthooks | reject U+202A–U+202E, U+2066–U+2069 (Trojan Source) |
| `conventional-commit` (stage `commit-msg`) | compilerla/conventional-pre-commit | lefthook-style commit-msg check |
| `check-jsonc`, `check-json5` | prek | – |
| `deny-pattern` / `require-pattern` | prek | similar to `pygrep`, built in |

Do not add tool reimplementations (linters, formatters) as builtin hooks.

---

## 8. Other popular hook repositories: is a native implementation worthwhile?

Stars are a snapshot from the GitHub API on 2026-10-07 (partial: the API rate-limited).

| repo | hooks | language | native? | reasoning |
|---|---|---|---|---|
| pre-commit/pre-commit-hooks (6.7k★) | this document | python | **yes** | trivial byte/git logic; by far the most used |
| pre-commit/pygrep-hooks (243★) | python-check-blanket-noqa, -type-ignore, -mock-methods, no-eval, no-log-warn, use-type-annotations, rst-backticks, rst-directive-colons, rst-inline-touching-normal, text-unicode-replacement-char | **`language: pygrep`** | **free** via snp's native `pygrep` language | entries need look-around, `(?x)` verbose mode and fixed-width look-behind with `\b` (`(?<!\bAsyncMock)`). snp's pygrep engine must be Python-`re` compatible (fancy-regex plus a translation layer, or PCRE2 via the `pcre2` crate, which is C, i.e. FFI: weigh it against the no-unsafe policy). No clone-time work beyond the manifest; embed manifests for Tier A as for pre-commit-hooks |
| astral-sh/ruff-pre-commit (2.0k★) | ruff, ruff-format | python (wheel with a binary) | no | a real tool. Optimise **installation** instead (uv, wheel cache, reuse across repos) |
| psf/black-pre-commit-mirror | black | python | no | same |
| PyCQA/isort (7.0k★), PyCQA/flake8 (3.8k★), asottile/pyupgrade (4.1k★), pre-commit/mirrors-mypy | | python | no | tools |
| codespell-project/codespell (2.4k★) | codespell | python | no | dictionary-driven; `crate-ci/typos` is the Rust alternative and ships its own hook |
| adrienverge/yamllint (3.5k★) | yamllint | python | no | a large rule set |
| gitleaks/gitleaks (29.8k★), Yelp/detect-secrets (4.6k★) | | golang / python | no | secret engines; golang hooks build once and are fast afterwards |
| antonbabenko/pre-commit-terraform (3.8k★) | terraform_fmt, tflint, … | script (bash) | no | wraps external CLIs; bash startup is cheap |
| igorshubovych/markdownlint-cli (1.1k★), rbubley/mirrors-prettier, eslint mirrors | | node | no | tools; optimise node env creation |
| shellcheck-py/shellcheck-py, koalaman/shellcheck-precommit, hadolint, rhysd/actionlint | | python-wheel / docker / golang | no | binaries |
| Lucas-C/pre-commit-hooks (153★) | forbid-crlf, remove-crlf, forbid-tabs, remove-tabs, chmod, insert-license | python | maybe (P3) | the first 4 are trivial byte hooks; the user base is moderate. Prefer snp-only builtins (§7.3) to substitution |
| sirosen/texthooks | fix-smartquotes, fix-ligatures, fix-spaces, forbid-bidi-controls, alphabetize-codeowners | python | maybe (P3) | Unicode-table driven, small |
| python-jsonschema/check-jsonschema | check-jsonschema, check-github-workflows, … | python | no | jsonschema plus remote `$ref`s and bundled vendor schemas |
| compilerla/conventional-pre-commit, commitizen | commit-msg | python | builtin alternative (§7.3) | one regex |
| macisamuele/language-formatters-pre-commit-hooks | pretty-format-yaml, -toml, -ini, -java, -kotlin, -golang, -rust | python + jars | no | |
| jumanjihouse/pre-commit-hooks | forbid-binary, require-ascii, script-must-have-extension, git-check, … | script/ruby | no | already cheap (bash) |

Conclusion: only `pre-commit/pre-commit-hooks` justifies automatic substitution. `pygrep-hooks` is effectively native once the `pygrep` language is native. Everything else benefits from faster environment provisioning, which is out of scope here.

---
## 9. Implementation notes for snp

### 9.1 Crate layout

| crate | contents | unsafe |
|---|---|---|
| `snp-pycompat` | `argparse` emulator; `pybytes` (strip/rstrip/isspace/splitlines/lower with ASCII-WS); `pystr` (WTF-8 `PyStr`, `repr`, `isspace`/PY-STR-WS, `splitlines`, `lower`); `pyint` (`int()` parsing, 4300-digit limit); `pyfloat` (`repr`); `shlex` (`quote`, `join`); `ospath` (`basename`, `splitext`, posix/nt); `difflib` (SequenceMatcher + `unified_diff`); `json` (decoder + encoder, CPython-version-parameterised messages); `configparser` subset; `codecs` (UnicodeDecodeError message formatting; utf-8/16/32 with `surrogatepass`); `pyio` (two-layer stdout model, §2.3); `random` (MT19937 + `shuffle`, shared with the runner) | `#![forbid(unsafe_code)]` |
| `snp-hooks-pch` | one module per hook: `Args`, `Epoch`, `parse`, `preflight`, `run`; the registry `id → NativeHook` | forbid |
| `snp-hooks-data` | **generated**: per-tag manifests (verbatim YAML), tag→commit, `VERIFIED_BLOBS` (blob→(module, epoch)), console-script map. Produced by `cargo xtask gen-pch-data <full-clone>`, checked in, verified by CI (re-generate and diff) | forbid |
| `snp-yaml-check` | ruamel-compatible validating loader (stage 1: event parser + composer + constructor-checker; stage 2: a ruamel scanner/parser port) | forbid |
| `snp-xml-wf` | expat-compatible well-formedness checker | forbid |
| `snp-toml-check` | `toml_parser` events + tomllib-compatible semantics and messages | forbid |

Dependencies (crates.io, checked 2026-10-07; pin exact versions in `Cargo.lock`):

| purpose | crate |
|---|---|
| fast byte search | `memchr` 2.8 |
| byte strings | `bstr` 1.13 |
| multi-pattern scan (private keys, AWS keys) | `aho-corasick` 1.1 |
| regex | `regex` 1.13 (bytes API for scanners); `fancy-regex` 0.19 (Python-like look-around/backrefs for user patterns) |
| TOML | `toml_parser` 1.1.x (`+spec-1.1.0`) |
| YAML stage 1 | `saphyr-parser` 0.1 or `yaml-rust2` 0.13 |
| XML tokens | `xmlparser` 0.13 |
| legacy encodings | `encoding_rs` 0.8 |
| HEAD/index/tree/blob reads | `gix` 0.88 (or the git CLI) |
| data parallelism | `rayon` |

Avoid C FFI (`pcre2`, libyaml) and `memmap2` (mmap needs `unsafe` and is unsound if files change underneath).

### 9.2 Hook implementation rules

1. **Byte-first.**
   - Every hook except check-yaml, pretty-format-json and sort-simple-yaml works on `&[u8]`.
   - Never `String::from_utf8_lossy` file contents that get written back.
   - Messages embed filenames as **raw bytes** of the repo-relative path. On Unix, use `OsStrExt` (Python would raise `UnicodeEncodeError` for non-UTF-8 names under a UTF-8 locale: documented crash-path deviation).
2. **Exact strings.** Keep all format strings in one `messages.rs` per hook with a doc comment citing the upstream source line. Add golden tests from this document.
3. **Crash paths.** Model them as `Err(Crash { kind, message })`. The runner maps them to exit 1, the message `"{prog}: error: {message}\n"`, and stop-at-first-crash within the invocation. Never `panic!`. The crate denies `clippy::unwrap_used`, `expect_used`, `panic`, `indexing_slicing` and `arithmetic_side_effects`.
4. **Epochs** are plain enums threaded through `run`. Keep the epoch differences small: an `if` per difference, with tests per epoch.
5. **No global mutable state.** `GitFacts` is per-run with interior caching (`OnceLock`/`Mutex<HashMap>`).

### 9.3 argparse emulator: the required subset

Required features:
- actions: `store`, `store_true`, `store_false`, `store_const`, `append`, `help`;
- `type`: `str` / `int` / custom closures (`parse_num_to_int`, `parse_topkeys`, `parse_ignore`);
- `choices`, `default`, `required`, `dest`, `metavar`;
- positional `nargs='*'` or `'+'`;
- one mutually exclusive group;
- `allow_abbrev=True`.

**Classification** of each argv token (CPython 3.12 `_parse_known_args`):
1. `--` ends option parsing. Its first occurrence is removed; the rest are positionals.
2. A token is a positional (`A`) if it does not start with `-`, or is exactly `-`, or looks like a negative number (`^-\d+$|^-\d*\.\d+$`) while the parser has no negative-number-like options. A token with a space after the prefix is also `A`.
3. Otherwise it is an optional (`O`):
   - exact match;
   - `--name=value` (split at the first `=`);
   - unique long-option prefix (abbreviation ✓). An ambiguous prefix gives the error `ambiguous option: {arg} could match {', '.join(matches)}`;
   - short `-fVALUE` (value attached);
   - unknown options are collected as extras.

**Consumption:**
- Options with one argument take the next `A` token. If there is none, the error is `argument {names}: expected one argument`, where names = `'/'.join(option_strings)`.
- Positionals are consumed **in chunks**: a `nargs='*'` positional matches only the first contiguous run of `A` tokens. Later `A` tokens become extras, giving `unrecognized arguments: …`. This is only reachable if user `args` contain positionals before options.

**Errors** (to stderr, exit 2):
```
{usage}
{prog}: error: {message}
```
Messages:
- `unrecognized arguments: {' '.join(extras)}`
- `the following arguments are required: {', '.join(names)}`
- `argument {X}: not allowed with argument {Y}` (X = the action being parsed, i.e. the later one)
- `argument {names}: invalid int value: {repr(value)}`
- `argument {names}: invalid choice: {repr(value)} (choose from {', '.join(map(repr, choices))})` (identical in CPython 3.12.3 and 3.13.14 ✓; 3.14+ unverified, §10 Q2)
- hook-specific `parser.error(...)` messages (trailing-whitespace).

**`-h`/`--help`:** print the help text (Appendix A) to stdout and exit 0. No files are processed.

**Int parsing** (Python `int(str)`):
- strip PY-STR-WS on both ends;
- optional `+`/`-`;
- digits from Unicode category Nd (each maps to its decimal value), with single `_` allowed **between** digits;
- no base prefixes, no empty string.

### 9.4 CPython `json` port: notes

- **Decoder:**
  - `detect_encoding` → decode with `surrogatepass` into a WTF-8 buffer, plus a parallel **code-point index** (not byte index) for positions. Compute positions on demand by counting chars up to the byte offset (only on error).
  - The scanner is a non-recursive state machine (explicit stack of frames).
  - Number grammar exactly as specified. Integers keep their digit string; no bigint arithmetic is needed:
    - check-json only needs the >4300-digit rule;
    - pretty-format-json re-emits the digit string **after Python int normalisation**. The only change is ✓ `-0` → `0` (leading zeros and `+` are already invalid JSON).
    - Numbers with a fraction or exponent become floats and are re-emitted by `float.__repr__` ✓: `1E5` → `100000.0`, `1e2` → `100.0`, `0.10` → `0.1`, `-0.0` → `-0.0`.
  - Floats: Rust `f64::from_str` is correctly rounded, as CPython `float()` is. Overflow gives inf; JSON `1e999` passes in Python and must not error natively.
- **Messages:** parameterise by the emulated CPython minor (3.12 vs 3.13+ trailing-comma messages, §4.C).
- **Encoder (pretty-format-json):** port `json.encoder` `_make_iterencode` with `indent`, `separators=(',', ': ')`, `ensure_ascii`, and `float.__repr__`.
- **`float.__repr__`:**
  - take the shortest round-trip digits (`ryu`-style; `f64` `{:?}`/`{}` are not the same format);
  - decimal exponent `e = position of the decimal point`;
  - if `-4 <= e-1 < 16`, use fixed notation with at least one digit after the point (`1.0`, `0.0001`, `1000000000000000.0`);
  - else scientific `d[.ddd]e±XX` with a 2+-digit exponent (`1e+16`, `1e-05`, `1.2345678901234568e+17`, `5e-324`);
  - `-0.0` is `-0.0`.

### 9.5 YAML checker architecture (check-yaml)

```
bytes ─► strict UTF-8 decode (crash path on error) ─► universal-newline normalise (\r\n,\r → \n; track original?)
     ─► reader checks (non-printable → ReaderError "unacceptable character #x%04x: special characters are not allowed", position = char index)
     ─► scanner+parser (events with marks: line/col 0-based, printed +1)
     ─► [unsafe mode stops here]
     ─► composer (document count rule; anchors map; alias → node-id; "found undefined alias '%s'")
     ─► constructor-checker (resolver 1.1/1.2 → tag; tag → validator; mapping: merge-key flatten rules, key value model, dup detection with PyValue equality/hash; printing via str())
```

- `PyValue` = `None | Bool | Int(BigInt-ish) | Float(f64, is_shared_nan) | Str | Bytes | Date | DateTime(tz?) | Tuple(Vec) | List(id) | Dict(id) | Set(id)`.
- Equality follows Python semantics: numeric tower; NaN equal only via the shared object; str ≠ bytes; tuple elementwise. Hash is consistent with equality: hash ints and integral floats identically.
- Containers print as **empty**, matching the "constructed later" behaviour.
- Never expand aliases.
- Limit nesting depth with an explicit stack. ruamel's composer and constructor recurse with about 2 Python frames per level against the 1000-frame limit. ✓ Nested flow sequences pass up to depth 494 and crash at 495; nested flow mappings pass at 487 and crash at 489. The crash is `RecursionError` in default and `--multi` modes. `--unsafe` (parser only) passed at 5000 levels. Treat depth > the emulated threshold as the crash path (§10 Q9).

### 9.6 Git facts: exact commands, caching and environment

- Spawn `git` with **the hook environment** (inherited, including `GIT_INDEX_FILE`, `GIT_DIR`, `GIT_WORK_TREE`; plus `PRE_COMMIT=1`, …) and cwd = repo root. Use the **identical argv** as Python (§3). Do **not** add `-c core.quotePath=false`, `-z` or `--literal-pathspecs` in substitution mode; those change the behaviour.
- Per-run cache (`GitFacts`):

| key | command |
|---|---|
| `git_dir` | `rev-parse --git-dir` |
| `merge_state` | the 4 path probes |
| `core_filemode_raw` | `config core.fileMode` stdout |
| `staged_added_raw` | `diff --staged --name-only --diff-filter=A` stdout |
| `ls_files_raw` | `ls-files` stdout |
| `head_symref` | `symbolic-ref HEAD` |

- **Invalidation:** compare `(len, mtime_ns, ino)` of the index file (`$GIT_INDEX_FILE` or `<git_dir>/index`) and the `HEAD` file before each use. Hooks of *other* languages may run `git add`.
- **Per-partition (not cached):** `check-attr filter -z --stdin`, `ls-files -z --stage -- <paths>`, `status --porcelain=v2 -z -- <paths>`, `diff --raw … -- <paths>`, `git lfs status --json`.
- `gix` may replace the CLI only where the semantics are trivially identical: reading HEAD (symbolic or detached); probing merge-state files; reading blobs and trees in destroyed-symlinks. It must honour `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR` and worktrees.

### 9.7 Performance techniques

| hook | technique |
|---|---|
| end-of-file-fixer | reverse block scan of the tail (4 KiB blocks); O(1) I/O; never reads the whole file unless it is all CR/LF |
| check-added-large-files, check-symlinks | metadata only (`symlink_metadata` / `metadata`) |
| executable/shebang hooks | read 2 bytes (`read_exact` into `[u8; 2]`, short file means no shebang) |
| fix-byte-order-marker | 3-byte probe; full read only when a BOM is present |
| detect-private-key, detect-aws-credentials | Aho–Corasick stream search with an early exit at the first hit (private key) or once all keys are found (AWS) |
| check-merge-conflict, check-vcs-permalinks | line scan in 64 KiB chunks; only line starts (merge) or `https://` hits (vcs) are examined |
| fixers (trailing-ws, mixed-eol, sorters) | one `read_to_end` into a pooled `Vec<u8>` sized from metadata; compute the output into a second pooled buffer only when the first difference is found; write once (`write_all`, then `set_len`) |
| all | per-file work in parallel within a partition (rayon); ordered commit; partitions in parallel on the global job pool; no per-line allocations; `SmallVec` for the per-file message list |

- Expected gains:
  - a cold first run with the sample-config hooks: tens of seconds (clone + venv + pip) become ~0 s;
  - a warm run: about 50 ms of Python startup × partitions × hooks becomes microseconds to milliseconds.
- Memory is bounded by `max(file size)` per worker for fixers and by O(chunk) for scanners.

### 9.8 File I/O safety

- **Write in place:** `OpenOptions::new().write(true).open(path)`, then `write_all`, then `set_len`. No `create`, so a vanished file is an error, i.e. the crash path, like Python's `FileNotFoundError`.
- **No temp+rename.** It changes inode, ownership, mode and xattrs, breaks hardlinks, fails across devices (prek #508 "Fix rename tempfile across device"), and on Windows conflicts with open handles.
- Follow symlinks like Python's `open` (std does). Do not special-case them: pre-commit's type filters already exclude symlinks for text hooks.
- **Windows:**
  - opening a file locked without `FILE_SHARE_WRITE` is the crash path;
  - std handles long paths (`\\?\`) when building absolute paths; prefer `root.join(rel)`;
  - text-mode parity (CRLF writes) applies only to pretty-format-json `--autofix` and sort-simple-yaml.
- Never follow `..` outside the repo root: filenames come from pre-commit's file list and are trusted repo-relative paths. Still reject absolute paths defensively in `RepoPath::new`.
- TOCTOU is irrelevant (same trust domain as the user). Do not hold file handles across partitions.

### 9.9 Testing and verification strategy (required before enabling a hook in `auto` or `exact`)

1. **Golden tests:** every ✓ example in this document, as `(argv, files-before) → (exit, output, files-after)` snapshot tests per epoch.
2. **Differential oracle** (CI, nightly):
   - pinned upstream venvs per supported tag (at least v3.2.0, v4.0.1, v4.4.0, v4.6.0, v5.0.0, v6.0.0) × CPython minors (3.10, 3.12, 3.13, 3.14, 3.15);
   - each case runs both the Python console script (in a temp repo with `PRE_COMMIT=1` and the same env) and the snp native hook;
   - compare the exit code, the merged output bytes and the resulting file bytes;
   - run the Python side as a long-lived worker that imports `pre_commit_hooks.<module>.main` and executes `main(argv)` with captured fds, to avoid process startup cost.
3. **Generators:**
   - whitespace/EOL fixers: random byte strings over the alphabet `{a, ' ', '\t', '\r', '\n', '\x0b', '\x0c', '\x1c', '\xc2\xa0', '#', '='}`, length 0–64, plus file names like `x.md`, `x.`, `X.MD`;
   - requirements: a grammar-based line generator (comments, blanks, continuations, eggs, CRLF);
   - JSON/YAML/TOML/XML: grammar-aware generators plus mutation of seed corpora (yaml-test-suite, toml-test, JSONTestSuite, W3C XML conformance suite), with acceptance diffs bucketed by error class.
4. **Fuzzing** (cargo-fuzz): every parser and every hook `run` on arbitrary bytes must never panic, overflow the stack, or exceed linear memory.
5. **Parity budget:** NATIVE hooks need 0 diffs. NATIVE-WITH-CAVEATS hooks need 0 acceptance diffs on the corpora, and message diffs only within the documented classes.

### 9.10 Pitfalls checklist

1. Shuffle + partition + per-partition footers: tests must pin `jobs` (`PRE_COMMIT_NO_CONCURRENCY`) or emulate.
2. `out.strip()` hides the trailing-newline bugs of check-vcs-permalinks; the interior newline handling still matters.
3. Python `print` + `buffer.write` ordering (pretty-format-json, non-tty).
4. `bytes.rstrip()` ≠ `str.rstrip()` (ASCII-WS vs PY-STR-WS); `bytes.splitlines` ≠ `str.splitlines`.
5. `--chars` is a **byte set** after UTF-8 encoding; `--chars=` strips nothing.
6. `os.path.splitext('foo.') == ('foo','.')`, so the default `md_exts == ['.']`.
7. end-of-file-fixer appends `\n` even to CRLF files and empties all-newline files.
8. mixed-line-ending appends the ending to an unterminated last line when fixing; ties prefer LF > CRLF > CR.
9. file-contents-sorter strips **all** trailing CR/LF and drops ASCII-WS-only lines; `--ignore-case` is ASCII-only.
10. requirements-txt-fixer: egg names include the trailing newline; header detection needs a column-0 comment as the first line followed by a blank line; dedupe compares only with the previous *kept* value; LF-only `pkg-resources` removal; indented lines crash.
11. check-json: char-based positions, `surrogatepass`, NaN/Infinity accepted, 4300-digit integers rejected, duplicate check at object close, Python-version-dependent trailing-comma messages.
12. check-yaml: duplicate keys use Python equality (`1 == 1.0 == true`, shared NaN); container values print empty; `=` is an error; invalid scalars crash; `--unsafe` skips the composer.
13. check-toml: TOML 1.0 vs 1.1 depends on CPython; BOM rejected; bigints accepted.
14. check-xml: namespaces off, external entities off, code-point columns (0-based), single-byte Python codecs accepted, multibyte legacy encodings crash.
15. git output quoting (`core.quotePath`) and rename detection change pass/fail for large-files and case-conflict.
16. `core.fileMode` is compared as the raw string `'false'`.
17. pathspec globbing in `ls-files --stage -- <paths>` and `status -- <paths>`.
18. check-merge-conflict ignores cherry-pick/revert conflicts and `|||||||`; the message uses `repr` (single quotes).
19. `no-commit-to-branch`: `re.match` is a prefix match; it prints nothing.
20. detect-aws-credentials: append-to-defaults argparse quirk, DEFAULT section fallback, interpolation crash, exit code 2.
21. The `GIT_INDEX_FILE` inherited from `git commit <pathspec>` must reach every git call.
22. A crash aborts the remainder of the invocation; fixes already written persist.
23. Hook display names differ per tag (Title Case before v4.1.0).
24. The sample-config pin is v3.2.0: the old-epoch `check-added-large-files` (`git lfs status --json`) must be native.

---

## 10. Open questions

| # | question | recommendation |
|---|---|---|
| Q1 | Default mode `auto` (NATIVE + NATIVE-WITH-CAVEATS) vs `exact`? | `auto`, but enable each caveat hook only after its differential parity gate passes. `check-xml` can stay delegated in v1 |
| Q2 | Which CPython version to emulate for version-dependent texts? Examples: json trailing-comma messages (3.13+ ✓); argparse help rendering of aliased options (`-f, --fix {…}` in 3.13+ ✓); argparse colour (3.14+ on a tty); TOML 1.1 in tomllib (3.15). | Emulate the **CPython minor that pre-commit would pick by default for a fresh install today** (3.13 or later; decide at release) via `SNP_PY_COMPAT=3.12\|3.13\|3.14\|3.15`. Default to the newest stable minor. Verify the argparse texts on 3.14 and 3.15 before freezing the strings |
| Q3 | Bug-compat in substitution mode: should crash paths (read-only EOF, indented requirement, BOM + invalid UTF-8, deep JSON) be emulated as failures, or improved? | Keep exit 1 and stop-processing for parse-related crashes (they signal real input problems). Improve purely environmental crashes: the read-only file with no change needed. Each case is listed in §4; the final list needs product sign-off |
| Q4 | Partition emulation by default (duplicate footers, CPU-count-dependent output) vs coalescing? | Default to emulation (strict parity); offer `fast_path_coalesce`. Revisit if users complain |
| Q5 | Support floor: v3.0.0 (includes the sample-config pin v3.2.0) vs v4.0.0? | v3.0.0. Below that, DELEGATE. Gather usage data (GitHub code search for `pre-commit-hooks` + `rev: v2.`) |
| Q6 | Windows: replicate text-mode CRLF writes (pretty-format-json `--autofix`, sort-simple-yaml) and locale encodings (cp1252) for sort-simple-yaml and configparser? | Replicate CRLF writes (they are observable). Assume UTF-8 for decoding (UTF-8 mode is the default in 3.15, and most configs are ASCII) and document it |
| Q7 | Should explicit `language: python` (prek convention) be the per-hook opt-out, given it overloads a semantic key? | Yes, for prek compatibility and zero pre-commit warnings. Also support an snp settings-file `fast_path_exclude` |
| Q8 | Unicode version drift for `str.lower()` (check-case-conflict) between CPython's UCD and Rust std | Accept it. Differences only affect newly assigned characters; note it in docs |
| Q9 | Recursion and depth limits: CPython raises `RecursionError` (crash). Thresholds ✓ on CPython 3.12.3: YAML sequences crash at depth 495 and mappings at 489 (ruamel load; parse-only mode has no limit); JSON arrays crash at 9998 and objects at 9996. Match the exact thresholds? They depend on the CPython version and on the frames already on the stack | Use the CPython thresholds as constants per emulated version, mapped to the crash path. They are cheap to match and keep pass/fail identical |
| Q10 | Ever nativise the AST hooks (debug-statements, check-builtin-literals) with a vendored Rust Python parser? | Not in v1. Grammar/version coupling, and ruff's parser crates are unpublished |
| Q11 | ruamel version drift: users with old envs (ruamel ≤0.18 + clib) see libyaml-style messages | Target ruamel ≥0.19 pure; message diffs on old envs are acceptable |
| Q12 | Tier B blob verification requires a clone. Should snp use `git ls-remote` plus a blob-less partial clone (`--filter=blob:none`) to make it cheaper? | Yes: `git fetch --depth=1 --filter=blob:none origin <rev>` then `ls-tree`. Fetch blobs only if delegation is needed |
| Q13 | `COLUMNS` in the environment changes argparse wrapping in usage/help | Ignore (use 78) unless COLUMNS is set; then port `HelpFormatter` wrapping (low priority) |
| Q14 | Should `repo: builtin` hook names differ from upstream (to signal native)? | No. Identical names keep `snp` and pre-commit status lines aligned after `snp migrate` |

---
## Appendix A — argparse usage and help texts (verbatim, CPython 3.12.3, width 78 = default 80 columns − 2)

The texts were captured with `args: [-h]` and `verbose: true` ✓.
- **Help** (`-h`) prints the whole block to **stdout** and exits 0.
- **Errors** print only the `usage:` paragraph (up to the first blank line), then `{prog}: error: {message}`, to **stderr**, and exit 2.

[[RECOVERY GAP: the verbatim per-hook argparse usage/help blocks of Appendix A (about 440 lines) were generated from a capture file that was lost in the /tmp wipe. Regenerate them by running each pre-commit-hooks v6.0.0 console script with `-h` under CPython 3.12.3 with COLUMNS=80 (width 78), as described above.]]

## Appendix B — Python semantics cheat-sheet (verified on CPython 3.12.3 unless noted)

| construct | semantics |
|---|---|
| `bytes.strip()/rstrip()/isspace()` | ASCII-WS `{0x20,0x09,0x0A,0x0D,0x0B,0x0C}` ✓ (`b'\x1c'.isspace()` is False). `isspace()` of empty is False |
| `bytes.rstrip(chars)` | strips trailing bytes that are members of the byte set `chars`; `b''` strips nothing |
| `bytes.lower()` | ASCII A–Z only |
| `bytes.splitlines(keepends)` | boundaries `\r\n`, `\r`, `\n` only ✓ |
| binary file iteration / `readlines()` | split after each `\n` only |
| `str.isspace()/strip()` | PY-STR-WS: `\t\n\x0b\x0c\r\x1c\x1d\x1e\x1f`, space, `\x85`, `\xa0`, ` `, ` `–` `, ` `, ` `, ` `, ` `, `　` ✓ |
| `str.splitlines()` | `\n \r \r\n \x0b \x0c \x1c \x1d \x1e \x85    ` ✓ |
| `str.lower()` | full Unicode lowercase (SpecialCasing, final sigma) |
| text-mode read (`newline=None`) | `\r\n` and `\r` become `\n`; `readlines()` then splits on `\n` |
| text-mode write | `\n` becomes `os.linesep` (`\r\n` on Windows) unless `newline=''` |
| `os.path.splitext` | `'foo.'`→`('foo','.')`; `'.md'`→`('.md','')`; `'...md'`→`('...md','')`; `'a..md'`→`('a.','.md')`; `'dir.d/file'`→`('dir.d/file','')` ✓ |
| `os.path.basename` | POSIX: after the last `/`. Windows (ntpath): after the last `/` or `\`, also after a drive `X:` |
| `shlex.quote(s)` | `''` if empty; unchanged if every char ∈ `[A-Za-z0-9_@%+=:,./-]` (ASCII `\w`); else `'` + `s.replace("'", "'\"'\"'")` + `'` ✓ (`é.sh` → `'é.sh'`) |
| `shlex.join(xs)` | `' '.join(map(quote, xs))` |
| `repr(str)` | quote char `'` unless the string contains `'` and no `"`, then `"`. Escapes `\\`, the quote char, `\t`, `\n`, `\r`; other non-printables as `\xhh`/`\uhhhh`/`\Uhhhhhhhh`; printable non-ASCII kept |
| `int(str)` | whitespace-strip; sign; Unicode Nd digits; single `_` between digits (`1_000` ✓, `0_7`=7 ✓, `1__0` ✗); no prefix (`0x10` ✗); ≤4300 digits (≥3.11) |
| `repr(float)` | shortest round-trip; fixed if `1e-4 <= |x| < 1e16`, else `d[.ddd]e±dd` (`1e+16`, `1.5e-07`, `1e-05`, `1.2345678901234568e+17`, `5e-324`, `1.7976931348623157e+308`) ✓; `inf`, `-inf`, `nan`, `-0.0` |
| `json.dumps(…, ensure_ascii=True)` | `{"a": "\u0000\u001f\u007f \ud800é😀\"\\/\b\f\n\r\t"}` ✓ (`/` not escaped; DEL escaped) |
| `json.dumps(…, ensure_ascii=False)` | only `"`, `\`, `\b \f \n \r \t` and `\u00XX` for other < 0x20 are escaped (DEL and U+2028 are raw ✓) |
| `json.dumps(indent=…)` | `[]`/`{}` for empty containers ✓; an indent string such as `'ab'` is repeated per level ✓ |
| `random.Random().seed(1542676187)` + `shuffle` | MT19937 `init_by_array([1542676187])`; `shuffle` runs `for i in reversed(range(1, n)): j = _randbelow(i+1)`; `_randbelow(m)`: `k = m.bit_length()`, `r = getrandbits(k)`, retry while `r >= m`; `getrandbits(k≤32) = genrand_uint32() >> (32-k)` (details in 02 §11.7) |
| argparse `append` with a list default | the result is default + user values (defaults are never replaced) |
| uncaught exception | traceback on stderr, exit 1; stdout is flushed before the traceback ✓ |
| `SystemExit(str)` | str + `\n` on stderr, exit 1 |
| `TextIOWrapper` (pipe) | pending text flushed when `pending + new > 8192`, when `pending >= 8192` after adding, at exit, or before a traceback; `print(x)` = `write(x)` + `write('\n')` |

## Appendix C — Reproducing the ✓ observations

```sh
# harness (scratchpad): writes files, stages, runs the pinned upstream hook through pre-commit, prints output + resulting bytes
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/tw.py      # trailing-whitespace
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/eof.py     # end-of-file-fixer, mixed-line-ending, BOM
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/sort.py    # sorters
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/json.py    # check-json messages
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/pfj.py     # pretty-format-json
python3 -I tmp-builtin/cases/gen_yaml.py default > c.py && python3 -I tmp-builtin/harness/probe.py c.py
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/tomlxml.py # check-toml / check-xml
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/git1.py    # merge-conflict, large files
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/git2.py    # case-conflict, keys, permalinks, naming, branch
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/git4.py    # aws, shebang hooks
python3 -I tmp-builtin/harness/probe.py tmp-builtin/cases/part.py    # partition duplication (no PRE_COMMIT_NO_CONCURRENCY)
```

Case files are Python literals: `{name, hook, args, files{path: bytes}, run_files, setup[], pre[], rev, extra_hook_keys{}, env{}}`. Use them as the seed corpus for snp's golden tests (§9.9).
