# D-CRASH-13

decoded lossily

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-16 (contributing: WP-24, WP-25); the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-16 (framework rule and scenario); the language backends of WP-24/WP-25 apply the rule to
  their probes.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| non-UTF-8 output of tool probes (`node --version`, …) → exit 3 | decoded lossily |

## Rationale

pre-commit reads the output of several tool probes with `util.cmd_output` (`util.py:116-120`), which decodes stdout
and stderr as **strict** UTF-8: `lua._get_lua_version` (`luarocks config --lua-ver`), python's `_version_info` and
`_find_by_py_launcher`, r's `_rscript_exec` probes, the `cygpath` calls of node and golang. A tool that prints a single
byte that is not UTF-8 (a localized message in a legacy code page, a corrupted install, a wrapper script that echoes a
Latin-1 path) makes pre-commit die with `An unexpected error has occurred: UnicodeDecodeError: …` and exit 3 —
before any hook ran, and on every subsequent run, because the probe repeats while the environment is (re)installed.
The traceback says nothing about which tool produced the bytes.

snp decodes probe output with `String::from_utf8_lossy` (invalid sequences become U+FFFD) and continues:

* the framework never decodes command output it reports: `LangError::CalledProcess` keeps stdout and stderr as raw
  bytes and the failure envelope prints format A byte for byte (`snp_lang::common::setup_cmd`, catalog row
  T10-XRG-21, `crates/snp-lang/tests/framework/catalog.rs`);
* every backend that needs a probe's text (the lua version, the python `version_info`, the R version, …) decodes it
  lossily (WP-24/WP-25 own those call sites; this file is the rule they cite).

The deviation is safe: it only replaces a crash (exit 3, nothing ran) by the behaviour pre-commit has for every valid
input. A replacement character can only reach an environment-variable value or a directory name that is derived from
the probe (`LUA_PATH`'s `share/lua/<version>`), exactly where the undecodable byte would have gone had Python used
`surrogateescape`; nothing is ever passed to a shell. It never turns a failing outcome into success unless the hooks
actually run and pass (COMPATIBILITY K5).

## Examples

A local `language: lua` hook whose `luarocks` prints `5.4\xff` for `luarocks config --lua-ver`
(`tests/differential/divergences/D-CRASH-13.yaml`; recorded with `target/oracle/pc470-py312`):

pre-commit 4.7.0, exit 3:

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
An unexpected error has occurred: UnicodeDecodeError: 'utf-8' codec can't decode byte 0xff in position 3: invalid start byte
Check the log at <store>/pre-commit.log
```

snp, exit 0 (the environment is installed with Lua version `5.4\u{fffd}` and the hook runs):

```
[INFO] Initializing environment for local.
[INFO] Installing environment for local.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
lua hook.................................................................Passed
```

The same applies to a failing setup command whose stderr is not UTF-8 (T10-XRG-21): pre-commit and snp both print
format A (`command: (…)` / `return code: 1` / `stdout: (none)` / `stderr:` + the raw bytes, indented) and exit 3 —
there the bytes are reproduced, not decoded.
