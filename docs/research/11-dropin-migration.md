# snp spec 11: drop-in migration from Python pre-commit

This file collects specs about swapping an installed Python pre-commit for snp and running the two side by side.
Each section is a self-contained addendum appended by a gap-fill task. Never rewrite earlier sections; only append.

---

## Addendum: How snp actually replaces an installed Python pre-commit, and how the two coexist

Gap-fill #1. This addendum is decision-ready: every open question it touches is closed with a rule, and the evidence
is cited. It covers how the swap physically happens, what a user sees on the first commit afterwards, how snp and
Python pre-commit coexist (hook scripts, `.legacy` chains, the `pre-commit` name, the store, CI caches), and how to
roll back.

**Provenance**

| Tag | Meaning |
|---|---|
| REF | pre-commit 4.6.2 source, `refs/pre-commit` (commit `c429238`). Citations like `pre_commit/x.py:L` are relative to `refs/pre-commit/`. |
| OBS-462 | Observed by running `scratchpad/tmp-test-strategy/venv462/bin/pre-commit` (4.6.2, CPython 3.12.3) in throwaway repos under `scratchpad/tmp-migration/`, with isolated `HOME`, `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1`, `PRE_COMMIT_HOME` and `PATH`. Helpers are in `scratchpad/scripts-migration/{env.sh,mkrepo.sh,write_tmpl.sh}`. git 2.43.0. `/bin/sh` is dash. |
| WEB | Fetched on 2026-10-08 into `scratchpad/dl-migration/`: `pre-commit/action` `action.yml` + README (latest tag v3.0.1, repo not archived), pre-commit.com `sections/{advanced,install,usage}.md`, the cargo-dist `book/src/reference/config.md`, the PyPI JSON API and the GitHub commits API for `pre_commit/resources/hook-tmpl`. |
| V1 | The user's **previous** snp, crates.io `snp` 1.0.1 (repo `devops247-online/snp`), extracted to `scratchpad/dl-snp-v1/x/snp-1.0.1/`. A `~/.cargo/bin/snp` v1.0.0 binary and a `~/.cache/snp` store written by it exist on the development machine (OBS). |
| prek | `refs/prek` (lessons only, no code reused) and its issue dump `scratchpad/prek-issues/all.json`. |

**Terms used below**
- *Manager script*: a hook script written by a hook manager (snp, snp 1.x, pre-commit or prek). Classification is
  in §A.5.1.
- *Compat mode*: snp was invoked under the name `pre-commit`. *snp mode*: any other name. See §A.8.
- `{prog}` is `pre-commit` in compat mode and `snp` in snp mode.
- `SNP_VERSION` stands for snp's own version, `2.0.0` in the examples. crates.io `snp` 1.0.0 and 1.0.1 already exist (V1),
  so the rewrite must be published as 2.0.0 or later. `PC_COMPAT_VERSION` is `4.6.2`.

---

### A.1 Decisions (TL;DR)

1. **No silent takeover.** Installing the snp binary changes nothing for git, CI or scripts. Users switch with three
   explicit, reversible actions:
   - `snp install`, or `snp doctor --fix`, rewrites hook scripts;
   - the **opt-in** `pre-commit` name, from `snp self alias install` or the `snp-pre-commit-compat` PyPI distribution;
   - edits to CI and tooling.

   No channel installs a `pre-commit` executable by default. That rules out cargo-dist `bin-aliases` and a second
   script in the main wheel.
2. **Existing pre-commit hook scripts keep running Python pre-commit after snp is installed**, as long as their
   `INSTALL_PYTHON` exists and can import `pre_commit`. In that case PATH is never consulted (OBS-462, Case A).
   Therefore `snp install` **must** treat pre-commit's `CURRENT_HASH` and all 5 `PRIOR_HASHES`, prek's ID, and the
   snp 1.x marker as manager scripts and overwrite them in place. It must never move them to `.legacy`.
3. **snp's hook script** (§A.6) is POSIX `sh`. It embeds three IDs:
   - its own ID, `5bb9bd89cf6c92c58fee7677ceaf4fc3` = md5(`snp hook script template v1`);
   - pre-commit's `CURRENT_HASH`;
   - prek's ID.

   With these, `pre-commit install`/`uninstall` and `prek install` treat it as their own, so the last installer wins and
   no `.legacy` chains form. This was verified with 4.6.2 (OBS-462). Its argv to `hook-impl` is byte-compatible with
   pre-commit's. The fallback chain is `$SNP_BIN` → embedded `SNP_EXE` → `snp` on PATH → `pre-commit` on PATH →
   error with exit 1.
4. A `.legacy` file that is **itself a manager script** is deleted by `snp install` and `snp doctor --fix`. Left in
   place, it causes either pre-commit's `bug: … migration mode` exit 1 or a double run.
5. **Branding is chosen by argv[0].** If the file stem is `pre-commit`, snp runs in compat mode and prints pre-commit
   4.6.2's strings byte for byte, with the listed exceptions (§A.8.2). Any other name gives snp mode.
6. **`--version` output:**
   - `snp --version` prints `snp 2.0.0 (pre-commit 4.6.2 compatible)`.
   - `pre-commit --version`, through any of snp's `pre-commit` entry points, prints `pre-commit 4.6.2 (snp 2.0.0)`.
7. **Store**, in order: `SNP_HOME`, then `$PRE_COMMIT_HOME/snp`, then `$XDG_CACHE_HOME/snp`, then `~/.cache/snp`.
   - snp never shares a directory with pre-commit's store. If the resolved directory *is* pre-commit's store, snp
     refuses to use it.
   - snp never opens pre-commit's `db.db`, `.lock` or `pre-commit.log`.
   - Top-level store entries carry format versions, because `~/.cache/snp` already holds snp 1.x data with its own
     `db.db` and `repos/` (OBS, 174 MB on the dev machine).
8. The crash log is `<store>/snp.log`. The only install-state file is `.snp-install-state.json`. Environment directory
   naming is private to snp, so 03 §20.5 item 9 applies.
9. **Entries.** `entry: pre-commit hazmat …` is rewritten to `[current_exe, "_hazmat", …]`. Usage errors exit 2 and
   runtime errors exit 1 (OBS-462). Other `pre-commit …` entries are **not** rewritten, which matches pre-commit.
10. `snp run` prints a **TTY-only stderr hint** when a default hook type is still owned by pre-commit, prek or snp 1.x.
    `hook-impl` never prints anything extra.
11. **PyPI.** The main distribution is `snp-cli`: maturin `bindings = "bin"`, binary `snp` only. The opt-in
    distribution `snp-pre-commit-compat` provides a `pre-commit` console script and a `pre_commit` import package, so
    `python -m pre_commit` and `"$INSTALL_PYTHON" -mpre_commit` reach snp. File a PEP 541 request for the `snp` name,
    currently a 0.0.0 placeholder with no author, uploaded 2025-02-14.
12. **CI.** `pre-commit/action` cannot run snp, because it `pip install`s pre-commit. Ship `snp-action` with the same
    `extra_args` input. Immutable CI caches **must change their key** when switching tools, or snp's store is never
    saved.
13. **Rollback.** Run `pre-commit install` (Python) in each repo. Python pre-commit recognizes snp's script and
    overwrites it in place (OBS-462). Then run `snp self alias uninstall` and `snp clean`. Configs need no change.
14. **New snp-only commands:** `snp doctor [--fix]` and `snp self alias {install,uninstall,status}`. They are hidden
    from `--help` in compat mode but still accepted there.

---

### A.2 What exists on a user's machine before the swap

`pre-commit install` writes `INSTALL_PYTHON=<shlex.quote(sys.executable)>` into each hook script
(`pre_commit/commands/install_uninstall.py:105`). What happens to that path when pre-commit is removed or upgraded
decides the case in §A.4.

| Install channel | `pre-commit` on PATH | `INSTALL_PYTHON` written into hooks | After removal/upgrade of pre-commit | Case |
|---|---|---|---|---|
| pipx (the dev machine: `~/.local/bin/pre-commit -> ~/.local/share/pipx/venvs/pre-commit/bin/pre-commit`, OBS) | `~/.local/bin/pre-commit` | `~/.local/share/pipx/venvs/pre-commit/bin/python` | `pipx uninstall`: venv deleted. `pipx upgrade`: same path kept | B/C; A |
| `uv tool install pre-commit` | `~/.local/bin/pre-commit` | `~/.local/share/uv/tools/pre-commit/bin/python` | as pipx | B/C; A |
| project venv (`requirements-dev.txt`) | `.venv/bin/pre-commit` (only when activated) | `<proj>/.venv/bin/python` | dependency swapped and venv recreated: python exists, module gone. venv deleted | **A′**; B/C |
| `pip install --user`, system pip, distro package | `~/.local/bin/pre-commit` or `/usr/bin/pre-commit` | shared interpreter, e.g. `/usr/bin/python3` | `pip uninstall` / `apt remove` leaves the interpreter | **A′** |
| Homebrew | `$(brew --prefix)/bin/pre-commit` | versioned Cellar path, `…/Cellar/pre-commit/<ver>/libexec/bin/python` (derived) | `brew upgrade` deletes the old Cellar dir. `brew uninstall` | B (new brew pre-commit, or the alias); B/C |
| conda env | `<env>/bin/pre-commit` | `<env>/bin/python` | package removed; env removed | A′; B/C |
| zipapp `pre-commit-X.pyz` | none (`python pre-commit-X.pyz …`) | `~/.cache/pre-commit-zipapp/<key>/python` (`testing/zipapp/entry:13,51-60`) | cache dir deleted | B/C |
| Windows pip/pipx | `Scripts\pre-commit.exe` (distlib launcher) | `'C:\…\python.exe'` (single-quoted, backslashes literal) | as above | as above |

Data that pre-commit leaves on disk:

| Path | Notes |
|---|---|
| `~/.cache/pre-commit/` | Holds `db.db`, `repo*/`, `README`, `.lock`, `pre-commit.log` and `patch<epoch>-<pid>` files, which are never cleaned. The dev machine holds 803 MB, including two `patch*` files (OBS). |
| `~/.pre-commit/` | Ancient store. `pre-commit clean` removes it (`pre_commit/commands/clean.py:11`). |
| `~/.cache/pre-commit-zipapp/` | zipapp extraction cache. |
| hook scripts | One per installed hook type, per clone, plus the `init.templateDir` copy. |

---

### A.3 pre-commit hook scripts in the wild (all eras must be handled)

