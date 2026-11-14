# D-DATA-19

The stash checks out only the patch's files

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (the git calls are WP-18's `Git::patch_paths`, `Git::changed_paths`, `Git::rollback_paths` and
  `Git::checkout_paths`); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-19.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented (`snp_stash::enter` and the conflict rollback of `StashGuard::restore`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| the stash's whole-tree `git checkout -- .` (and the conflict rollback's) rewrites every stat-dirty file from the index: a change `git diff` cannot see — hidden by a clean filter, or in an `--assume-unchanged` file — is not in the patch and is lost | the checkout rewrites only the patch's files (`git apply --numstat -z`), the rollback those plus every file `git diff` shows changed, as literal pathspecs on stdin (`git -c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul`); a failing checkout's `CalledProcessError` names that command; a recovery hint with too many files to spell out names them in a kept `<patch>.paths` list instead of restoring the whole worktree |

## Rationale

`staged_files_only._unstaged_changes_cleared` (`staged_files_only.py:53-104`) saves the unstaged changes as
`git diff-index --binary <tree>` and then runs `git -c submodule.recurse=0 checkout -- .`; on a conflict the
rollback runs the same checkout again (`:95`). The patch contains what git can see. The checkout rewrites more: every
index entry whose file is stat-dirty, without looking at its content when the size differs. Two kinds of change are
therefore rewritten but never saved:

* **A change a clean filter hides.** A filter that strips machine-local lines (`filter.<name>.clean = grep -v
  SECRET`, git-crypt, `nbstripout`-style filters) makes the file look unchanged to `git diff`, so it is not in the
  patch; its size differs from the index entry, so the checkout overwrites it. The restore re-applies only the patch:
  the hidden lines are gone (prek issue j178/prek#2912, reproduced with pre-commit 4.7.0).
* **An edit in an `--assume-unchanged` file.** `git diff-index` skips such entries, so the edit is not in the patch;
  `git checkout -- .` ignores the bit and rewrites the file.

Both happen whenever another file has an ordinary unstaged change (otherwise there is nothing to stash).

snp keeps pre-commit's algorithm and narrows the two checkouts:

1. **Stash.** The paths come from git's own patch parser, `git apply --numstat -z <patch>` (it changes nothing):
   exactly the files the restore will re-apply. Those, and only those, are checked out:
   `git -c submodule.recurse=0 checkout --pathspec-from-file=- --pathspec-file-nul`, fed `:(literal)<path>\0` per file
   (no argv limit, no glob or magic; `GIT_*_PATHSPECS` removed from its environment), with
   `_PRE_COMMIT_SKIP_POST_CHECKOUT=1` as before. A `post-checkout` hook still runs (a file checkout, flag 0).
   git before 2.25 has no `--pathspec-from-file`: there the same `:(literal)` pathspecs follow
   `git -c submodule.recurse=0 checkout --` as arguments, in chunks of at most 24 KiB.
2. **Conflict rollback.** The patch's files again (so the patch applies), followed by every other file `git diff
   --name-only` lists after refreshing the stat data — what pre-commit's whole-tree checkout visibly changes: every
   hook fix git can see is discarded exactly as before, the index is untouched, and a change git cannot see stays. A
   failing listing is the rollback's failure (its `CalledProcessError`; the K+ path keeps the patch, D-DATA-01).
3. **Recovery hints.** The fallback of a hint whose apply refuses (D-DATA-01, D-DATA-13) restores the patch's files
   from the stash-time tree. A list too long to spell out used to become `git restore --source=<tree> --worktree --
   :/`, which overwrites hidden changes in every file; it now names a kept NUL-separated list,
   `git --literal-pathspecs restore --source=<tree> --worktree --pathspec-file-nul --pathspec-from-file=<patch>.paths`
   (written next to the patch like `<patch>.ita`; gc removes both with the patch). Only a patch whose headers do not
   parse, or a store that cannot take the list, keeps the whole-worktree form.

What stays identical: every transcript line, the patch bytes, the index, refs, the stash list, and the worktree of
every repository without such hidden changes (the checkout rewrites the same files). What differs: the argv shown in
the `CalledProcessError` of a failing stash or rollback checkout (X13-STASH-01: `('git', '-c',
'submodule.recurse=0', 'checkout', '--pathspec-from-file=-', '--pathspec-file-nul')` instead of `… 'checkout', '--',
'.')`), the worktree after a run with hidden changes, and what hooks read from such a file during the run: its
working-tree content including the hidden change (pre-commit showed the index version, then lost the change). The
listing runs `git apply --numstat -z --whitespace=nowarn`, like the stash's other applies, so `apply.whitespace=error`
cannot fail it.

Residual risk, documented: a file that has both a visible change and a hidden one is in the patch and is checked out,
so its hidden part is lost exactly as in pre-commit (git cannot represent it in a patch); likewise a hidden change in a
file a hook visibly modifies before a conflict rollback. Files with the skip-worktree bit were never touched by either
tool.

**Amendment (ADR-095 K7, fix F3 of docs/VERIFICATION.md §11).** The checkout sink now refuses a list holding an
empty path or a path with a NUL byte (`ValueError`, before any git call, in both the stdin and the git < 2.25 argument
form): an empty `:(literal)` pathspec selects the whole tree, which is the very checkout this divergence removed, and a
NUL would split one item into two. The rollback list drops an empty record of the `git diff --name-only -z` listing
instead of passing it on. git never prints such a path, so no run with a git-made patch or listing changes; the
refusal only turns a should-never-happen whole-tree checkout into an error. The lists themselves are the verified
kernel `crates/snp-git/src/git/pathspec.rs` (VERIFICATION §9.2 K7): the `--numstat -z` paths are non-empty and
NUL-free, the `:(literal)` list reads back as exactly one pathspec per path, the argument chunks concatenate to the
whole list, and the rollback list is the first-seen dedup of the patch's paths followed by the non-empty changed
paths.

## Examples

```text
$ git config filter.strip.clean 'grep -v SECRET'; echo 'secret.cfg filter=strip' > .gitattributes
$ printf 'key=1\nSECRET=hunter2\n' > secret.cfg; echo change >> foo     # secret.cfg looks unchanged to git
$ pre-commit run
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patch1794587008-1190673.
identity.............................................(no files to check)Skipped
[INFO] Restored changes from /s/patch1794587008-1190673.
$ cat secret.cfg
key=1
$ snp run
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patches/patch1794587008-1190673.
identity.............................................(no files to check)Skipped
[INFO] Restored changes from /s/patches/patch1794587008-1190673.
$ cat secret.cfg
key=1
SECRET=hunter2
```

Proof: `crates/snp-stash/tests/behaviour/hidden.rs` (a clean filter at the stash and through a conflict rollback, an
`--assume-unchanged` edit, literal names, a 400-file list); the scenario `tests/differential/divergences/D-DATA-19.yaml`;
for the F3 refusal, `checkout_sink_refuses_empty_and_nul_paths` in `crates/snp-git/src/git.rs` and the empty-record
case of `every_call_can_fail_and_is_recorded` in `crates/snp-git/tests/behaviour/scripted.rs`.
