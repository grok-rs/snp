# D-PERF-01

uv (`--seed`) with pip-config translation and a pip fallback; seeded packages may differ in version, and uv's resolver
can differ on pre-releases, yanked versions and extra-index strategy (FD-7); `SNP_PYTHON_INSTALLER=pip` restores pip's
resolver per env (ADR-073)

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-24 (`crates/snp-lang/src/langs/python.rs`, `python/{uv,pipconf}.rs`).

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.7.0 | snp |
|---|---|
| Python envs built with virtualenv + pip | uv (`--seed`) with pip-config translation and a pip fallback; seeded packages may differ in version, and uv's resolver can differ on pre-releases, yanked versions and extra-index strategy (FD-7); `SNP_PYTHON_INSTALLER=pip` restores pip's resolver per env (ADR-073) |

## Rationale

pre-commit builds every `language: python` environment with `<its interpreter> -mvirtualenv <env> [-p X]` (cwd `/`)
and `python -mpip install . *deps` in the hook clone (`python.py:214-228`). That costs ≈ 0.25–0.4 s per venv and
seconds per install (04a §2.3, §11.1), and it needs a Python with virtualenv — which snp, a single binary, does not
have. snp (ADR-033, ADR-073, ARCHITECTURE §6.2) builds the environment with uv by default:

```
uv venv <env> --python <abs interp> --seed --no-project --no-config --no-python-downloads        (cwd /)
uv pip install --python <env>/bin/python --no-config [translated pip config] [.] *deps          (cwd = the clone)
```

* **Same interpreter choice.** `<abs interp>` is chosen by pre-commit's rules relative to a reference interpreter
  (`python3`, then `python`; Windows `py -3`, `python`): `default` and versions matching it use it, then the `py`
  launcher (Windows), `PATH`, `expanduser(version)`, and only then uv's discovery; env directory names are pre-commit's
  (`py_env-python3.12`).
* **Same environment shape.** `--seed` installs pip (and, below 3.12, setuptools and wheel), as virtualenv does, so
  hooks that shell out to `pip` keep working; the activation (`PIP_DISABLE_PIP_VERSION_CHECK`, `PYTHONHOME` unset,
  `VIRTUAL_ENV`, `PATH`) is pre-commit's byte for byte (recorded goldens in `crates/snp-lang/testdata/group1/golden`).
* **Same configuration.** uv ignores `pip.conf` and `PIP_*`; snp reads them the way pip layers them and translates
  every setting that has an equivalent (index URLs with `--index-strategy unsafe-best-match`, find-links, trusted
  hosts, certificates, proxy, pre-releases, constraints, the inverted `no-build-isolation`, binary policies, timeout,
  keyring, hashes). `--no-config` keeps the hook repository's `uv.toml`/`[tool.uv]` from redirecting indexes. The
  `UV_*` variables that would change the interpreter, project or resolution are stripped; network, cache and TLS ones
  are kept; the uv cache is isolated (`cache-v1/uv`) unless `UV_CACHE_DIR` is set.
* **Fallback.** A pip setting without a translation, an `additional_dependencies` item that is a pip option uv rejects
  (`--user`, `--progress-bar=…`, `--use-pep517`, …), or no obtainable uv (no `$SNP_UV`, none next to snp, none ≥ 0.8.0
  on `PATH`, the pinned release cannot be downloaded) builds that environment with pip instead
  (`<interp> -m venv <env>` + pre-commit's `python -mpip install . *deps`). `SNP_PYTHON_INSTALLER=pip` forces pip for
  every environment, `=uv` forces uv and fails instead of falling back; the choice is recorded in the install state,
  and a different choice rebuilds the environment.

What remains different is what uv and pip themselves do differently: the seeded pip/setuptools versions (uv's
bundled seed versus virtualenv's embedded wheels), `pyvenv.cfg` (`uv = …`/`version_info = 3.12.3` versus
`virtualenv = …`/`version_info = 3.12.3.final.0`; snp's health check accepts both, 04a G5), and uv's resolver, which
can pick a different version than pip for pre-releases, yanked releases or multiple indexes (FD-7). Users who depend
on pip's resolution set `SNP_PYTHON_INSTALLER=pip`.

## Examples

A local python hook with a dependency that prints which tool created its environment
(`tests/differential/divergences/D-PERF-01.yaml`; needs the network for `six`):

pre-commit 4.7.0, exit 0:

```
[INFO] Initializing environment for local:six.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
creator..................................................................Passed
- hook id: creator
- duration: 0.02s

virtualenv
```

snp, exit 0:

```
[INFO] Initializing environment for local:six.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
creator..................................................................Passed
- hook id: creator
- duration: 0.02s

uv
```

With `SNP_PYTHON_INSTALLER=pip`, snp prints `venv` (the standard library's creator writes neither key).
