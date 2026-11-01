# D-DIAG-03

run hint, only when stderr is a TTY, SNP_NO_HINTS unset/empty and core.hooksPath unset: hint: {hook_path} runs {owner}, not snp; run {prog} install to switch…

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::pipeline::run_hint`).

## Register entry (COMPATIBILITY §5.8)

| Diagnostic (stderr only) |
|---|
| `run` hint, only when stderr is a TTY, `SNP_NO_HINTS` unset/empty and `core.hooksPath` unset: ``hint: {hook_path} runs {owner}, not snp; run `{prog} install` to switch`` (11 §A.14) |

## Rationale

The most confusing state of a migration is a repository whose git hooks still run Python pre-commit (or prek, or
snp 1.x) while the user types `snp run` (or `pre-commit run` through the snp alias): the manual run uses snp, the
commit uses the old tool, and the two can disagree. When `snp run` is started directly (not through `hook-impl`), it
classifies `<common-dir>/hooks/<T>` for each `T` in the config's `default_install_hook_types` (bounded reads, 11
§A.5.1) and, for a script owned by pre-commit, prek or snp 1.x, prints one line per type on stderr before any other
output: ``hint: .git/hooks/pre-commit runs Python pre-commit, not snp; run `snp install` to switch``.

It is printed only when stderr is a terminal, `SNP_NO_HINTS` is unset or empty and `core.hooksPath` is unset: CI logs,
the differential harness, scripts and pipes stay byte-identical with pre-commit, and stdout never changes.

## Examples

```text
$ pre-commit install            # Python pre-commit's script
$ snp run                       # on a terminal
hint: .git/hooks/pre-commit runs Python pre-commit, not snp; run `snp install` to switch
passing..................................................................Passed
$ snp run 2>&1 | cat            # not a terminal: no hint
passing..................................................................Passed
```

Proof: the scenario `tests/differential/divergences/D-DIAG-03.yaml` (a pty step); the conditions are in
`snp_run::pipeline::run_hint`.
