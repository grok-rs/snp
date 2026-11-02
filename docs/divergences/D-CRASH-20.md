# D-CRASH-20

shim-less system env

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-20.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-24 (`crates/snp-lang/src/langs/node.rs`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| Windows node hook with explicit `language_version: system` → exit 3 | shim-less system env |

## Rationale

pre-commit never picks `system` for node on Windows by itself (`get_default_version` returns `default` on `win32`,
`node.py:25-35`), but a configuration may ask for it (`language_version: system`, or `default_language_version:
{node: system}` shared with Linux and macOS colleagues). pre-commit then runs `python -mnodeenv --prebuilt
--clean-src <env> -n system`, and nodeenv refuses: it logs `Installing system node.js on win32 is not supported!` and
exits 1, so the install ends with `An unexpected error has occurred: CalledProcessError: …` and exit 3 (04a §3.4).

nodeenv's system mode does nothing a hook needs on Windows anyway: on POSIX it writes a bash shim `bin/node` that sets
`NODE_PATH`/`npm_config_prefix` and `exec`s the system node, values that pre-commit's own activation sets for the hook
as well (`get_env_patch`, `node.py:38-57`). snp reimplements nodeenv without the shim on every platform: a `system`
environment is the directory layout (`<env>\Scripts\node_modules` on Windows, `<env>/bin` and
`<env>/lib/node_modules` elsewhere), the system `node`/`npm` are found on `PATH`, and `npm install
--allow-git=root --install-links -g <pkgs>` installs into the environment through `npm_config_prefix` (04a §14.8,
decision 04a Q14). The default stays `default` on Windows, so only configurations that explicitly ask for the system
node behave differently — and they get what they asked for instead of a crash.

## Examples

A local node hook with `language_version: system` and a dependency given as an absolute path to a package in the
repository (no network), run on Windows (`tests/differential/divergences/D-CRASH-20.yaml`):

pre-commit 4.7.0, exit 3 (the install lines, then nodeenv's failure as a `CalledProcessError`):

```
[INFO] Initializing environment for local:<work>\pkg.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: CalledProcessError: command: ('<pre-commit's python>', '-mnodeenv', '--prebuilt', '--clean-src', '\\\\?\\<store>\\repo…\\node_env-system', '-n', 'system')
return code: 1
…
Check the log at <store>\pre-commit.log
```

snp, exit 0:

```
[INFO] Initializing environment for local:<work>\pkg.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
hello....................................................................Passed
- hook id: hello
- duration: 0.05s

hello from node
```