`CURRENT_HASH = 138fd403232d2ddd5efb44317e38bf03` has been unchanged since commit `5c90c1a68f` (2018-02-04, "Rewrite the
hook template in python"). It is the same value in `95b8d71bd9` (2020-01-20) and `e622f793c3` (2021-09-27), both
checked with WEB raw fetches. Every pre-commit released since early 2018 therefore recognizes a file that contains this
string.

| Era (introducing commit) | Script language | ID form | How it reaches pre-commit | Parseable fields (for `doctor --fix`, §A.5.3) |
|---|---|---|---|---|
| 2018-02 → 2020-01 (`5c90c1a68f`) | Python (`#!/usr/bin/env python3` or `shebang()`) | `ID_HASH = '138fd…'` | Tries `INSTALL_PYTHON`, then `sys.executable`, each with `-c 'import pre_commit.main'`. Then `<py> -m pre_commit.main run …`, or `pre-commit run …` from PATH. Args: `--config CFG --hook-stage {commit,push,commit-msg}` plus `--commit-msg-filename F` / `--origin SHA --source SHA` / `--all-files` | `CONFIG = '…'`, `HOOK_TYPE = '…'`, `INSTALL_PYTHON = '…'`, `SKIP_ON_MISSING_CONFIG = True\|False` |
| 2020-01 → 2021-09 (`95b8d71bd9`, pre-commit 2.x) | Python | `# ID: 138fd…` | `[INSTALL_PYTHON, '-mpre_commit', *ARGS]` if `os.access(X_OK)`, else `pre-commit` on PATH. `--hook-dir` is `realpath(dirname(__file__))` | `INSTALL_PYTHON = '<py repr>'`, `ARGS = ['hook-impl', '--config=…', '--hook-type=…'(, '--skip-on-missing-config')]` |
| 2021-09 → now (`e622f793c3`; Windows `#!/bin/sh` first line since `e3dc3f7934`, 2022-01) | bash | `# ID: 138fd…` | `pre_commit/resources/hook-tmpl:13-20` | `INSTALL_PYTHON=<shlex.quote>`, `ARGS=(<shlex.join>)` |
| before 2018 (`PRIOR_HASHES`) | bash/python | one of the 5 prior hashes | varies | none: use defaults |

Consequences for snp, reached through a `pre-commit` entry point:
- Era-1 scripts call **`run`**, not `hook-impl`. snp's `run` must accept:
  - `--config`, `--hook-stage commit|push|commit-msg` (legacy stage names, see doc 01);
  - `--commit-msg-filename`;
  - `--origin/-o` and `--source/-s` (aliases of `--to-ref`/`--from-ref`, `pre_commit/main.py:98,109,130`);
  - `--all-files`.

  Era-1 legacy handling happens inside the Python script itself.
- Era-2 and era-3 scripts call `hook-impl … --hook-dir DIR -- <git args>`, exactly like snp's own script (§A.6).

---

### A.4 What runs at `git commit` once snp is installed (OBS-462 precedence matrix)

The pre-commit template's resolution order (`pre_commit/resources/hook-tmpl:13-20`):

```bash
if [ -x "$INSTALL_PYTHON" ]; then
    exec "$INSTALL_PYTHON" -mpre_commit "${ARGS[@]}"
elif command -v pre-commit > /dev/null; then
    exec pre-commit "${ARGS[@]}"
else
    echo '`pre-commit` not found.  Did you forget to activate your virtualenv?' 1>&2
    exit 1
fi
```

| Case | `INSTALL_PYTHON` | `pre-commit` on PATH | What runs at `git commit` | Observed output (OBS-462) | Exit |
|---|---|---|---|---|---|
| **A** | executable, can import `pre_commit` | anything, including snp's alias | **Python pre-commit.** PATH is never consulted. | Normal pre-commit output. A logging stub placed first on PATH as `pre-commit` received **0 calls**. | the hooks' code |
| **A′** | executable, **cannot** import `pre_commit` | anything | nothing; the commit is aborted | `/usr/bin/python3: No module named pre_commit` (stderr, printed by Python). The stub on PATH received 0 calls. | 1 |
| **B** | not executable (deleted venv or Cellar dir) | resolves to snp (alias or compat dist) | **snp `hook-impl`, compat mode** | The stub received argv `hook-impl` `--config=.pre-commit-config.yaml` `--hook-type=pre-commit` `--hook-dir` `<abs>/.git/hooks` `--`. For commit-msg the args end with `--` `.git/COMMIT_EDITMSG`. argv0 was the PATH entry's path. | snp's |
| B′ | not executable | another Python pre-commit | that pre-commit | normal | its |
| **C** | not executable | nothing | nothing; the commit is aborted | `` `pre-commit` not found.  Did you forget to activate your virtualenv? `` (stderr). `git log` unchanged. | 1 |
| D | not executable | none, but `snp` is on PATH | same as C; **the pre-commit template never looks for `snp`** | same as C | 1 |

Notes:
- argv[0] is whatever bash passes for `exec pre-commit`: the bare name for a native binary, or the script path for a
  script. snp must compare the **file stem**, never the whole string (§A.8.1).
- The template passes no `--color`, so `PRE_COMMIT_COLOR`/auto applies (03 §6.1).
- Recursion guard, run against the real 4.6.2 script with `PRE_COMMIT_RUNNING_LEGACY=1` (OBS-462), exits 1 with:
  ```
  bug: pre-commit's script is installed in migration mode
  run `pre-commit install -f --hook-type pre-commit` to fix this

  Please report this bug at https://github.com/pre-commit/pre-commit/issues
  ```
  This is what any pre-commit script sitting in a `.legacy` slot prints when a manager runs it with
  `PRE_COMMIT_RUNNING_LEGACY=1`, which both pre-commit and snp do (`pre_commit/commands/hook_impl.py:22-29,44`).
- **CDPATH bug in pre-commit's template (OBS).** git 2.43 runs hooks as `.git/hooks/<type>` (a relative argv0, OBS). With
  an exported `CDPATH=.`, `HERE="$(cd "$(dirname "$0")" && pwd)"` evaluates to the path **twice, separated by a
  newline**. This happens under both dash and bash. The `--hook-dir` value is then corrupt, and pre-commit's
  `os.access(<dir>/<type>.legacy, X_OK)` is False, so **legacy hooks are silently skipped**. snp's template avoids
  this (§A.6). snp's `hook-impl` repairs such values (§A.7).

**Answer to "what does a user see on the first commit after installing snp?"** It depends on whether they ran
`snp install`:

| User action after installing snp | First `git commit` |
|---|---|
| nothing | Case A: identical to before (Python). Or A′/C: the commit is blocked, if Python pre-commit was also removed. |
| `snp self alias install` + uninstall Python pre-commit (pipx/uv/brew) | Case B: snp in compat mode, byte-identical output, **but a cold cache** (the `[INFO]` lines below), because snp never reuses pre-commit's store. |
| `snp install` (recommended) | snp in snp mode. Same hook lines, same cold-cache `[INFO]` lines on the first run only. |

Cold-cache lines. These are byte-identical in both modes and quoted from pre-commit.com `sections/install.md`:

```
[INFO] Initializing environment for https://github.com/pre-commit/pre-commit-hooks.
[INFO] Installing environment for https://github.com/pre-commit/pre-commit-hooks.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
```

Recommend `snp install --install-hooks` to pay this cost up front. Repos with only `local`/`system` hooks print nothing
extra.

---

### A.5 `snp install`, `snp uninstall` and `snp doctor --fix` against existing scripts

#### A.5.1 Ownership classifier (normative)

Read at most **1 MiB** of the file, following symlinks, and search the bytes. Use `aho-corasick`, or one
`memchr::memmem::Finder` per marker. pre-commit reads the whole file, but hook scripts are under 4 KiB, so the cap
changes nothing in practice. The **first matching row wins**. Order matters, because snp scripts also contain the
pre-commit and prek IDs.

| # | Class | Marker (bytes, anywhere) | Evidence |
|---|---|---|---|
| – | `Missing` | `!lexists(path)` | `install_uninstall.py:76` |
| – | `NotAFile` | a directory (pre-commit: `IsADirectoryError`, exit 3) | 03 §2.5 |
| 1 | `Snp { stale }` | `5bb9bd89cf6c92c58fee7677ceaf4fc3` (current, `stale = false`) or any `SNP_PRIOR_IDS` (`stale = true`) | §A.6 |
| 2 | `SnpV1` | `Generated by SNP`, the substring snp 1.x itself matched | V1 `src/install.rs:765-767` (template header `# Generated by SNP (Shell Not Pass)`), `:527-541` |
| 3 | `PreCommit { era }` | `138fd403232d2ddd5efb44317e38bf03`, `4d9958c90bc262f47553e2c073f14cfe`, `d8ee923c46731b42cd95cc869add4062`, `49fd668cb42069aa1b6048464be5d395`, `79f09a650522a87b0da915d0d983b2de`, `e358c9dae00eac5d06b38dfdb1e33a8c` | `install_uninstall.py:22-30` |
| 4 | `Prek` | `182c10f181da4464a3eec51b83331688` | `prek/crates/prek/src/cli/install.rs:388-400` (`PRIOR_HASHES` is empty) |
| 5 | `Lefthook` (foreign) | `LEFTHOOK` | `lefthook/internal/command/lefthook.go:31,96-119` |
| 6 | `Foreign` | anything else, including a **dangling symlink**, which is not "ours" for pre-commit either (`install_uninstall.py:57`) | – |

The "manager" classes are `Snp`, `SnpV1`, `PreCommit` and `Prek`. For `PreCommit`, `era` is 1, 2, 3 or `Unknown`, by
the shape in §A.3.
- `ARGS=(` gives era 3.
- `ARGS = [` gives era 2.
- `HOOK_TYPE = ` gives era 1.

#### A.5.2 Actions

`T` is a hook type being installed or uninstalled. `L` is `T.legacy`. Unless stated otherwise, pre-commit's own
semantics apply (03 §2-§3).

| `T` class | `snp install` (no `-f`) | `snp install -f` | `snp uninstall` | `snp doctor --fix` |
|---|---|---|---|---|
| Missing | write | write | skip silently (if `T` is missing, `L` is **not** restored, pre-commit quirk) | – |
| `Snp` | overwrite in place, no message | same | remove; `{T} uninstalled` | rewrite if `stale` or if `SNP_EXE` is not executable |
| `SnpV1` | overwrite in place + **M1** | same | remove | rewrite + M1 |
| `PreCommit` | overwrite in place + **M1** (never `.legacy`) | same | remove (snp *is* a pre-commit implementation: same `is_our_script` as pre-commit) | rewrite, **preserving parsed args** (§A.5.3) + M1 |
| `Prek` | overwrite in place + **M1**; if it used prek-only options (`--cd=`, selectors), also print **M1b** | same | **skip silently** (not ours: run `prek uninstall`) | rewrite unless prek-only options are present (then report `cannot adopt`) |
| `Lefthook`/`Foreign` | move to `L` → migration mode (pre-commit semantics; for an existing foreign `L` see 03 §20.3, refuse unless `-f`) | delete `L`, write | skip silently | untouched |

| `L` class (after the `T` step) | `install` | `install -f` | `uninstall` (after removing `T`) | `doctor --fix` |
|---|---|---|---|---|
| `Snp`, `SnpV1`, `PreCommit`, `Prek` | **delete + M2** | delete (pre-commit `-f` semantics) | `Snp`/`SnpV1`: **delete + M12**. `PreCommit`/`Prek`: restore (`Restored previous hooks to {T}`). That is a valid rollback to that tool. | delete + M2 |
| Foreign | keep; print `Running in migration mode with existing hooks at {L}` / `Use -f to use only {prog}.` | delete | restore | untouched |

After processing the requested types, `snp install` scans the other 9 hook types in the same hooks dir. For each one
whose class is `PreCommit`, `Prek` or `SnpV1`, it prints **M3** but does **not** modify it. `snp doctor --fix` is the
command that adopts everything.

The same rules apply to `snp init-templatedir DIR`. It uses `overwrite = true` like pre-commit, so `L` is always
deleted.

Why `.legacy` manager scripts must go:
- **Chain C1** (OBS-462): `pre-commit install` over a prek script moved it to `.legacy`. The commit then ran the prek
  mock with `PRE_COMMIT_RUNNING_LEGACY=1` **and** pre-commit's hooks, a double run. `pre-commit uninstall` restored the
  prek script (`pre-commit uninstalled` / `Restored previous hooks to .git/hooks/pre-commit`).
- **Chain C2**: `prek install` over pre-commit. prek sets only `PREK_RUNNING_LEGACY`, so the legacy pre-commit runs
  fully, another double run. This is prek issues #1620/#1621: "suddenly both were running", and
  `` `.git/hooks/pre-commit` is not managed by prek, skipping. ``
- **Chain C3**: a pre-commit script in `.legacy`, run by snp or pre-commit with `PRE_COMMIT_RUNNING_LEGACY=1`, is the
  `bug:` exit 1 shown in §A.4, so **every commit is blocked**.
- **Chain C4**: snp 1.x backed foreign hooks up to `.legacy` but never ran them (V1 `src/install.rs:380-420,750`). A
  pre-commit script left there by 1.x would block commits as in C3 once new snp runs legacy hooks.

`hook-impl` itself keeps pre-commit's runtime semantics: it runs any executable `L`. Cleanup happens at install or
doctor time only, so parity is unchanged.

#### A.5.3 Argument recovery for `doctor --fix` (and for M1's parenthetical)

| Source | Fields recovered | Parser |
|---|---|---|
| pre-commit era 3 | `INSTALL_PYTHON`, `--config=`, `--skip-on-missing-config` | the line starting `INSTALL_PYTHON=` is one sh word (shlex split, posix); the `ARGS=(` … `)` content is shlex-split |
| pre-commit era 2 | same | `INSTALL_PYTHON = '<repr>'`, `ARGS = [<repr>, …]`: a Python string-literal reader (single or double quotes, `\\`, `\'`, `\"`, `\n`, `\t`, `\xNN`, `\uNNNN`) |
| pre-commit era 1 | `CONFIG`, `INSTALL_PYTHON`, `SKIP_ON_MISSING_CONFIG` | the same reader; `True`/`False` literals |
| prek | `--config="…"`, `--skip-on-missing-config`; **any** `--cd=` or selector token means "cannot adopt" | `prek/…/install.rs` template line `exec "$PREK" hook-impl --hook-dir "$HERE" --script-version 4 [ARGS] -- "$@"` |
| snp 1.x | `--config="…"` (before `run`), `--allow-missing-config` | V1 `src/install.rs:819-836` |
| anything unparseable | defaults: `--config=.pre-commit-config.yaml`, no skip flag; add the note ` (could not read its options; defaults used)` to M1 | – |

The hook type is always the file name, never the parsed value.

#### A.5.4 Messages (stdout, one `writeln` each, in this order per hook type)

| Id | When | Exact text |
|---|---|---|
| M1 | a manager script of another tool is replaced | `Replaced hook script from {owner} at {hook_path}` + (`PreCommit` with a parsed, non-empty `INSTALL_PYTHON`) ` (Python: {INSTALL_PYTHON})`. `{owner}` is `pre-commit`, `prek` or `snp 1.x`. |
| M1b | the replaced prek script used prek-only options | `Note: the replaced prek hook used prek-only options ({opts}); they are not supported and were dropped` |
| M2 | a manager script in `.legacy` is deleted | `Removed {legacy_path}: it is a hook script from {owner}, not a user hook` |
| (pc) | always, after M1/M2 | `{prog} installed at {hook_path}` (`install_uninstall.py:111`) |
| (pc) | foreign `.legacy` kept | `Running in migration mode with existing hooks at {legacy_path}` / `Use -f to use only {prog}.` (`install_uninstall.py:83-86`) |
| M3 | after all requested types, for each other manager-owned type | ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp`` |
| M12 | `uninstall` deletes snp's own script from `.legacy` | `Removed {legacy_path}: it is a hook script from snp, not a user hook` |

Example. `pre-commit` (pipx) had installed `pre-commit` and `pre-push`. The user runs `snp install` with the default
`default_install_hook_types`:

```
Replaced hook script from pre-commit at .git/hooks/pre-commit (Python: /home/u/.local/share/pipx/venvs/pre-commit/bin/python)
snp installed at .git/hooks/pre-commit
Note: .git/hooks/pre-push still runs pre-commit; run `snp install -t pre-push` to switch it to snp
```

---

### A.6 snp's hook script (normative template)

Bytes, with LF line endings on **every** platform, written atomically per 03 §20.3:

```sh
#!/bin/sh
# File generated by snp: https://github.com/grok-rs/snp
# ID: 5bb9bd89cf6c92c58fee7677ceaf4fc3
# pre-commit compat ID: 138fd403232d2ddd5efb44317e38bf03
# prek compat ID: 182c10f181da4464a3eec51b83331688

HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

# start templated
SNP_EXE=/home/u/.cargo/bin/snp
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
else
    echo '`snp` not found.  Did you forget to activate your virtualenv?' 1>&2
    exit 1
fi
```

Rendering rules:
- The templated `set --` line is: `hook-impl`, `--config=<cfg>`, `--hook-type=<T>`, optional `--skip-on-missing-config`,
  then the fixed tail `--hook-dir "$HERE" -- "$@"`. This is **exactly pre-commit's `ARGS` order**
  (`install_uninstall.py:88-90` + `hook-tmpl:11`), so the `pre-commit` fallback and Python pre-commit ≥ 2.0 accept it
  (verified, T2 below).
- Each templated token, and the `SNP_EXE` value, goes through `sh_quote`. This is Python's `shlex.quote`: `''` for
  empty; unchanged if every byte is in `[A-Za-z0-9_@%+=:,./-]`; otherwise `'` + replace(`'` → `'"'"'`) + `'` (03 §2.4).
  Paths are bytes (`OsStr::as_encoded_bytes`) on Unix. On Windows `SNP_EXE` uses forward slashes (`C:/…/snp.exe`).
- The whole set of positional args is rebuilt in **one** `set --`, so git's original `"$@"` is expanded before it is
  replaced. POSIX sh has no arrays; do not split this into two `set --` calls.
- `CDPATH= cd -- "$(dirname -- "$0")"` fixes the pre-commit CDPATH bug (§A.4). Logical `pwd` is used, as in pre-commit.
- The `# … ID:` lines are **load-bearing**: pre-commit's `is_our_script` (any version since 2018) and prek's search the
  bytes anywhere in the file. Never remove the compat IDs. When this template changes, bump the snp ID to
  md5(`snp hook script template v2`) and move the old one into `SNP_PRIOR_IDS`.
- `SNP_BIN` is an override (lefthook's `LEFTHOOK_BIN` idea), used for dev builds and tests. If it is set but not
  executable, the script falls through silently.
- **Fail closed.** If nothing is found, it exits 1, like pre-commit. lefthook's default prints "Can't find lefthook in
  PATH" and exits 0, which lets the commit through; snp must not do that. To bypass, use `git commit --no-verify`.
- The `pre-commit` fallback keeps commits working if snp is removed while Python pre-commit is still installed. That
  is graceful rollback, and the args are compatible.
- `SNP_EXE` selection: see §A.19.3. It prefers a stable PATH entry over a versioned real path (Homebrew Cellar, mise
  `installs/<ver>`; prek issue #2734 shows a mise-versioned path embedded in a prek script).

Verified (OBS-462, dash, `scratchpad/scripts-migration/write_tmpl.sh` with a logging stub as `SNP_EXE`):

| # | Scenario | Result |
|---|---|---|
| T1 | `git commit` via `SNP_EXE`; commit-msg hook | argv `hook-impl --config=.pre-commit-config.yaml --hook-type=commit-msg --hook-dir <abs>/.git/hooks -- .git/COMMIT_EDITMSG` |
| T5 | same as T1 with `CDPATH=.` exported | `--hook-dir` is a single correct line (pre-commit's template gives two lines) |
| T4 | `SNP_BIN=<other>` | `<other>` executed |
| T2 | `SNP_EXE` missing, no `snp` on PATH, real pre-commit 4.6.2 on PATH | Python pre-commit ran the hooks normally, exit 0 |
| T3 | nothing available | `` `snp` not found.  Did you forget to activate your virtualenv? ``, exit 1, no commit |
| T6 | `pre-commit install -t pre-commit -t commit-msg` (4.6.2) over snp scripts | `pre-commit installed at .git/hooks/pre-commit` (and commit-msg), **no `.legacy`**; then `pre-commit uninstall` printed `pre-commit uninstalled` / `commit-msg uninstalled` and removed them |

---

### A.7 `snp hook-impl` when reached from someone else's script

- **Accept pre-commit's argv byte for byte** under either name (03 §6.1). This includes era-2 and era-3 scripts reached
  through `pre-commit` on PATH (Case B), and config-based hooks without `--hook-dir`.
- **Legacy child environment:** set both `PRE_COMMIT_RUNNING_LEGACY=1` and `SNP_RUNNING_LEGACY=1`. Refuse when either is
  already non-empty (`hook_impl.py:22`). The refusal text is chosen by brand and always points at **snp's** tracker:
  - compat mode:
    ```
    bug: pre-commit's script is installed in migration mode
    run `pre-commit install -f --hook-type {T}` to fix this

    Please report this bug at https://github.com/grok-rs/snp/issues
    ```
  - snp mode: the same text with `snp's script` and `` `snp install -f --hook-type {T}` ``.

  The text goes to stderr with exit 1, like pre-commit's `SystemExit`. snp must **never** send its users to
  pre-commit's issue tracker.
- **`--hook-dir` CDPATH repair** (deviation; pre-commit silently skips legacy). If the value contains `\n`, split it on
  `\n`. If all pieces are equal and name an existing directory, use that piece. Otherwise use the value unchanged.
- **No migration hints and no extra output in `hook-impl`.** Hook output must stay byte-identical, and stderr during
  `git commit` is the user's terminal.
- When reached through era-1 scripts, the work is done by `run` (§A.3), not `hook-impl`.

---

### A.8 Invocation names and branding

#### A.8.1 Brand detection (normative)

```
arg0 = std::env::args_os().next()            // NOT current_exe(): Linux resolves symlinks, which would hide the alias
name = Path::new(&arg0).file_name()          // basename, OsStr
stem = name minus a trailing ".exe" (ASCII case-insensitive)
brand = if stem == "pre-commit" (ASCII case-insensitive on Windows, exact on Unix) { Compat } else { Snp }
```

There is no environment override. Every compat entry point either passes argv0 `pre-commit`, or *is* a file named
`pre-commit[.exe]`:
- symlink or hard link;
- `exec pre-commit` from the pre-commit template;
- `snp-pre-commit-compat`, which sets argv0 explicitly (§A.9.4).

The differential harness in doc 10 runs snp through a `pre-commit`-named symlink to get compat mode. This settles
10 Q1: compat mode is chosen by argv0 **only**. The "or when `.pre-commit-config.yaml` is used" variant is rejected,
because it would make snp mode impossible.

#### A.8.2 Rules

- **B1 (compat mode).** Every byte equals pre-commit 4.6.2's output, except:
  - (E1) the `--version` suffix;
  - (E2) bug-report URLs point to snp;
  - (E3) store-derived paths, the crash-log name and the crash-log body (separate store, §A.10);
  - (E4) the `minimum_pre_commit_version` hint sentence;
  - (E5) snp-only messages M1-M3, M12 and the hints of §A.14, plus snp-only commands;
  - (E6) `clean` cleans snp's store only.
- **B2 (snp mode).** Replace `pre-commit` with `snp` only where the string (i) names a command to type, or (ii) is the
  tool talking about itself. Never touch:
  - file names (`.pre-commit-config.yaml`, `.pre-commit-hooks.yaml`);
  - environment variable names (`PRE_COMMIT_*`, `SKIP`);
  - the phrases `pre-commit hook(s) made changes.` and `Your pre-commit configuration is unstaged.` (they describe the
    framework and config format, and CI log greps match them);
  - `https://pre-commit.com/...` documentation URLs (the config format is still documented there).

| Source (REF) | Compat mode | snp mode |
|---|---|---|
| argparse `prog='pre-commit'` (`main.py:204`); usage line and `pre-commit: error: unrecognized arguments: --bogus` (OBS-462, exit 2; also via `python -m pre_commit`) | `pre-commit` | `snp` (`snp: error: …`, `usage: snp …`) |
| `{prog} installed at {p}` (`install_uninstall.py:111`) | `pre-commit installed at …` | `snp installed at …` |
| `Use -f to use only pre-commit.` (`:85`) | same | `Use -f to use only snp.` |
| hook-impl recursion `bug: …` (`hook_impl.py:24-28`) | §A.7 (E2) | §A.7 |
| ``Skipping `pre-commit`.`` (`hook_impl.py:56`) | same | ``Skipping `snp`.`` |
| missing-config block (`hook_impl.py:59-66`) | same | `- To permanently silence this, install snp with the --allow-missing-config option` / `` - To uninstall snp run `snp uninstall` `` (the `PRE_COMMIT_ALLOW_NO_CONFIG=1` line is unchanged) |
| `run.py:307-311` show-diff banner | same | line 1 unchanged; `` reproduce locally with: `snp run --all-files`. `` / ``To run `snp` as part of git workflow, use `snp install`.`` |
| `` Hint: often `pre-commit autoupdate --repo …` `` (`clientlib.py:106`), `` `pre-commit migrate-config` `` (`:158,184`), `` Hint: `pre-commit autoupdate` often fixes this. `` (`:309`), `` Often `pre-commit autoupdate` fixes this. `` (`repository.py:185`) | same | `snp autoupdate …`, `snp migrate-config` |
| `pre-commit normalizes …` (`clientlib.py:326,344`), `pre-commit failed to diff` (`staged_files_only.py:103`), `for now, pre-commit requires/does not support …` (`lang_base.py:97,108`), `pre-commit requires system-installed "cs"…` (`languages/coursier.py:34`), `pre-commit has detected a mix of cygwin…` (`git.py:223`), `Locking pre-commit directory` (`store.py:106`) | same | `snp …` |
| `Try upgrading identify and pre-commit?` (`clientlib.py:46`) | same | `Try upgrading snp?` |
| min-version message (`clientlib.py:58-60`) | §A.8.4 (E4) | §A.8.4 |
| help strings naming the tool (`main.py:222,245,273,290,299,335`) | same | `{prog}` substituted (Tier B) |

Implementation: a `Brand` value is computed once in `main` and passed to a message catalog. There are about 30 strings.
Golden tests cover both brands.

#### A.8.3 `--version` (`-V`), stdout, `\n`-terminated, exit 0

| Invoked as | Output |
|---|---|
| `snp` (any non-`pre-commit` name) | `snp 2.0.0 (pre-commit 4.6.2 compatible)` |
| `pre-commit` (alias, hard link, compat dist, `python -m pre_commit` via the compat dist, the pre-commit template's `exec pre-commit`) | `pre-commit 4.6.2 (snp 2.0.0)` |

Reasoning. Python always prints `pre-commit 4.6.2`, because `prog` is fixed (`main.py:204-210`; OBS-462: `python -m
pre_commit --version` gives `pre-commit 4.6.2`). Keeping `pre-commit 4.6.2` as the first two fields keeps
`awk '{print $2}'`, `split()[1]` and first-match semver regexes working. The suffix answers "which tool is this?",
which is the most common question during coexistence. Exact-equality and `$NF` parsers break; see the open questions.
`--version` is Tier B in doc 10.

#### A.8.4 `minimum_pre_commit_version`

- Comparison value: `PC_COMPAT_VERSION = "4.6.2"`, bumped only when snp implements that pre-commit release.
  `SNP_VERSION` is never compared.
- Compat text: ``pre-commit version {required} is required but version 4.6.2 is installed.  Perhaps upgrade snp, which provides this `pre-commit` (snp {SNP_VERSION}).``
  The first sentence is byte-identical to `clientlib.py:58-59`. The original `` Perhaps run `pip install --upgrade
  pre-commit`. `` would upgrade the wrong tool.
- snp-mode text: `pre-commit version {required} is required but snp {SNP_VERSION} is compatible with pre-commit 4.6.2.  Perhaps upgrade snp.`

#### A.8.5 Help

In compat mode, `--help` and `help` list exactly pre-commit's commands (Tier B text). The snp-only commands (`doctor`,
`self`) are **hidden but accepted**. In snp mode they are listed.

#### A.8.6 Hook `entry` values that invoke `pre-commit`

| Entry form | pre-commit behavior (REF/OBS) | snp |
|---|---|---|
| `pre-commit hazmat <tool> …` (first two shlex tokens) | Rewritten to `sys.executable -m pre_commit.commands.hazmat …` (`lang_base.py:174-178`). **No `pre-commit` on PATH needed** (OBS-462: passed with `PATH=/usr/bin:/bin`). | Rewrite to `[current_exe(), "_hazmat", …]`. `_hazmat` is a hidden subcommand under both brands. |
| `pre-commit <anything else>` with `language: system`/`unsupported` | Resolved on PATH. If missing, the hook fails with ``Executable `pre-commit` not found`` and `- exit code: 1` (OBS-462). | Same, **not rewritten**: it works through the alias or the compat dist, and fails like pre-commit otherwise. |
| `pre-commit validate-manifest` in `language: python` hooks from `https://github.com/pre-commit/pre-commit` (`.pre-commit-hooks.yaml:1-6`) | The hook venv installs real pre-commit and resolves it there. | **Must not** be rewritten. It runs the pinned Python pre-commit inside the hook env, independent of migration. |
| meta hooks (`clientlib.py:355`, `sys.executable -m pre_commit.meta_hooks.*`), pygrep (`languages/pygrep.py:100`) | Python subprocesses. | Native in snp (docs 02, 09); no Python needed. |

`_hazmat` exit codes. This resolves the "hazmat failure exit code" contradiction. Verified with OBS-462 in `run` output:

| Situation | Entry path (`_hazmat`) | Direct CLI (`{prog} hazmat …`) |
|---|---|---|
| unknown tool (argparse) | exit **2**; stderr `usage: hazmat.py [-h] {cd,ignore-exit-code,n1} ...` / `hazmat.py: error: argument tool: invalid choice: 'bogus' (choose from 'cd', 'ignore-exit-code', 'n1')` | exit **2**; `usage: pre-commit hazmat [-h] [--color {auto,always,never}]` (wrapped) / `pre-commit hazmat: error: argument tool: …` |
| entry missing trailing `--` (`hazmat.py:39`) | exit 1; `` hazmat entry must end with `--` `` | exit 1, same text |
| `cd` with a filename outside the subdir (`hazmat.py:51`) | exit 1; `unexpected file without prefix='sub/': {f}` | exit 1, same |
| command not found (`ExecutableNotFoundError`, `parse_shebang.py:10-12`) | exit **1**. Python prints a traceback ending in ``pre_commit.parse_shebang.ExecutableNotFoundError: Executable `definitely-not-a-cmd` not found``. snp prints **only that last line** (Tier B). No crash log. | exit **3**; ``An unexpected error has occurred: ExecutableNotFoundError: Executable `x` not found`` / `Check the log at {store}/snp.log` |

03 §15's "surfaces as exit 3" holds **only** for the direct CLI.

---

### A.9 Delivering the `pre-commit` name (opt-in only)

#### A.9.1 Policy

Shadowing `pre-commit` depends on PATH order. Doing it silently would make "I installed snp to try it" switch every
repo on the machine (Case B) or none of them (Case A), unpredictably. Precedents for an explicit opt-in: Debian
`podman-docker`, and Docker Desktop's Compose V1 compatibility toggle. Therefore:
- **cargo**: one `[[bin]]`, `snp`. No `pre-commit` bin target.
- **cargo-dist installers**: do **not** use `[dist.bin-aliases]`. It exists since 0.14 and creates the alias
  unconditionally: a symlink from the shell installer, a hard link from PowerShell, `bin.install_symlink` from
  Homebrew (WEB, `book/src/reference/config.md:791-810`).
- **Homebrew formula**: installs only `snp`. No `conflicts_with "pre-commit"` is needed.
- **PyPI main wheel**: only `snp` (§A.9.3).
- The name is created by **`snp self alias install`**, or provided by the **`snp-pre-commit-compat`** distribution.

#### A.9.2 `snp self alias install|uninstall|status`

| Subcommand | Behavior |
|---|---|
| `install [--dir DIR] [--force]` | `DIR` defaults to the parent of the stable snp path (§A.19.3), e.g. `~/.cargo/bin` or `$(brew --prefix)/bin`. Unix: symlink `DIR/pre-commit` → absolute stable snp path. Windows: hard link `DIR\pre-commit.exe` → `snp.exe` (no privilege needed; same volume). Falls back to a copy if the volumes differ. If `DIR/pre-commit[.exe]` exists and is **not** snp (§A.19.4), print the refusal message and exit 1. With `--force`, rename it to `<name>.snp-backup` (refuse if that already exists), then create the alias. After creating it, resolve `pre-commit` the way `command -v` does (§A.19.5). If the result is not the alias, print the shadow warning (exit 0). |
| `uninstall [--dir DIR]` | Remove the alias only if it is snp. Restore `<name>.snp-backup` if present. |
| `status` | One line: `pre-commit -> {resolved path} ({kind})`. `{kind}` is one of `snp alias (this snp)`, `snp alias (snp {v})`, `Python pre-commit {v}`, `snp-pre-commit-compat`, `unknown`, `not found`. |

Messages (stdout unless noted):
- `Created {alias} -> {target}`, `Created {alias} (hard link to {target})`, `Created {alias} (copy of {target})`
- refusal (stderr, exit 1): ``{alias} already exists and is not snp ({kind}); use --force to replace it (it will be kept as {alias}.snp-backup)``
- shadow warning (stderr): ``warning: `pre-commit` resolves to {other} on PATH, which comes before {DIR}; typing `pre-commit` will not run snp``
- `Removed {alias}`, `Restored {alias} from {alias}.snp-backup`, and ``{alias} is not an snp alias; leaving it alone`` (stderr, exit 1)

Windows caveat: a hard link goes stale when an updater replaces `snp.exe` by rename. `snp doctor` and `status` detect
this by file identity and print `snp alias (snp {v})` with a version mismatch. The fix is to re-run
`snp self alias install --force`.

The alias must be a `.exe`. Git Bash's `command -v pre-commit` (used by the pre-commit template) finds `pre-commit.exe`,
not `.cmd`/`.bat`/`.ps1`. This is derived, not verified on Windows. Batch wrappers would also reintroduce
argument-reparsing hazards.

#### A.9.3 PyPI main distribution

- Name: **`snp-cli`**. `snp` is taken by a placeholder: version 0.0.0, no author, no summary, files uploaded
  2025-02-14 (WEB, PyPI JSON).
  - `snp-cli`, `you-shell-not-pass`, `snp-hooks` and `snp-pre-commit-compat` all returned 404 on 2026-10-08.
  - `snp-cli` follows the `ast-grep-cli` precedent: tool name plus `-cli` when the bare name is unavailable.
  - Also file a **PEP 541** request for `snp`: an empty placeholder is an "invalid project" candidate. If it succeeds,
    publish `snp` and keep `snp-cli` as a dependency-only alias.
- maturin `bindings = "bin"`. The wheel contains only `snp` (`snp.exe`). `pipx install snp-cli`, `uv tool install
  snp-cli` and `uvx --from snp-cli snp …` all work.
- Document that `uvx snp` / `pip install snp` fetch the unrelated placeholder.
- conda-forge lesson (prek #2138): a conda package that ships only the binary is invisible to pip. Build the conda
  package from the wheel, or include the `.dist-info`.

#### A.9.4 `snp-pre-commit-compat` (opt-in, pure-Python wheel, published with every release)

Purpose: in Python workflows, swapping one requirement swaps the tool, with no other edits:
- `deps = pre-commit` in tox/nox, and `requirements-dev.txt`;
- `python -m pre_commit …` in Makefiles;
- **existing hook scripts whose `INSTALL_PYTHON` is the project venv**. This fixes Case A′, where the dependency was
  swapped and the venv recreated: `.venv/bin/python -mpre_commit` now reaches snp.

| Item | Spec |
|---|---|
| Metadata | `Requires-Dist: snp-cli=={SNP_VERSION}` (exact pin), `Requires-Python: >=3.9`, `py3-none-any` |
| Files | `pre_commit/__init__.py` (empty), `pre_commit/__main__.py` (calls `snp_pre_commit_compat._launch.main()`), `snp_pre_commit_compat/__init__.py`, `snp_pre_commit_compat/_launch.py` |
| Console script | `pre-commit = snp_pre_commit_compat._launch:main` |
| `_launch.main()` | 1. `SNP_BIN` env if non-empty. 2. `os.path.join(sysconfig.get_path('scripts'), 'snp.exe' if os.name == 'nt' else 'snp')` if it is a file; this is where `snp-cli` was installed for this interpreter. 3. `shutil.which('snp')`. 4. Otherwise print to stderr `` `snp` not found (snp-pre-commit-compat needs snp-cli in the same environment) `` and exit 1. POSIX: `os.execv(snp, ['pre-commit', *sys.argv[1:]])`. Windows: `signal.signal(signal.SIGINT, signal.SIG_IGN)`, then `sys.exit(subprocess.call(['pre-commit', *sys.argv[1:]], executable=snp))`. CreateProcess with `lpApplicationName = snp.exe` and a command line starting with `pre-commit` gives snp argv0 `pre-commit`, so compat mode. Import only `os`, `sys`, `sysconfig`, `shutil` (and `subprocess`/`signal` on Windows) to keep start-up near 20 ms. |
| Hard rule | **Never install alongside the real `pre-commit`.** Both own `pre_commit/__init__.py` and `__main__.py`, so pip silently overwrites, and uninstalling one breaks the other. Order for switching: `pip uninstall pre-commit && pip install snp-pre-commit-compat`. Rollback is the reverse order. Document this on the PyPI page. `snp doctor` flags it (§A.15). |
| Not provided | `pre_commit.main`, `pre_commit.constants` and other internals. Tools importing pre-commit's Python API keep needing real pre-commit. |

#### A.9.5 Other channels

- **npm**: not planned.
- **Docker image**: `snp` only. A `pre-commit` symlink inside the image is acceptable, because the image is single-purpose.
- **crates.io**: `snp` ≥ **2.0.0** (V1 occupies 1.0.0 and 1.0.1). `cargo install snp` replaces the stale
  `~/.cargo/bin/snp` 1.0.0, which matters because hook-script fallbacks resolve `snp` on PATH.

---

### A.10 Store, cache, locks and logs: one rule (resolves the store contradictions)

#### A.10.1 Location

```
fn snp_store_dir(env, invocation_cwd) -> Result<PathBuf>:
    raw, source = first of:
        SNP_HOME            (non-empty)  -> SNP_HOME
        PRE_COMMIT_HOME     (non-empty)  -> PRE_COMMIT_HOME / "snp"
        XDG_CACHE_HOME      (non-empty)  -> XDG_CACHE_HOME / "snp"
        otherwise                        -> home_dir()? / ".cache" / "snp"     // Windows: %USERPROFILE%\.cache\snp; macOS: ~/.cache/snp
    path = absolute(raw against invocation_cwd)   // before any chdir to the toplevel, as pre-commit (store.py:25-36)
    path = realpath_lenient(path)                 // canonicalize the longest existing prefix, append the rest (= os.path.realpath)
    if path == pre_commit_store_dir(env) or first_line(path/"README") == "This directory is maintained by the pre-commit project."
        -> FatalError M7
```

- "Non-empty" follows pre-commit's `os.environ.get(...) or` semantics (`store.py:32-35`): an empty string means unset.
- `pre_commit_store_dir` is pre-commit's own rule: `PRE_COMMIT_HOME`, else `$XDG_CACHE_HOME/pre-commit`, else
  `~/.cache/pre-commit`, realpath'd. snp computes it only for this comparison and for `doctor`.
- `home_dir()` is `std::env::home_dir()`: `$HOME` then passwd on Unix, `USERPROFILE` on Windows. This matches Python's
  `expanduser`. If there is none: `FatalError` `cannot determine the home directory; set SNP_HOME`.

| `SNP_HOME` | `PRE_COMMIT_HOME` | `XDG_CACHE_HOME` | snp store | pre-commit store |
|---|---|---|---|---|
| – | – | – | `~/.cache/snp` | `~/.cache/pre-commit` |
| – | – | `/x` | `/x/snp` | `/x/pre-commit` |
| – | `/p` | any | **`/p/snp`** | `/p` |
| `/s` | any | any | `/s` | its own rule |
| `~/.cache/pre-commit` (or any value equal to pre-commit's store) | – | – | **refused (M7)** | `~/.cache/pre-commit` |

Why `$PRE_COMMIT_HOME/snp`:
- **Existing CI cache steps keep working.** pre-commit's documented GitLab and Azure recipes set `PRE_COMMIT_HOME` and
  cache exactly that directory (WEB `sections/advanced.md:803-891`). snp's store then lives inside the cached directory
  with no YAML edit, apart from the cache key (§A.11.3).
- **Both tools stay safe** (OBS-462). With `$PRE_COMMIT_HOME/snp/{.lock,repos/abc}` present, `pre-commit gc` printed
  `0 repo(s) removed.` and left it untouched, because gc works from `db.db` rows only. `pre-commit clean` removed the
  whole `$PRE_COMMIT_HOME` including `snp/` (`Cleaned …`), which is acceptable for "clean pre-commit files". snp
  recreates its store lazily.
- 10 Q5 ("honor `PRE_COMMIT_HOME` exactly") is **rejected**. It would mix layouts in one directory, make `snp clean`
  delete pre-commit's `db.db`, and share `.lock`.
- 06 §8 ("read `PRE_COMMIT_HOME` only for clean/gc") is **rejected**. snp never cleans pre-commit's data.

#### A.10.2 Invariants

- snp never opens, creates or deletes pre-commit's `db.db`, `.lock`, `README`, `pre-commit.log`, `repo*` dirs or
  `patch*` files, or `~/.pre-commit`, or `~/.cache/pre-commit-zipapp`. There is **no** `rusqlite` dependency.
- **Lock interop is moot.** The stores are disjoint, so snp takes only `<snp store>/.lock`. 04a §14.10's "must also hold
  pre-commit's `<store>/.lock`" and 07 D7's overlap analysis apply only to a shared store, which is forbidden.
- **Crash log**: `<snp store>/snp.log`, never `pre-commit.log`. This resolves 03 §20.5 item 13, 02 Q7 and 10 Q9. The
  console line is `Check the log at {path}`, the same wording (`error_handler.py:32`). The body keeps pre-commit's
  skeleton, with these lines in place of `pre-commit version:` and the `sys.*` lines:
  ~~~
  ### version information

  ```
  snp version: 2.0.0 (pre-commit 4.6.2 compatible)
  invoked as: pre-commit
  git --version: git version 2.43.0
  executable: /home/u/.cargo/bin/snp
  target: x86_64-unknown-linux-gnu
  os: linux
  ```

  ### error information
  …
  ~~~
  `invoked as` is `snp` or `pre-commit`.
- **Install state**: only `<env>/.snp-install-state.json`, as in 03 §20.5 item 8. Do not write pre-commit's
  `.install_state_v1`/`_v2` or `.snp_state.json` (04a §11.4 step 7 and §14.9 are superseded). Nothing reads
  pre-commit-format state in snp's store (`repository.py:25-106` reads it only from pre-commit's own store).
- **Env-dir naming** is private, so 03 §20.5 item 9 applies: `<ENV_DIR>-<version>` if the version matches
  `^[A-Za-z0-9._+-]{1,64}$`, else `<ENV_DIR>-h<hash>`. 04a §14.2's literal naming was needed only for a shared store.
- **README**, written only when snp creates the directory, 2 lines:
  ```
  This directory is maintained by snp (You Shell Not Pass).
  Learn more: https://github.com/grok-rs/snp
  ```
  If snp has to create `$PRE_COMMIT_HOME` itself, pre-commit later skips its own README, because it writes one only when
  it creates the directory (`store.py:70-76`). This is harmless.
- **Lazy creation** (03 §20.5 item 7). Read-only commands never create the store. A read-only store works if
  everything needed is present (parity with `store.py:65-68,226`). Config tracking is skipped there.
- If the store cannot be created: **M8**, exit 1. snp never falls back silently to another location.

#### A.10.3 Versioned top-level entries (because snp 1.x already uses `~/.cache/snp`)

OBS on the dev machine: `~/.cache/snp` contains `README` ("This directory is maintained by the SNP (Shell Not Pass)
project." / "Learn more: https://github.com/devops247-online/snp"), `db.db`, `repos/repo_<16hex>/`,
`environments/python-3.12.3-<16hex>/` and `resource-pools/`, 174 MB in total. V1 `src/storage.rs:112-135` used
`SNP_HOME`, then `$XDG_CACHE_HOME/snp`, then `$HOME/.cache/snp`, the same default.

Rules:
- Every top-level directory snp creates carries a format suffix, `<name>-v<N>`: `repos-v1/`, `configs-v1/`, …. The
  exact names belong to 03 §20.5, with the suffix added. Unsuffixed entries are `README`, `.lock`, `snp.log`,
  `scratch/` and `patches/`.
- When a format changes, bump `N`. The old directory is ignored, never parsed. This is the uv approach (`wheels-v5`,
  `archive-v0`).
- Reserved names snp must **never create**: `db.db`, `repos`, `environments`, `resource-pools`.
- `snp gc` removes leftover snp 1.x data **only** when the README's first line is the 1.x signature above.
  - It deletes `db.db`, `repos/`, `environments/` and `resource-pools/`, and rewrites the README.
  - It then prints `Removed snp 1.x cache data from {store}.` before `{n} repo(s) removed.`
  - It also deletes unknown `*-v<N>` directories whose `N` is not current.
- `snp clean` removes the whole snp store (1.x leftovers included) and prints `Cleaned {store}.`. In compat mode it
  still never touches pre-commit's directories (E6).

#### A.10.4 Relocated caches (robustness extension)

pre-commit's GitLab recipe warns that the cache "requires to be served from a constant location". With k8s runners
the build dir moves, and absolute paths baked into `db.db` and venvs give `InvalidManifestError` (WEB
`sections/advanced.md:888-891`). snp's equivalent:
- Write `<store>/store-v1.json` = `{"path": "<abs store path>"}` when the store is created.
- If the current path differs, keep the clones (git repos relocate fine), treat **environments** as missing, and print
  once: `[INFO] snp store moved from {old} to {new}; hook environments will be reinstalled.`
- Then update the file.

---

### A.11 CI integrations

#### A.11.1 `pre-commit/action` (v3.0.1, maintenance-only; WEB `action.yml:1-20`)

```yaml
inputs:
  extra_args: {description: options to pass to pre-commit run, required: false, default: '--all-files'}
runs:
  using: composite
  steps:
  - run: python -m pip install pre-commit                    # always installs real pre-commit
  - run: python -m pip freeze --local
  - uses: actions/cache@v4
    with:
      path: ~/.cache/pre-commit
      key: pre-commit-3|${{ env.pythonLocation }}|${{ hashFiles('.pre-commit-config.yaml') }}
  - run: pre-commit run --show-diff-on-failure --color=always ${{ inputs.extra_args }}
```

Analysis:
- The action installs Python pre-commit into the setup-python interpreter, which is first on PATH. It cannot run snp
  unless another directory is prepended to PATH, which is fragile.
- The compat distribution does not help: the `pip install pre-commit` step overwrites `bin/pre-commit` and the
  `pre_commit` package files.
- **Decision**: users replace the action. snp needs no Python and no pip. The replacement keeps the same flags and exit
  codes, and the hook output is identical. In snp mode only the branded lines differ, for example the show-diff
  banner's ``reproduce locally with: `snp run --all-files`.``

Replacement steps, calling `snp` directly:

```yaml
- uses: actions/checkout@v7
- uses: taiki-e/install-action@v2        # or the snp release installer / cargo-binstall
  with: {tool: snp}
- uses: actions/cache@v4
  with:
    path: ~/.cache/snp
    key: snp-1|${{ runner.os }}|${{ runner.arch }}|${{ env.pythonLocation }}|${{ hashFiles('.pre-commit-config.yaml') }}
- run: snp run --show-diff-on-failure --color=always --all-files
```

`grok-rs/snp-action`, a composite action to ship:
- Inputs: `extra_args` (default `'--all-files'`, same name and default as pre-commit/action), `version` (default:
  latest release), `cache` (default `'true'`).
- Steps:
  1. Download the release archive for `runner.os`/`runner.arch` and verify its SHA-256 against the release checksums.
  2. Add it to `$GITHUB_PATH`.
  3. Cache `~/.cache/snp` with the key above.
  4. `snp run --show-diff-on-failure --color=always ${{ inputs.extra_args }}`.

  Migrating is then a one-line `uses:` change.
- Cache-key semantics:
  - `snp-1` is the key-scheme version, like `pre-commit-3`.
  - `runner.os` and `runner.arch` are needed because environments are platform binaries.
  - `pythonLocation` matters only for Python-language hooks (empty without setup-python), as in pre-commit.
  - Do **not** put the snp version in the key: versioned buckets (§A.10.3) make old data harmless, and a per-release
    key would force cold caches.

#### A.11.2 pre-commit.ci (01 §3.7)

- pre-commit.ci runs **Python pre-commit** on its own infrastructure. snp cannot be plugged in. Coexistence is purely a
  config contract:
  - snp must accept and preserve the `ci:` mapping without warnings, and keep it intact through `autoupdate` and
    `migrate-config` rewrites.
  - While pre-commit.ci (or any teammate) runs Python pre-commit, `.pre-commit-config.yaml` must stay valid for the
    latest Python pre-commit:
    - **no snp-only keys or values that change semantics** there;
    - unknown keys only warn in pre-commit (`Unexpected key(s) present …`);
    - unknown `language`/`stages` values are hard errors.
  - snp extensions belong in a separate file. That is out of scope here.
  - `snp autoupdate` must produce byte-identical `rev:` rewrites (03 §11). Otherwise local `snp autoupdate` and
    pre-commit.ci's weekly autoupdate commits ping-pong diffs.
  - `ci.skip` uses hook ids, as `SKIP=` does.

#### A.11.3 Other CI systems (recipes from WEB `sections/advanced.md:771-897`)

| Recipe (pre-commit docs) | Cache path / key | Works unchanged with snp? | Required edit |
|---|---|---|---|
| GitLab: `PRE_COMMIT_HOME: ${CI_PROJECT_DIR}/.cache/pre-commit`, `cache: paths: [${PRE_COMMIT_HOME}]` | mutable cache, re-uploaded each job (`pull-push`) | **yes**: snp uses `${PRE_COMMIT_HOME}/snp` | replace `pre-commit` with `snp` in `script:` (or use the alias) |
| Azure: `PRE_COMMIT_HOME: $(Pipeline.Workspace)/pre-commit-cache` + `CacheBeta@0` key `pre-commit \| .pre-commit-config.yaml \| "$(PY)"` | **immutable** | path yes, key **no** | add a token, e.g. `snp \| pre-commit \| …`, so the first snp run saves |
| CircleCI: `~/.cache/pre-commit`, key `v1-pc-cache-{{ checksum … }}` | immutable | no | path `~/.cache/snp` (or set `PRE_COMMIT_HOME: ~/.cache/pre-commit`), and bump the key prefix |
| GitHub `actions/cache`: `~/.cache/pre-commit`, key `pre-commit\|${{ env.PY }}\|…` | immutable | no | as §A.11.1 |
| Travis `$HOME/.cache/pre-commit`; AppVeyor `%USERPROFILE%\.cache\pre-commit` | path-keyed | no | change the path to `…/.cache/snp`, or set `PRE_COMMIT_HOME` to the cached directory |
| tox: `commands = pre-commit run --all-files`, `passenv = SSH_AUTH_SOCK` (and proxies) | – | with `deps = snp-pre-commit-compat` instead of `pre-commit`: **yes** | or `deps = snp-cli` + `commands = snp run --all-files`; also add `SNP_HOME`/`PRE_COMMIT_HOME` to `passenv` if used |

**Immutable-cache trap.** This is the critical checklist item. With an immutable cache (GitHub, Azure, CircleCI), a key
already saved by pre-commit runs is **restored on a hit and never re-saved**. After switching, snp's `snp/` subdirectory
(or `~/.cache/snp`) is therefore never persisted, and every CI run is cold until the config hash changes. **Any switch
must change the cache key.**

#### A.11.4 CI-relevant snp guarantees

- `snp run --show-diff-on-failure --color=always --all-files` gives the same exit codes and hook output as pre-commit.
- `CI`, `GITHUB_ACTIONS` and similar are not read by default (03 §20.5 item 11 uses `CI` only for clone retries).
- Cold cache lines are those of §A.4.

---

### A.12 git ≥ 2.54 config-based hooks and `init.templateDir`

- Config hooks (03 §6.1). pre-commit's docs prescribe the friendly names `hook.pre-commit.<type>` with
  `command = 'pre-commit hook-impl --hook-type <type> --'` (WEB `sections/advanced.md:448-512`).
  - These resolve `pre-commit` on **PATH** at hook time. They reach snp only through the alias or the compat dist.
    Without either, they keep running Python.
  - Migration: `git config set [--global] hook.pre-commit.<type>.command '<snp> hook-impl --hook-type <type> [--skip-on-missing-config] --'`,
    under the **same friendly name**, so the entry is replaced in place, not duplicated.
  - If a hooks-dir manager script **and** a config hook both call a manager for the same event, git runs both. The
    hooks-dir script runs last. `snp doctor` reports `error` for that.
- `init.templateDir`. `pre-commit init-templatedir DIR` writes Python pre-commit scripts with `--skip-on-missing-config`
  into `DIR/hooks/` (03 §5). **Every future clone** gets them.
  - Migration: `snp init-templatedir DIR`. This overwrites recognized scripts; `overwrite = true` deletes `.legacy`.
  - Clones made earlier keep their copies. Fix them per repo with `snp install` or `snp doctor --fix`, or rely on
    Case B after Python pre-commit is removed.

---

### A.13 Rollback, uninstall and `.legacy` chains

#### A.13.1 Back to Python pre-commit (exact steps and output)

1. Reinstall Python pre-commit if needed: `pipx install pre-commit`, `uv tool install pre-commit`, `pip …` or `brew …`.
2. `snp self alias uninstall` prints `Removed /home/u/.cargo/bin/pre-commit` and restores a backup if one exists.
   Compat-dist users run `pip uninstall snp-pre-commit-compat` **first**, then `pip install pre-commit`.
3. In each repo, run `pre-commit install [-t …]`. Expected output (OBS-462, T6): `pre-commit installed at
   .git/hooks/pre-commit`. There is **no** migration message and no `.legacy`, because snp's script carries
   `CURRENT_HASH`.
   - Alternative: `snp uninstall` first. It prints `{T} uninstalled` (`pre-commit uninstalled` for the pre-commit
     type) and restores a foreign or manager `.legacy`.
4. Run `pre-commit init-templatedir DIR` if a template dir is used, and revert `hook.pre-commit.*.command` config hooks.
5. In CI, restore `pre-commit/action` or the old commands, and **change the cache key again**.
6. Run `snp clean` (prints `Cleaned {snp store}.`). pre-commit's store was never touched. If the user deleted it,
   pre-commit re-initializes.
7. Configs need nothing. snp never writes snp-only syntax into `.pre-commit-config.yaml`, and its autoupdate and
   migrate-config output equals pre-commit's.

Rollback **while both are installed** needs only step 3. That is "last installer wins".

#### A.13.2 prek interplay

- snp recognizes prek's ID `182c10f181da4464a3eec51b83331688` (06 §2.13) and replaces prek scripts in place (M1).
- snp's script embeds that ID, so `prek install` over snp replaces it in place too. prek's `is_our_script` reads with
  `read_to_string`, so a non-UTF-8 `SNP_EXE` path would make prek's install fail with an I/O error. This is rare.
- prek never honours `PRE_COMMIT_RUNNING_LEGACY` (it uses `PREK_RUNNING_LEGACY`, 06 §2.13 and §4). That is why a prek
  `.legacy` double-runs.
- `snp uninstall` leaves prek scripts alone.
- Lessons adopted from prek's issues:
  - **#1620/#1621**: cross-tool `.legacy` chains are confusing, so snp deletes manager `.legacy` files.
  - **#2734**: users did not understand "Migration mode: … will also run legacy hook" when the script showed no sign of
    it, so snp keeps pre-commit's wording and `snp doctor` shows the chain explicitly.
  - **#1700**: legacy hooks must actually run, so snp keeps pre-commit's semantics.

---

### A.14 Warnings and hints (exact conditions)

| Where | Condition (all must hold) | Output |
|---|---|---|
| `{prog} run`, started directly (not via `hook-impl`) | `SNP_NO_HINTS` unset or empty; **stderr is a terminal**; `core.hooksPath` unset; for some `T` in the config's `default_install_hook_types`, `<common-dir>/hooks/T` is `PreCommit`, `Prek` or `SnpV1` | stderr, once per such `T`, before any other output: ``hint: {hook_path} runs {owner}, not snp; run `{prog} install` to switch`` |
| `{prog} install` | §A.5.2 | M1, M1b, M2, M3 (stdout) |
| `hook-impl` | never | – |

The TTY condition keeps CI logs, differential tests and scripted callers byte-identical. Compat mode shows the hint
too: a user typing `pre-commit run` while git still runs Python is exactly the confusing case. The cost is up to 10
bounded file reads.

---

### A.15 `snp doctor [--fix]` (snp-only; hidden in compat help)

Read-only by default. Output goes to stdout as lines `"{level:<5} {subject}: {message}"` with `level` in
`ok|info|warn|error` (Tier B, not a stable API). Exit code is 1 if any `error`, else 0. `--fix` performs the §A.5.2
"doctor --fix" column for the current repo only, then re-runs the checks.

| # | Check | Level and message (shape) |
|---|---|---|
| 1 | snp itself | `info snp: 2.0.0 (pre-commit 4.6.2 compatible), {exe}, invoked as {prog}` |
| 2 | store | `info store: {path} (from {SNP_HOME\|PRE_COMMIT_HOME\|XDG_CACHE_HOME\|default})`; `warn` if not writable; `info` if snp 1.x leftovers are present (suggest `snp gc`) |
| 3 | pre-commit's store | `info pre-commit cache: {path} exists ({size}); snp does not use it`, plus the command to remove it. Size is computed only with `--sizes`. |
| 4 | `pre-commit` on PATH | resolution per §A.19.5, then `kind` per §A.9.2. Kind is decided by `same_file` with the snp exe; otherwise by running `{path} --version` with a 10 s timeout and matching `pre-commit X` / `pre-commit X (snp Y)`. A stale hard link gives `warn`. |
| 5 | `snp` on PATH | `warn` if the first `snp` on PATH is not this exe (e.g. the stale 1.x `~/.cargo/bin/snp`) |
| 6 | git | version; `warn` below the minimum chosen in doc 10 (Q8) |
| 7 | `core.hooksPath` | `git config --show-origin --show-scope core.hooksPath`. If set: `warn install refuses while core.hooksPath is set ({scope}: {value})`; if it ends in `.husky/_` or `.husky`, add `(husky)`. |
| 8 | per hook type (all 10) | Class (§A.5.1). For `PreCommit`, parse `INSTALL_PYTHON` and run `"{INSTALL_PYTHON}" -c "import importlib.util,sys; sys.exit(0 if importlib.util.find_spec('pre_commit.__main__') else 1)"` with cwd = toplevel, a 10 s timeout and the same env. This predicts Case A (`warn: runs Python pre-commit, not snp`), A′ (`error: commits fail: {py} cannot import pre_commit`), B (`warn: depends on the pre-commit found on PATH ({kind})`) or C (`error: commits fail: nothing named pre-commit on PATH`). For `Snp`: `ok`, or `warn` if `stale` or `SNP_EXE` is not executable. For foreign scripts: `info`. |
| 9 | per `.legacy` | manager script: `error: {path} is a {owner} hook script (double run or blocked commits)`; foreign: `info: migration mode` |
| 10 | config hooks | `git config --get-regexp '^hook\..*\.command$'` (git ≥ 2.54): list entries containing `hook-impl`; `error` if the same event also has a hooks-dir manager script |
| 11 | `init.templateDir` | classify `{dir}/hooks/*` as in check 8 (no Python probe) |
| 12 | config | `info` for `pre-commit …` entries that are not `hazmat` and need `pre-commit` on PATH; `warn` if `minimum_pre_commit_version` > 4.6.2 |
| 13 | Python compat collision | for each distinct `INSTALL_PYTHON`, also test `find_spec('pre_commit.main')`. If both `pre_commit.main` and the compat launcher's marker module `snp_pre_commit_compat` are importable: `error: snp-pre-commit-compat and pre-commit are both installed in {py}` |

The final line is `{n} problem(s): {e} error(s), {w} warning(s).`, with ``Run `snp doctor --fix` to repair this repository's hook scripts.`` appended when fixable items exist.

---

### A.16 User-facing migration checklist (for the README / docs site)

1. Install snp: `cargo install snp` (≥ 2.0.0), the release installer, `brew install snp`, or `pipx install snp-cli`.
   Check with `snp --version`, which prints `snp 2.0.0 (pre-commit 4.6.2 compatible)`.
2. In a repository, run `snp doctor`. It shows which tool git runs today for each hook.
3. Switch the repository:
   - `snp install --install-hooks` switches the default hook types and pre-builds environments, so the first commit is
     fast;
   - or `snp doctor --fix` switches **every** hook type pre-commit or prek had installed, keeping their options.
4. Template dir: `snp init-templatedir ~/.git-template`. git ≥ 2.54 config hooks: point `hook.pre-commit.<type>.command`
   at `snp hook-impl --hook-type <type> --`.
5. Scripts and tooling that call `pre-commit` (Makefile, tox, nox, IDE, docs):
   - replace them with `snp`; or
   - keep the name with `snp self alias install`; or
   - in Python environments, depend on `snp-pre-commit-compat` **instead of** `pre-commit`, never both.
6. CI:
   - replace `pre-commit/action` with `snp-action` (same `extra_args`) or the snp steps;
   - **change every cache key** (immutable caches would never save snp's store);
   - recipes that set `PRE_COMMIT_HOME` keep their cache path.
7. pre-commit.ci keeps working. Keep `.pre-commit-config.yaml` valid for Python pre-commit while anyone still uses it.
8. Optional: remove Python pre-commit (`pipx uninstall pre-commit` etc.) and its cache (`pre-commit clean` before
   uninstalling, or delete `~/.cache/pre-commit`).
   - Leftover pre-commit hook scripts then fall through to `pre-commit` on PATH (the alias), or fail with
     `` `pre-commit` not found. ``
   - With `pip --user`/system/project-venv installs they fail with `No module named pre_commit`.
   - Run step 3 everywhere first.
9. Rollback at any time: run `pre-commit install` with Python pre-commit in each repo, then `snp self alias uninstall`
   and `snp clean` (§A.13.1).

#### A.16.1 Message catalogue (new snp strings; all other strings follow §A.8.2)

| Id | Stream | Exact text |
|---|---|---|
| M1 | stdout | `Replaced hook script from {owner} at {hook_path}[ (Python: {INSTALL_PYTHON})][ (could not read its options; defaults used)]` |
| M1b | stdout | `Note: the replaced prek hook used prek-only options ({opts}); they are not supported and were dropped` |
| M2 | stdout | `Removed {legacy_path}: it is a hook script from {owner}, not a user hook` |
| M3 | stdout | ``Note: {hook_path} still runs {owner}; run `{prog} install -t {T}` to switch it to snp`` |
| M4 | stderr | ``hint: {hook_path} runs {owner}, not snp; run `{prog} install` to switch`` |
| M5 | stderr (hook script) | `` `snp` not found.  Did you forget to activate your virtualenv? `` (exit 1) |
| M6 | stderr | recursion guard, §A.7 (exit 1) |
| M7 | stdout (error handler) | ``An error has occurred: FatalError: refusing to use {dir} as the snp store: it is pre-commit's store.  Set SNP_HOME to a different directory.`` (exit 1; no crash log is written) |
| M8 | stdout (error handler) | `An error has occurred: FatalError: cannot create the snp store at {dir}: {io_error}.  Set SNP_HOME to a writable directory.` (exit 1) |
| M9 | stdout | `Removed snp 1.x cache data from {store}.` (gc) |
| M10 | stdout | `[INFO] snp store moved from {old} to {new}; hook environments will be reinstalled.` |
| M11 | stdout/stderr | alias messages, §A.9.2 |
| M12 | stdout | `Removed {legacy_path}: it is a hook script from snp, not a user hook` |
| V | stdout | `snp 2.0.0 (pre-commit 4.6.2 compatible)` / `pre-commit 4.6.2 (snp 2.0.0)` |

`{owner}` ∈ {`pre-commit`, `prek`, `snp 1.x`}. In M4 the owner is rendered as `Python pre-commit` for `PreCommit`.

---

### A.17 Platform differences (this topic)

| Area | Linux | macOS | Windows |
|---|---|---|---|
| snp hook script | `#!/bin/sh`, LF | same | same bytes. Git for Windows runs it with its bundled sh. `SNP_EXE='C:/…/snp.exe'`. pre-commit writes `#!/bin/sh` + a bash template with CRLF (derived). snp writes LF only. |
| Alias | symlink | symlink. Homebrew: `$(brew --prefix)/bin` may already hold brew's `pre-commit` (refusal → `--dir ~/.local/bin` or uninstall it) | `pre-commit.exe` hard link, or copy across volumes. Stale after rename-replace upgrades. |
| argv0 for compat | from `exec` | same | from the CreateProcess command line's first token. Compare case-insensitively and strip `.exe`. |
| `INSTALL_PYTHON` volatility | pipx/uv stable; venv | Homebrew Cellar path changes on every `brew upgrade` (→ Case B) | `'C:\…\python.exe'` |
| Store default | `~/.cache/snp` | `~/.cache/snp` (not `~/Library/Caches`, same as pre-commit) | `%USERPROFILE%\.cache\snp` |
| `.legacy` executable test | mode bits | mode bits | any existing file (03 §6.3) |
| CDPATH bug | affects pre-commit's template (dash and bash, OBS) | same (bash/zsh-as-sh) | Git Bash honours CDPATH too (derived) |

---

### A.18 Contradictions resolved by this addendum

| Topic | Conflicting statements | Resolution | Evidence |
|---|---|---|---|
| Store location / `PRE_COMMIT_HOME` | 03 §20.5 (own store, interplay open), 10 Q5 (honor `PRE_COMMIT_HOME` exactly), 06 §8 (only for clean/gc), 04a §14.10 (possibly shared), 07 Q2, 03 Q1 | `SNP_HOME` → `$PRE_COMMIT_HOME/snp` → `$XDG_CACHE_HOME/snp` → `~/.cache/snp`; never shared; refuse pre-commit's store; versioned buckets | `store.py:25-36`; WEB CI recipes; OBS-462 gc/clean with a nested `snp/`; OBS snp 1.x store |
| Lock interop | 04a §14.10 (also hold pre-commit's `.lock`), 07 D7 (overlapping regions) | Not needed: disjoint stores; snp locks only its own `.lock` | §A.10.2 |
| Crash log | 03 §20.5 item 13 (`snp.log`), 02 Q7 (shared `pre-commit.log`?), 10 Q9 (keep skeleton) | `<snp store>/snp.log` with pre-commit's skeleton and snp's key lines | `error_handler.py:32,47` |
| Install-state files | 03 §20.5 item 8 vs 04a §11.4 step 7 / §14.9 | `.snp-install-state.json` only | nothing reads pre-commit-format state in snp's store |
| Env-dir naming | 03 §20.5 item 9 vs 04a §14.2 | 03 (sanitized) wins; the store is private | follows from the store rule |
| hazmat failure exit code | 03 §15 ("exit 3") | entry path 1 (runtime) / 2 (usage, prog `hazmat.py`); direct CLI 3 / 2 | OBS-462 transcripts in §A.8.6 |
| Argparse error prefix | 02 §2.2 (`pre-commit run: error:`) vs 10 CLI-07 | Unrecognized args are reported by the **top-level** parser: top-level usage, then `{prog}: error: unrecognized arguments: --bogus`, exit 2; `{prog}` per brand | OBS-462 `pre-commit run --bogus` |
| Branding | 02 Q1, 03 Q6, 03 §20.3, 10 Q1 | argv0-based brand, B1/B2 rules | §A.8 |
| `--version` | 02 Q2, 03 Q6, 10 Q2 | §A.8.3 | `main.py:204-210`; OBS `python -m pre_commit --version` |
| Hook-script identity | 03 Q2, 10 Q3, 06 Q9 | own ID + embedded pre-commit and prek IDs | OBS-462 T6 |
| Script fallback | 03 Q3 | fall back to `pre-commit` on PATH | OBS-462 T2 |
| `pre-commit` in entries | 03 Q13, 10 Q4, 03 §20.8 | rewrite only `pre-commit hazmat` (to `current_exe _hazmat`) | `lang_base.py:174-178`; `.pre-commit-hooks.yaml:1-6` |
| PyPI name | 07 §3.17, 07 Q10 | `snp-cli` + `snp-pre-commit-compat` + PEP 541 request | WEB PyPI JSON |
| Executable lookup (for doctor/alias PATH resolution) | 04a §14.18 vs 06 §8 (`which`) | snp's own lookup emulating `command -v` (§A.19.5); not the `which` crate | the pre-commit template uses `command -v` |

The remaining reported contradictions do not affect the migration story and are **not** re-decided here: hook stdin,
YAML error-text parity, regex engine, async runtime, toolchain pin, `PRE_COMMIT_COLOR`, CPU count, fsmonitor flag,
stash restore, in-process emulation granularity and pre-push stdin parsing.

### A.19 Implementation notes for snp (Rust 1.98, edition 2024, `forbid(unsafe_code)`)

#### A.19.1 Brand

```rust
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Brand { Snp, PreCommit }
impl Brand {
    pub fn detect() -> Self;                       // §A.8.1; called once at the top of main(), before clap
    pub const fn prog(self) -> &'static str;       // "snp" | "pre-commit"
}
```

- Build the clap `Command` with `.name(brand.prog())` and `.bin_name(brand.prog())`, a custom `version` string (§A.8.3),
  and `.hide(brand == Brand::PreCommit)` on `doctor` and `self`.
- All user-facing strings go through `msg::*` functions taking `Brand`. A unit test asserts compat output for every
  catalog entry against REF strings.

#### A.19.2 Classifier, templates and parsers

- `fn classify(path: &Path) -> io::Result<Owner>`:
  - `symlink_metadata` for `Missing`/dir; `File::open` follows links (a dangling link gives `Foreign`);
  - `take(1 << 20).read_to_end`;
  - an `aho_corasick::AhoCorasick` built once in a `LazyLock` over all markers. Scan and pick the lowest class rank,
    not the first match position.
- Template rendering into a `Vec<u8>`, with `sh_quote(&OsStr) -> Vec<u8>`. Property test: for arbitrary byte strings
  without NUL, `sh -c 'printf %s "$1"' _ <rendered word>` round-trips (posix only).
- Era parsers: snp's `shlex` port for era 3; a ~60-line Python string-literal reader for eras 1-2 (no `eval`). All
  return `Option`, and `None` means defaults + the M1 note. Fuzz them (no panics).
- Atomic write: `tempfile::Builder::new().prefix(".snp-").tempfile_in(hooks_dir)`, write, set perms (03 §20.3), then
  `persist(hook_path)`. On Windows, retry `persist` on `PermissionDenied`/sharing violations (AV scanners) up to 5
  times with 50·2^n ms backoff (prek has `rename_with_retry`).

#### A.19.3 Stable `SNP_EXE` / alias target

```
cur = std::env::current_exe()?                            // Linux: /proc/self/exe (resolved)
cands = []
if brand == Snp and arg0 contains a path separator
   and absolute(arg0) has no `..` component:            cands.push(std::path::absolute(arg0))
for dir in PATH (skip empty and relative entries): cands.push(dir / "snp[.exe]")
cands.push(cur)
pick the first c with is_file(c) && same_file(c, cur)     // Unix: (st_dev, st_ino); Windows: same-file crate (volume serial + file index)
```

- Never canonicalize the pick. Homebrew's `/opt/homebrew/bin/snp` symlink must survive upgrades.
- An arg0 with a `..` component (`../../bin/snp install` run inside the repository) is skipped, not folded:
  `absolute` keeps the `..`, which ties `SNP_EXE` to the invoking directory (moving or renaming the repository then
  breaks it), and folding `..` lexically is wrong across symlinked directories. The PATH or `cur` candidate wins.
  Tests: `scripts::tests::select_exe_skips_a_dotted_arg0`, `install::snp_exe_never_embeds_dot_dot`.
- The script's `command -v snp` / `pre-commit` fallbacks cover versioned picks such as mise installs.

#### A.19.4 Is a file "snp"?

`same_file(file, cur)`; or, for a copy, the same length and the same BLAKE3 or SHA-256 as `cur`; or `file --version`
run with `SNP_VERSION_PROBE=1` (10 s timeout, own process group) matching `^pre-commit [0-9.]+ \(snp ([^)]+)\)$`,
which also reports the other snp's version. A plain `--version` cannot be used: the compat brand prints exactly
`pre-commit 4.6.2` (ADR-023); with the variable (and exactly that argv) snp of either brand prints
`pre-commit 4.6.2 (snp X)`, while Python pre-commit ignores it.

#### A.19.5 PATH lookup emulating `command -v`

- Split `PATH` on `:` (Unix) or `;` (Windows). An empty entry means cwd.
- Take the first `dir/name` that is a regular file (following links) and executable. On Unix use
  `rustix::fs::access(X_OK)`, which is safe.
- On Windows try `name.exe` (Git Bash semantics), then `PATHEXT` for the report.
- This is the same helper 04a §14.18 asks for. Do not use the `which` crate.

#### A.19.6 Store

- `StoreLocation::resolve(&EnvSnapshot, &invocation_cwd)` is a pure function over a captured env snapshot. Never read
  the process env deep inside the code, and never `set_var`.
- Returns `{ path, source }`. `realpath_lenient` walks up to the first existing ancestor, canonicalizes it, then
  re-appends the remaining components. `..` in the non-existing remainder is applied lexically, which is what
  `os.path.realpath` does. `std::path::absolute` alone keeps `..` components, so it is not enough.
- Pre-commit-store detection compares canonical paths and reads only the first README line, bounded to 256 bytes.

#### A.19.7 Alias

- `std::os::unix::fs::symlink`; `std::fs::hard_link`; `std::fs::copy` as the fallback.
- Creation is atomic: create `DIR/.pre-commit.snp-tmp-<pid>`, then `rename` over the target, which is allowed only
  after the refusal checks.
- Windows cannot replace a running `pre-commit.exe`. If `rename` fails with a sharing violation, report
  ``close running pre-commit processes and retry``.

#### A.19.8 Doctor process probes

- Use `std::process::Command` with `stdin(Stdio::null())` and captured output.
- Timeout: poll `try_wait()` every 20 ms up to 10 s, then `kill()`. std has no `wait_timeout`, and pulling tokio into
  doctor is not justified.
- Inherit the env so the probe mirrors the hook. Do not pass `-I` to the user's interpreter: the hook doesn't, and
  `PYTHONPATH` may matter.

#### A.19.9 `_hazmat`

- A hidden clap subcommand with its own parser named `hazmat.py`, mirroring `pre_commit/commands/hazmat.py:10-27`.
- Errors print the Tier B texts of §A.8.6 and return 1 or 2 directly, bypassing the crash-log error handler.
- The `hook_cmd` rewrite compares the first two **shlex tokens** exactly to `pre-commit`, `hazmat`, and substitutes
  `current_exe()` (absolute) and `_hazmat`.

#### A.19.10 Environment

| Variable | Read or set |
|---|---|
| `SNP_HOME`, `PRE_COMMIT_HOME`, `XDG_CACHE_HOME`, `HOME`/`USERPROFILE` | read (store) |
| `SNP_BIN` | read by the hook script only |
| `SNP_NO_HINTS` | read (§A.14) |
| `PRE_COMMIT_RUNNING_LEGACY`, `SNP_RUNNING_LEGACY` | read (guard), set for the legacy child (§A.7) |
| `PRE_COMMIT_ALLOW_NO_CONFIG` | read (03 §6.4); `SNP_ALLOW_NO_CONFIG` is an alias |

Nothing in the table is exported to hooks beyond pre-commit's set (`PRE_COMMIT=1` etc.). There is deliberately **no**
`SNP=1` marker for hooks.

#### A.19.11 Performance

None of this touches the hot path:
- `hook-impl` reads no extra files;
- the `run` hint costs up to 10 bounded reads, and only on a TTY;
- classification is a single SIMD multi-pattern scan over a file under 4 KiB.

### A.20 Tests to add (extends doc 10 §7.8; `interop_*` need the 4.6.2 oracle)

| Id | Scenario | Expected |
|---|---|---|
| MIG-01 | `pre-commit install` (oracle), then `snp install` | M1 with `(Python: …)`, then `snp installed at .git/hooks/pre-commit`; no `.legacy`; commit runs snp |
| MIG-02 | `snp install`, then oracle `pre-commit install` | `pre-commit installed at .git/hooks/pre-commit`; no `.legacy` (T6) |
| MIG-03 | `snp install`, then oracle `pre-commit uninstall` | `pre-commit uninstalled`; file removed (T6) |
| MIG-04 | prek script (fixture bytes from `prek/crates/prek/src/cli/install.rs:372-386`), then `snp install` | M1 (`prek`); no `.legacy` |
| MIG-05 | chain C1 (pre-commit over prek), then `snp install` | M1 + M2; one script left |
| MIG-06 | chain C2, then `snp install` | M1 (`prek`) + M2 (`pre-commit`) |
| MIG-07 | snp 1.x script (V1 template), then `snp install` | M1 (`snp 1.x`) |
| MIG-08 | Case B: era-3 pre-commit script with `INSTALL_PYTHON=/nonexistent`, alias on PATH | commit runs snp in compat mode; output byte-equal to the oracle run on the same repo |
| MIG-09 | Case C with snp on PATH but no alias | stderr `` `pre-commit` not found.  Did you forget to activate your virtualenv? ``, exit 1 |
| MIG-10 | era-1 fixture script (Python template from `9f0cfed600`), alias on PATH | `run --hook-stage commit` accepted; hooks run |
| MIG-11 | snp script with `CDPATH=.` exported, legacy hook present | legacy runs (T5) |
| MIG-12 | pre-commit era-3 script + `CDPATH=.` + legacy, alias → snp | snp repairs `--hook-dir`; legacy runs (deviation; the oracle skips it) |
| MIG-13 | snp script: `SNP_EXE` missing, oracle `pre-commit` on PATH | T2 |
| MIG-14 | snp script: nothing available | M5, exit 1 |
| MIG-15 | `PRE_COMMIT_HOME=/p`: snp run, then oracle gc and clean | snp store at `/p/snp`; oracle gc leaves it; oracle clean removes it; next snp run recreates it |
| MIG-16 | `SNP_HOME` = the oracle's store | M7, exit 1, nothing written |
| MIG-17 | store containing a snp 1.x layout (README signature + `db.db` + `resource-pools/`) | snp ignores it; `snp gc` prints M9; never creates reserved names |
| MIG-18 | `entry: pre-commit hazmat n1 nope --`, `… hazmat bogus`, `… hazmat cd sub echo` (no `--`) | `- exit code: 1` / `2` / `1` (§A.8.6) |
| MIG-19 | `pre-commit --version` via symlink; `snp --version` | §A.8.3 strings |
| MIG-20 | compat brand golden test over every catalog string | byte-equal to REF strings except E1-E6 |
| MIG-21 | `snp self alias install` with an existing foreign `pre-commit` in `DIR` | refusal, exit 1; with `--force` a `.snp-backup` is kept; uninstall restores it |
| MIG-22 | `snp run` with a pre-commit-owned hook, stderr a pty vs a pipe | hint only on the pty (`pty_*` gate) |
| MIG-23 | compat dist (Python, `net_*`): `python -m pre_commit --version`; Case A′ venv with the compat dist | `pre-commit 4.6.2 (snp …)`; the commit runs snp |
| MIG-24 | `doctor` on the fixtures above | levels and exit codes per §A.15 |

### A.21 Open questions

1. **Repository URL** `https://github.com/grok-rs/snp` is assumed: in the script header, the README, the bug-report
   URL in M6, and the crash-log hint. Confirm it before the first release, because the script header is permanent per
   ID.
2. **Embedding prek's ID** in snp's script, a courtesy that makes `prek install`/`uninstall` treat it as theirs. The
   recommendation is yes (no chains between prek and snp). Drop it if the prek maintainers object.
3. **`--version` in compat mode.** Keep `pre-commit 4.6.2 (snp X)`, which is diagnosable but breaks `$NF` and equality
   parsers, or print exactly `pre-commit 4.6.2` and rely on `snp doctor` / `snp self alias status` for diagnosis?
4. **The `pre-commit` fallback** in snp's hook script: switching to another tool when snp is missing ("fail over")
   versus failing closed. The recommendation is fail over, because the args are compatible and the user explicitly
   installed that pre-commit.
5. **PyPI**: confirm `snp-cli` and file the PEP 541 request for `snp`. Ship `snp-pre-commit-compat` from the first
   release (recommended) or later?
6. **`snp run` hint** limited to TTYs. Is an always-on variant behind `SNP_HINTS=always` wanted?
7. **Bucket names** (`repos-v1`, …) must be merged into 03 §20.5's layout by its owner. Should `store-v1.json`
   relocation detection (§A.10.4) be in 1.0?
8. **Windows alias staleness.** Hard links go stale on rename-replace upgrades. Alternative: ship a tiny launcher exe
   in release archives. Also verify on real Windows that Git Bash's `command -v pre-commit` finds `pre-commit.exe` and
   that CreateProcess argv0 parsing yields `pre-commit`.
9. **Should `snp clean --pre-commit`** (delete pre-commit's store and `~/.pre-commit`) exist? Currently no; doctor
   prints the command instead.
10. **`snp uninstall` and prek scripts**: skipped now. Should it remove them, symmetric with prek removing snp's?
11. **The doctor output format** is Tier B. Is a `--json` mode needed for tooling or IDE integration in 1.0?
12. **git 2.54 config hooks**: should `snp install` write `hook.pre-commit.<type>` entries (03 Q14)? It must then also
    remove hooks-dir manager scripts for the same events to avoid double runs.
13. **CDPATH repair** of pre-commit-generated `--hook-dir` values (§A.7) is a behavior deviation: legacy hooks run
    where pre-commit silently skips them. Confirm.
