# D-CRASH-22

`hazmat` with no tool is an argparse usage error, exit 2.

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-26 (`snp_hooks::hazmat`; the usage error itself is snp-cli's parse, WP-12); the first package of the
  register row owns this file and `tests/differential/divergences/D-CRASH-22.yaml` (WBS "Divergence files",
  ADR-071).
* **Catalog:** 12 §A.10 (path C, "no tool"), 03 Addendum §H.6 E1; tests `cli_path_exit_codes`
  (`crates/snp-hooks/tests/hazmat/main.rs`) and snp-cli's hazmat parse tests.

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| `pre-commit hazmat` with no tool → `AttributeError` exit 3 | argparse usage error, exit 2 |

## Rationale

`hazmat.add_parsers` (`pre_commit/commands/hazmat.py:10-28`) creates its sub-parsers with
`add_subparsers(dest='tool')`, which is not `required`. A bare `pre-commit hazmat` therefore parses to
`Namespace(tool=None, color=…)` and reaches `hazmat.impl(args)` inside `main.py`'s `error_handler`; the first line of
`impl` is `args.cmd = tuple(args.cmd)`, which raises `AttributeError: 'Namespace' object has no attribute 'cmd'`. The
error handler turns that into `An unexpected error has occurred: AttributeError: …`, writes a crash log into the store
(creating the store first) and exits 3 — the generic "pre-commit has a bug" exit code, for what is plainly a usage
mistake.

snp's CLI table marks the tool as missing and reports it the way argparse reports every other missing argument of the
same command: the `pre-commit hazmat` usage block on stderr, `pre-commit hazmat: error: the following arguments are
required: tool`, exit 2. No crash log, no store. The behaviour of every invocation that names a tool is unchanged
(`snp_hooks::hazmat::cli` runs exactly `hazmat.py`'s tools), and nothing pre-commit accepts is rejected: the only input
affected is the one pre-commit crashes on. K4 (a): a crash becomes a precise diagnostic with the conventional usage
exit code.

The in-hook path is **not** affected: `entry: pre-commit hazmat` with nothing after it is rewritten to
`snp __hazmat`, whose `in_hook_main` reproduces Python's uncaught exception exactly as `python -m
pre_commit.commands.hazmat` shows it (final traceback line `AttributeError: 'Namespace' object has no attribute
'cmd'`, exit 1; 03 Addendum §H.5 D1).

## Examples

`pre-commit hazmat` (any directory, fresh `PRE_COMMIT_HOME`):

pre-commit 4.7.0 (stdout, exit 3; `README`, `db.db`, `.lock` and `pre-commit.log` created in the store):

```
An unexpected error has occurred: AttributeError: 'Namespace' object has no attribute 'cmd'
Check the log at <store>/pre-commit.log
```

snp (stderr, exit 2, the store untouched):

```
usage: pre-commit hazmat [-h] [--color {auto,always,never}]
                         {cd,ignore-exit-code,n1} ...
pre-commit hazmat: error: the following arguments are required: tool
```

The scenario in `tests/differential/divergences/D-CRASH-22.yaml` runs exactly this invocation.
