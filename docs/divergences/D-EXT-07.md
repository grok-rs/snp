# D-EXT-07

snp-brand commands and flags (§8), including their presence in the snp brand's usage/help blocks

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-12; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-12 (`snp_cli::SNP_SPEC`, the typed `Cli` facade); the commands behind the flags are
  implemented by WP-32/WP-33/WP-34.

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| snp-brand commands and flags (§8), including their presence in the snp brand's usage/help blocks |

## Rationale

The snp brand adds the extensions of COMPATIBILITY §8.1 / ARCHITECTURE §9.3 to pre-commit's parser tree:

| Parser | Additions |
|---|---|
| top level | commands `cache`, `completions`, `config`, `doctor`, `list`, `migrate`, `self` (inserted alphabetically; `help` and `hook-impl` stay last) |
| `run`, `try-repo` | several hook ids (`hook` is `nargs='*'`; `:` allowed), `-j/--jobs`, `--group`, `--require-group`, `--no-group`, `--no-fast-path`, `--stdin-files[=nul\|lines]` (a third member of the `--all-files \| --files` group), `--summary`, `--no-local-config` |
| `install`, `uninstall` | `--git-config-hooks` (ADR-057) |
| `install-hooks` | `--with-fallback` |
| `validate-config`, `validate-manifest` | `--strict`, `--portable` |

They change the usage and help blocks of those parsers (new options, a longer `{…}` command list), the
`invalid choice` list of the top-level parser and of course what is accepted.

Why it is safe:

* **Only the snp brand has them.** Under the `pre-commit` name the parser tree is exactly pre-commit's
  (`PRE_COMMIT_SPEC`): every extension is rejected with pre-commit's own texts (`invalid choice: 'list' (choose from
  …)`, `unrecognized arguments: --summary`), byte-identical to pre-commit 4.7.0 (`compat.json` goldens).
* **No existing command line changes meaning.** `snp_argparse::prefix_stable(PRE_COMMIT_SPEC, SNP_SPEC)` checks that
  every long-option abbreviation pre-commit resolves uniquely resolves to the same option in the snp brand
  (`run --all`, `run --fi`, `install --conf x.yaml`, …); the planted names `--files-from-stdin` and `--config-hooks` fail
  it (ADR-057, ARCHITECTURE §9.2). The extensions only turn pre-commit's usage errors into accepted command lines:
  `run a b` (two hook ids) instead of `unrecognized arguments: b`.
* **The texts are argparse's.** The snp-brand usage/help/error blocks of the parsers that carry extensions are
  rendered by `snp_argparse` and golden-tested against a real CPython 3.13.14 argparse built from
  `export_json(SNP_SPEC, "snp")` (ADR-058; `crates/snp-cli/tests/goldens/snp.json`).

argparse semantics apply to the extensions too: `--stdin-files` takes an optional value, so a hook id must not follow
it directly (`snp run --stdin-files hook` → `invalid choice: 'hook'`); write `snp run hook --stdin-files` or
`--stdin-files=lines hook`. Unknown options are still deferred to the top level, and hook ids interleaved with
options follow argparse's positional rules (`snp run a -a b` → `unrecognized arguments: b`).

## Examples

```text
$ pre-commit list
usage: pre-commit [-h] [-V]
                  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...
pre-commit: error: argument command: invalid choice: 'list' (choose from 'autoupdate', 'clean', 'gc', 'hazmat', 'init-templatedir', 'install', 'install-hooks', 'migrate-config', 'run', 'sample-config', 'try-repo', 'uninstall', 'validate-config', 'validate-manifest', 'help', 'hook-impl')
$ snp list            # accepted: Cli::List
```

```text
$ pre-commit run --bogus
usage: pre-commit [-h] [-V]
                  {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...
pre-commit: error: unrecognized arguments: --bogus
$ snp run --bogus
usage: snp [-h] [-V]
           {autoupdate,cache,clean,completions,config,doctor,gc,hazmat,init-templatedir,install,install-hooks,list,migrate,migrate-config,run,sample-config,self,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...
snp: error: unrecognized arguments: --bogus
```

`snp run -h` (usage block; pre-commit's has no `--stdin-files`, `-j` … `--no-local-config`, and `[hook]` instead of
`[hook ...]`):

```text
usage: snp run [-h] [--color {auto,always,never}] [-c CONFIG] [--verbose]
               [--all-files | --files [FILES ...] |
               --stdin-files [{nul,lines}]] [--show-diff-on-failure]
               [--fail-fast]
               …
               [--rewrite-command REWRITE_COMMAND] [-j JOBS] [--group GROUP]
               [--require-group GROUP] [--no-group GROUP] [--no-fast-path]
               [--summary] [--no-local-config]
               [hook ...]
```

The scenario `tests/differential/divergences/D-EXT-07.yaml` runs `snp -h`, `snp run -h` and `snp run a b` against the
oracle, where they must keep differing.
