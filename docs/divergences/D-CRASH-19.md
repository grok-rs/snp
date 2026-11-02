# D-CRASH-19

uv-managed download into the store (`SNP_PYTHON_DOWNLOADS=never` restores the failure)

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-19.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-24 (`crates/snp-lang/src/langs/python.rs`, `uv_find_python`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| explicitly requested Python version not installed → install crash exit 3 | uv-managed download into the store (`SNP_PYTHON_DOWNLOADS=never` restores the failure) |

## Rationale

A hook (or `default_language_version`) that names an interpreter, e.g. `language_version: python3.11`, cannot run on
a machine without that interpreter: pre-commit passes `-p python3.11` to virtualenv, whose discovery fails with
`RuntimeError: failed to find interpreter for Builtin discover of python_spec='python3.11'`, and pre-commit ends with
`An unexpected error has occurred: CalledProcessError: …` and exit 3 — on every run, until the user installs that
Python by hand (04a §2.2, §11.4 step 2).

snp resolves the interpreter itself (the reference interpreter when the version matches it, the `py` launcher on
Windows, `PATH`, `expanduser(version)`), then asks uv (`uv python find --no-project --no-config
--no-python-downloads <request>`, `pythonX.Y` → `X.Y`, `pypyX.Y` → `pypy@X.Y`). Only when that fails too, and only
for an **explicitly requested** version (never for `default`), it installs a uv-managed CPython (python-build-standalone,
verified by uv) into the store's shared tools (`uv python install --no-config --no-bin --install-dir
tools-v1/python/uv <request>`, under the tool lock; `--no-bin` keeps it out of `~/.local/bin`), builds the
environment with it and reports `("python", "uv")` in the install outcome so the store keeps it. Decision 04a Q2 /
ADR-033.

The deviation turns a crash that nothing but a manual installation fixes into a working environment for exactly the
interpreter the configuration asks for; it never changes which interpreter is used when the requested one exists, and
it never applies to `default`. Strict environments (no network, curated interpreters) set `SNP_PYTHON_DOWNLOADS=never`:
the install then fails with the `CalledProcessError` of `uv python find` and exit 3, as pre-commit does. The pip
installer (`SNP_PYTHON_INSTALLER=pip`) has no discovery and fails like pre-commit
(`ValueError: failed to find interpreter for python_spec='python3.11'`, exit 3).

## Examples

A local hook pinned to `language_version: python3.11` on a host that has only 3.12 and 3.13
(`tests/differential/divergences/D-CRASH-19.yaml`). Note that the virtualenv pinned with pre-commit 4.7.0 (21.x)
also discovers uv-managed interpreters in uv's default directory (verified: with a uv-managed 3.11.13 present it
builds the environment from it), and so does snp's `uv python find`; the scenario needs a host where neither `PATH`
nor uv's directory has 3.11.

pre-commit 4.7.0, exit 3 — the install lines, then the `CalledProcessError` of the virtualenv command, whose output
carries virtualenv's `RuntimeError: failed to find interpreter for Builtin discover of python_spec='python3.11'`
(04a §2.2 observed the same text for `python_spec='system'`):

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: CalledProcessError: command: ('<pre-commit's python>', '-mvirtualenv', '<store>/repo…/py_env-python3.11', '-p', 'python3.11')
return code: 1
…
Check the log at <store>/pre-commit.log
```

snp, exit 0:

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
py311....................................................................Passed
- hook id: py311
- duration: 0.02s

3.11
```

With `SNP_PYTHON_DOWNLOADS=never` snp prints the four install lines, `An unexpected error has occurred:
CalledProcessError: command: ('<uv>', 'python', 'find', '--no-project', '--no-config', '--no-python-downloads',
'3.11')`, uv's `No interpreter found` text and exits 3.
