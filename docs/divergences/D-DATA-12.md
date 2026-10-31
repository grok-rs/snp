# D-DATA-12

--no-renames on the ITA query

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (the argv is WP-18's `Git::intent_to_add`); the first package of the register row owns this file
  and `tests/differential/divergences/D-DATA-12.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented (`snp_git::Git::intent_to_add` adds `--no-renames`; `snp_stash::enter` uses it).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| an intent-to-add file paired as a rename is missed ⇒ restore fails and a deletion is lost (hazard b) | `--no-renames` on the ITA query |

## Rationale

`git.intent_to_add_files` (`git.py:146-151`) lists intent-to-add files as `--diff-filter=A` entries of `git diff`.
With rename detection on (`diff.renames=true`, git's default), an intent-to-add file whose content resembles a tracked
file deleted in the worktree is reported as a rename (`R`), not an addition, so it is missed (02 G.0 hazard (b),
observed on 4.6.2). It then stays in the index as an intent-to-add entry: the stash patch contains it as a new file,
the checkout leaves it in place, every `git apply` fails with `already exists in working directory`, pre-commit exits
3, and the user's unstaged deletion is lost from the worktree (it exists only in the patch).

snp adds `--no-renames` to that one query (02 G.7 row 1): additions are then always reported as additions. The other
`--name-only` queries keep git's (and the user's) rename settings where they change pre-commit's output (02 G.0
item 9).

Why it is safe: on every repository where the query does not pair a rename, its output is byte-identical; where it
does, pre-commit loses data and snp does not.

## Examples

```text
$ git rm --cached -q a.txt; mv a.txt b.txt; git add -N b.txt   # (or: delete a.txt, create a similar b.txt, add -N)
$ pre-commit run
[INFO] Stashing unstaged files to /s/patch1791403656-1781218.
…
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '/s/patch1791403656-1781218')
return code: 1
stdout: (none)
stderr:
    error: b.txt: already exists in working directory
$ snp run
[WARNING] Unstaged intent-to-add files detected.
[WARNING] Unstaged files detected.
…
[INFO] Restored changes from /s/patches/patch1791403656-1781218.
```

Proof: `crates/snp-stash/tests/behaviour/guard.rs::hazard_b_intent_to_add_paired_as_a_rename`; the scenario
`tests/differential/divergences/D-DATA-12.yaml`.
