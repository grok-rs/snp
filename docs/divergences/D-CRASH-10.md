# D-CRASH-10

any all-zero OID (40 or 64 hex) is null

* **Class:** CRASH (COMPATIBILITY §5.1)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-CRASH-10.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-18 (`snp_git::Git::pre_push`, `crates/snp-git/src/git.rs`; ADR-022).

## Register entry (COMPATIBILITY §5.1)

| pre-commit 4.6.2 | snp |
|---|---|
| pre-push with a 64-hex all-zero OID (SHA-256 repo) → crash on branch deletion | any all-zero OID (40 or 64 hex) is null |

## Rationale

git writes the null object id of the repository's hash algorithm on the pre-push stdin: 40 zeros for SHA-1, 64
zeros for SHA-256 (`transport.c`, `oid_to_hex(null_oid())`). `hook_impl._pre_push_ns` (`hook_impl.py:131-133`)
compares both the local and the remote sha with the constant `Z40 = '0' * 40` only. In a SHA-256 repository a branch
deletion (`git push origin :gone`, local sha = 64 zeros) is therefore not recognised as a deletion: pre-commit asks
`git cat-file -e <remote>^{commit}`, builds `from_ref=<remote>, to_ref=<64 zeros>` and runs the hooks on the range,
whose `git diff … <remote>...<zeros>` fails — `CalledProcessError`, exit 3, and the push is rejected. A deletion
pushes no content, so there is nothing to check; every pushing user of a SHA-256 repository hits this on every
branch deletion, and Git 3.0 makes SHA-256 the default for new repositories.

snp treats an OID as null when it is 40 **or** 64 ASCII zeros (`types::is_null_oid`), for the local sha (the line is
skipped) and for the remote sha (a new branch: the `rev-list --not --remotes=<remote>` path). SHA-1 repositories see
no change: 40 zeros were null before and still are. The fix cannot turn a failing push into a passing one for a line
that carries content: only all-zero ids, which name no object, are affected (K5).

## Examples

SHA-256 repository with remote branch `gone` at `1200b96c…df49`; `git push origin :gone` gives the stdin
`(delete) 0000…0000 (64) refs/heads/gone 1200b96c26decb6aeb6921e88cae7cee8e01c47af133f254e9f8bfc66172df49`, and the hook
runs `pre-commit hook-impl --hook-type=pre-push --hook-dir .git/hooks -- origin ../remote.git`.

pre-commit 4.6.2 (oracle `pc462-py312`, git 2.43.0), exit **3**, stdout:

```
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', 'diff', '--name-only', '--no-ext-diff', '-z', '1200b96c26decb6aeb6921e88cae7cee8e01c47af133f254e9f8bfc66172df49..0000000000000000000000000000000000000000000000000000000000000000')
return code: 128
stdout: (none)
stderr:
    fatal: Invalid revision range 1200b96c26decb6aeb6921e88cae7cee8e01c47af133f254e9f8bfc66172df49..0000000000000000000000000000000000000000000000000000000000000000
Check the log at <store>/pre-commit.log
```

snp: the line is a deletion, no namespace is produced (`pre_push` returns `None`), hook-impl returns the legacy hook's
status: exit **0**, no output, the push proceeds — exactly what pre-commit does for the same deletion in a SHA-1
repository.

Regression tests: `crates/snp-git/tests/behaviour/scripted.rs` (`pre_push_errors_use_the_stdlib_format_with_a_bare_git`:
a 64-zero deletion is skipped without any git call), the V-PP vector `(delete) <Z64> …` in
`crates/snp-git/tests/vectors/v_pp.json`, and the fuzz target `git_pre_push` (a namespace never has a null `to_ref`).
