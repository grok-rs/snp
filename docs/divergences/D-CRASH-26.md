# D-CRASH-26

`ls-files -z` + adjacent dedup (byte-identical list); the real flag is kept on ≥ 2.31 for argv parity in error texts

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-26.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-18 (`snp_git::Git::all_files`, `crates/snp-git/src/git.rs`; ADR-063).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.7.0 | snp |
|---|---|
| git < 2.31: `run --all-files`, the pre-push root push and the meta hooks call `ls-files -z --deduplicate` → `CalledProcessError` (rc 129) exit 3 (a traceback inside the meta hook's output) (02 G.3) | `ls-files -z` + adjacent dedup (byte-identical list); the real flag is kept on ≥ 2.31 for argv parity in error texts |

## Rationale

`git.get_all_files` (`git.py:154-155`) has used `git ls-files -z --deduplicate` since pre-commit 4.6.1;
`--deduplicate` exists from git 2.31.0. pre-commit 4.7.0 has no version check, so on git 2.19–2.30 (Debian 11 ships
2.30.2, Ubuntu 20.04 2.25.1, older macOS Xcode 2.30–2.32) every `--all-files` run, every pre-push of a new history
and the `check-hooks-apply`/`check-useless-excludes` meta hooks die with git's usage error — while the default staged
run works (02 G.3). ADR-063 keeps every such git usable: snp never refuses an old git, it falls back.

`ls-files` in cached mode prints index entries sorted by path and then stage; `--deduplicate` only skips an entry
whose name equals the previously printed one (`builtin/ls-files.c`). So `git ls-files -z` with every record equal to
the previous record dropped is byte-identical to `--deduplicate` on any git. snp uses it only when `GitCaps::
ls_files_dedup` is false (version < 2.31); on ≥ 2.31 it keeps the real flag so that a failing call prints the same
`CalledProcessError` argv as pre-commit. The fallback changes no output on any git where pre-commit works.

## Examples

`pre-commit run show --all-files` on git 2.30.9 (02 G.3; 2.18.5 differs only in the usage wording), exit **3**:

```
An unexpected error has occurred: CalledProcessError: command: ('/opt/git-2.30.9/bin/git', 'ls-files', '-z', '--deduplicate')
return code: 129
stdout: (none)
stderr:
    error: unknown option `deduplicate'
    usage: git ls-files [<options>] [<file>...]
    
        -z                    paths are separated with NUL character
        …(the full option list, 4-space indented)…
        --debug               show debugging data
Check the log at <store>/pre-commit.log
```

snp on the same git: stderr carries the one D-DIAG-01 line (`[WARNING] git 2.30.9 is older than 2.31.0, …`), the hook
runs on every tracked file once (unmerged paths appear once, as with `--deduplicate`), stdout is what pre-commit
prints on git ≥ 2.31, exit as the hooks decide.

Equality proof: `crates/snp-git/tests/behaviour/user_repo.rs` `dedup_fallback_equals_ls_files_deduplicate` compares,
in a merge conflict (`conflict_file` listed once per stage by plain `ls-files -z`), the fallback of a 2.30-capability
`GitExe` with git's own `ls-files -z --deduplicate` byte for byte; the fuzz target `git_z_split` checks the fallback
against Python-style adjacent dedup on arbitrary input. The scenario below runs on the CI git 2.30 container.
