# D-SEC-01

sanitized-or-hashed env dir names; deletion contained in the repo's envs/

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| `language_version: ../../x` (or an absolute path) makes the install step delete arbitrary directories; Windows `C:` versions break | sanitized-or-hashed env dir names; deletion contained in the repo's `envs/` |

## Rationale

pre-commit names an environment `os.path.normpath(os.path.join(prefix, f'{ENVIRONMENT_DIR}-{language_version}'))`
(`lang_base.py:90-91`). `language_version` comes from the config: `..` components escape the store entry, and the
install step removes the directory it computed — first `rmtree(venv)` when it exists (a "partial previous install"),
then `clean_path_on_failure` when the install fails (`repository.py:72-89`). A config in a cloned repository can thus
delete any directory the user can write (04b §5.1, verified); an absolute version nests directories, and a Windows
`C:` version breaks the path.

snp's `Store::env_dir` (ADR-009) uses `<ENV_DIR>-<version>` only when the version matches `^[A-Za-z0-9._+-]{1,64}$`,
is not `.`/`..` and cannot alias another name on the filesystem (no trailing `.`, which Win32 strips, so `3.12.` would
open `3.12`; on Windows and macOS, whose default filesystems fold case, no upper-case ASCII, so `3.12A` and `3.12a` never
share an environment); anything else becomes `<ENV_DIR>-h<hex16(blake3(version))>`. The name is always one component
directly inside `<repo>/envs`, and environment removal goes through `remove_tree_within(env_dir, <repo>/envs)`
(fd-relative on Unix, never following a symlink). Every real version string (`python3.12`, `3.12.1`, `system`,
`default`, `22.11.0`) keeps its readable name.

Safe: the store is private to snp (FD-2), so the directory name is not observable except in error texts that quote it;
the install itself receives the original `language_version` and fails or succeeds exactly as before.

## Examples

Recorded with pre-commit 4.6.2 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc462.txt`); `<W>` is the scratch root, `<store>` the store directory.

A local `language: python` hook with `language_version: ../../../../sec01-victim`, where `<W>/sec01-victim/keep.txt`
exists next to the store (`<W>/store`). pre-commit 4.6.2, `run --all-files`, exit 3:

```text
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: CalledProcessError: command: ('<venv>/bin/python', '-mvirtualenv', '<W>/sec01-victim', '-p', '../../../../sec01-victim')
return code: 1
stdout:
    RuntimeError: failed to find interpreter for Builtin discover of python_spec='../../../../sec01-victim'
stderr: (none)
Check the log at <W>/store/pre-commit.log
```

and `<W>/sec01-victim` (with `keep.txt`) **has been deleted**. snp computes `<store>/repos-v1/<key>/envs/py_env-h<16
hex>`; the install fails on the unusable interpreter request the same way (exit 3), and nothing outside `envs/` is
touched. The scenario (`tests/differential/divergences/D-SEC-01.yaml`) climbs to `/` and back down into the worktree,
so the oracle deletes a committed directory and the tree comparison shows the difference.

Unit coverage: `crates/snp-store/tests/support/behaviour.rs::env_dir_names_are_sanitized_or_hashed` and the
`store_env_dir_name` fuzz target.
