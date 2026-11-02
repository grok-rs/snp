# D-SEC-07

With `PATH` unset or empty, snp's activation patches put no empty entry (the current directory) on `PATH`.

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-00 (snp-core `EnvPatch::prepend_path`); this file and
  `tests/differential/divergences/D-SEC-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.7.0 | snp |
|---|---|
| unset `PATH` ⇒ activation patches produce `"<bin>:"` (cwd on PATH) | no trailing separator |

## Rationale

Every environment language of pre-commit 4.7.0 activates its environment with an `envcontext` patch of the form
`('PATH', (bin_dir, os.pathsep, Var('PATH')))` (`languages/python.py:60`, `node.py:56`, `golang.py:73,82`,
`rust.py:53`, `ruby.py:65`, `conda.py:29`, `coursier.py:67`, `dart.py:27`, `dotnet.py:28`, `haskell.py:23`,
`lua.py:32`, `perl.py:23`, `swift.py:26`). `Var('PATH')` defaults to `''`, so when `PATH` is unset or empty the hook
runs with `PATH=<bin>:` (`<bin>;` on Windows). An empty `PATH` entry means "the current directory" to `execvp`, to
shells and to `parse_shebang.find_executable`; hooks run in the repository root, so any executable file committed to
the repository whose name matches a command the hook runs (`git`, `sh`, `python3`, …) would be executed. That turns
an untrusted repository into code execution for a user whose `PATH` happens to be empty.

snp builds these patches with `snp_core::env::EnvPatch::prepend_path(key, dirs)`, which renders
`d1 PATHSEP … dn PATHSEP $key` only when `$key` is set and non-empty, and `d1 PATHSEP … dn` otherwise. When `PATH` is
set and non-empty the result is byte-identical to pre-commit's, so no hook output changes; the divergence is only
the missing current-directory entry.

## Examples

Patch `('PATH', ('/env/bin', os.pathsep, Var('PATH')))` applied to three environments (POSIX):

| startup `PATH` | pre-commit 4.7.0 hook `PATH` | snp hook `PATH` |
|---|---|---|
| unset | `/env/bin:` | `/env/bin` |
| `` (empty) | `/env/bin:` | `/env/bin` |
| `/usr/bin` | `/env/bin:/usr/bin` | `/env/bin:/usr/bin` |

End to end the difference cannot be observed through the CLI: with `PATH` unset or empty both tools fail before any
hook runs, because neither can find `git` (`find_executable` searches `PATH`, and git sets `PATH` itself before it
runs a hook script). The divergence protects library callers and every activation computed from an `Env` without
`PATH` (snp-lang's `ToolProbe`/`CmdRunner` paths, tests, embedders). It is pinned by the unit test
`prepend_path_without_trailing_separator` in `crates/snp-core/src/env.rs`; the scenario file records why no
differential scenario exists.
