# D-CRASH-06

per-repo [{repo}] <error> lines, the others are updated and written, exit 1

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-33; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-33 (`snp_cmd::autoupdate::autoupdate`; tests `autoupdate::x13_git_02_one_fetch_fails`,
  `errors::x13_cmd_06_autoupdate_git_failure_is_a_repo_line`).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| `autoupdate`: one unreachable repo → `CalledProcessError` exit 3, nothing written (X13-GIT-02) | per-repo `[{repo}] <error>` lines, the others are updated and written, exit 1 |

## Rationale

pre-commit's `autoupdate` (`commands/autoupdate.py:183-214`) runs `_update_repo` for each repository in turn and
catches only `RepositoryCannotBeUpdatedError`. A git command that fails inside `RevInfo.update`
(`git fetch origin HEAD --tags` against an unreachable or deleted remote) raises `CalledProcessError`, which escapes
the loop: the error handler prints the `An unexpected error has occurred: CalledProcessError: …` block, writes the
crash log and exits 3. Repositories already resolved are lost — nothing is written, although their new revs were
printed as `updating … -> …` lines just before (X13-GIT-02).

snp treats a failing git command of one repository like pre-commit's own per-repository failure: the repository's
line is `[{repo}] ` followed by the same `CalledProcessError` block pre-commit would have printed (command,
`return code:`, `stdout:`, `stderr:`), the return value becomes 1, and every other repository is still resolved,
printed and written (rev lines rewritten in place, as for a successful run). With `-j N` the per-repository lines
appear in completion order, as they do for pre-commit's own failures.

Why it is safe: the stdout lines of every successful repository and the file bytes they produce are the ones
pre-commit writes when no repository fails; the failing repository is reported with the same text pre-commit
shows, and the run still fails (exit 1, never 0), so CI notices. The only behaviour lost is "an unreachable remote
throws the whole update away", which is the hazard this row removes. No crash log is written because nothing
crashed.

## Examples

One reachable repository with a newer tag, one deleted remote (paths shortened):

```text
$ pre-commit autoupdate        # pre-commit 4.6.2
[/w/hooks] updating v1 -> v2
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.useBuiltinFSMonitor=false', '-C', '/tmp/tmp_vls6xdv', 'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')
return code: 128
stdout: (none)
stderr:
    fatal: '/w/gone' does not appear to be a git repository
    fatal: Could not read from remote repository.

    Please make sure you have the correct access rights
    and the repository exists.
Check the log at /home/u/.cache/pre-commit/pre-commit.log
$ echo $?
3                              # .pre-commit-config.yaml unchanged: both revs still v1
```

```text
$ snp autoupdate
[/w/hooks] updating v1 -> v2
[/w/gone] command: ('/usr/bin/git', '-c', 'core.useBuiltinFSMonitor=false', '-C', '/tmp/tmpxBsvle', 'fetch', 'origin', 'HEAD', '--quiet', '--filter=blob:none', '--tags')
return code: 128
stdout: (none)
stderr:
    fatal: '/w/gone' does not appear to be a git repository
    fatal: Could not read from remote repository.

    Please make sure you have the correct access rights
    and the repository exists.
$ echo $?
1                              # /w/hooks is now at rev v2, /w/gone stays at v1
```

The scenario `tests/differential/divergences/D-CRASH-06.yaml` runs this configuration in both brands.
