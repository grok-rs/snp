# D-DATA-11

GIT_DIFF_OPTS removed for the stash diff-index

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (the argv/env of the call is WP-18's `Git::diff_index_patch`); the first package of the register
  row owns this file and `tests/differential/divergences/D-DATA-11.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented (`snp_git::Git::diff_index_patch` removes the variable; `snp_stash::enter` uses it).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| `GIT_DIFF_OPTS=--unified=0` makes the stash patch unrestorable (hazard a) | `GIT_DIFF_OPTS` removed for the stash `diff-index` |

## Rationale

`GIT_DIFF_OPTS=--unified=<n>` changes the context of every patch git prints, `diff-index` included (02 G.0 hazard (a),
observed on 4.7.0). With `--unified=0` the stash patch has no context lines; `git apply` refuses zero-context hunks
unless `--unidiff-zero` is given, so both applies fail, the rollback does not help, and pre-commit exits 3 with the
user's unstaged changes only in the patch file — on every commit with unstaged changes, as long as the variable is
exported (some users set it for their own `git diff`).

snp removes `GIT_DIFF_OPTS` from the environment of the stash's `diff-index` only (02 G.7 row 2). The modification
detection diff (`git diff --no-ext-diff --no-textconv --ignore-submodules`) keeps it — it is compared only for
equality, so it is harmless there — and hooks still see it.

Why it is safe: the variable has no other effect on that call; on every path where pre-commit's stash works, the
patch is byte-identical (the default context is 3 lines either way).

## Examples

```text
$ GIT_DIFF_OPTS=--unified=0 pre-commit run      # unstaged change + a non-conflicting hook fix
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patch1791403656-1781218.
fixer....................................................................Failed
…
[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '/s/patch1791403656-1781218')
return code: 1
…
$ GIT_DIFF_OPTS=--unified=0 snp run
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patches/patch1791403656-1781218.
fixer....................................................................Failed
…
[INFO] Restored changes from /s/patches/patch1791403656-1781218.
```

Proof: `crates/snp-stash/tests/behaviour/guard.rs::hazard_a_git_diff_opts_unified_zero_is_restorable`,
`behaviour/stage_fixed.rs::binary_patch_and_unified_zero_are_restorable_with_staged_fixes`; the scenario
`tests/differential/divergences/D-DATA-11.yaml`.
