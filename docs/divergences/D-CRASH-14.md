# D-CRASH-14

stderr message, legacy rc 1, snp's hooks still run

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-14.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::hook_impl`, the legacy phase).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| broken legacy hook (missing shebang interpreter, directory) → exit 3 (X13-HI-07) | stderr message, legacy rc 1, snp's hooks still run |

## Rationale

`hook-impl` runs `<hook-dir>/<type>.legacy` first (migration mode: the hook that was there before `install`). When
that file cannot be executed — its `#!` interpreter does not exist, or it is a directory (which passes
`os.access(X_OK)`) — pre-commit's `normalize_cmd` raises `ExecutableNotFoundError` out of `hook_impl`, and the git hook
dies with exit 3 before any of the configured hooks has run.

git itself treats a broken hook as a failed hook, not as a crash. snp does the same: the message
(``Executable `/nonexistent/interp` not found``) goes to stderr, the legacy return code is 1, and snp's hooks run as
usual; the hook's exit code is `1 | run`, so the commit is still refused (13 X13-HI-07, decision C-err).

Why it is safe: the result is still a failure (non-zero), the cause is printed, and the user's configured hooks get to
check the commit instead of being skipped by an unrelated crash.

## Examples

```text
$ printf '#!/nonexistent/interp\n' > .git/hooks/pre-commit.legacy; chmod +x .git/hooks/pre-commit.legacy
$ pre-commit hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit --hook-dir "$PWD/.git/hooks" --; echo $?
An unexpected error has occurred: ExecutableNotFoundError: Executable `/nonexistent/interp` not found
Check the log at ~/.cache/pre-commit/pre-commit.log
3
$ snp hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit --hook-dir "$PWD/.git/hooks" --; echo $?
Executable `/nonexistent/interp` not found          (stderr)
passing..................................................................Passed
1
```

Proof: `crates/snp-run/tests/pipeline/hook_impl.rs::x13_hi_07_broken_legacy_hook_fails_but_hooks_run`; the scenario
`tests/differential/divergences/D-CRASH-14.yaml`.
