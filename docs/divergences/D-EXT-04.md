# D-EXT-04

_SNP_STASH_LOCK=<token> is exported to hooks while a stash is held (nested runs skip the worktree lock) — class DATA

* **Class:** DATA (COMPATIBILITY §5.7)
* **Owner:** WP-23 (exported by WP-32's run pipeline from `StashGuard::child_env_token`); the first package of the
  register row owns this file and `tests/differential/divergences/D-EXT-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-23 (`StashGuard::child_env_token`, the nested-run check of `snp_stash::enter`).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| `_SNP_STASH_LOCK=<token>` is exported to hooks while a stash is held (nested runs skip the worktree lock) — class DATA |

## Rationale

The worktree lock of D-DATA-13 would deadlock a hook that itself runs snp in the same worktree (a wrapper hook, a
`snp run` inside a `make` target, `snp try-repo` from a hook): the nested run would wait for the lock its parent holds
until it times out. pre-commit has no lock, and its nested run simply sees the stashed (clean) worktree.

snp therefore exports one extra variable to hooks while its guard is alive: `_SNP_STASH_LOCK=<token>`, a random
128-bit hex token that is also written into the held lock file. A run that finds the lock held **and** its own
`_SNP_STASH_LOCK` equal to the file's token skips the lock and behaves as pre-commit's nested run does; a token that
matches a released lock (left behind by a crash) exempts nobody, and releasing the lock truncates the token. The name
starts with an underscore like pre-commit's own `_PRE_COMMIT_SKIP_POST_CHECKOUT`. It is class DATA (not EXT) because it
is always on and exists to protect the stash.

Why it is safe: one additional environment variable is visible to hooks; nothing else about the hook environment
(02 §6.1) changes, and no hook relies on the absence of an underscore-prefixed snp variable.

## Examples

```text
$ cat .pre-commit-config.yaml   # a hook that prints its environment
…  entry: sh -c 'env | grep _SNP_STASH_LOCK' --
$ pre-commit run --verbose
(nothing printed by the hook; it fails because grep finds nothing)
$ snp run --verbose
_SNP_STASH_LOCK=5f0c…e21a                     (32 hex digits)
```

Proof: `crates/snp-stash/tests/behaviour/guard.rs::{nested_run_with_the_token_proceeds_while_the_lock_is_held,
stale_token_in_a_released_lock_file_exempts_nobody}`; the scenario `tests/differential/divergences/D-EXT-04.yaml`.
