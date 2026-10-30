# D-DATA-17

a stash that would be non-empty on git < 2.19 is refused before any mutation: An error has occurred: FatalError: git <V> cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (the version facts are WP-18's `GitCaps`); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-17.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-23 (`StashCtx::old_git`, set by the caller from `GitCaps.version < 2.19`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| git < 2.19 does not report intent-to-add files: a stash with an ITA file fails after the rollback (`already exists in working directory`, exit 3), leaving the unstaged work only in the patch (02 G.3); runs without a stash work | a stash that would be non-empty on git < 2.19 is refused before any mutation: `An error has occurred: FatalError: git <V> cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged changes (found <git>).` (exit 1); every run that needs no stash works as in pre-commit |

## Rationale

Before git 2.19, `git diff` does not show intent-to-add entries as new files, so `git.intent_to_add_files` returns
nothing (02 G.3, observed on 2.18.5). An intent-to-add file then ends up in the stash patch as an addition while it
stays in the index; the checkout leaves it, both applies fail with `already exists in working directory`, the
rollback does not help, and pre-commit exits 3 with the user's unstaged changes only in the patch.

snp cannot detect intent-to-add files on such a git either, so it does not try (ADR-063): when `diff-index` shows
unstaged changes on git < 2.19, snp refuses before writing a patch or touching the worktree. A run with nothing to
stash (`diff-index` clean or the CRLF quirk) proceeds exactly as in pre-commit, so commits without unstaged changes
keep working on old git.

Why it is safe: K5 holds (a refusal is non-zero, like pre-commit's crash on the affected path); nothing is mutated; the
message names the fix (upgrade git). The refusal also covers unstaged changes without intent-to-add files, which
pre-commit stashes correctly on 2.18 — a deliberate, documented over-approximation, because snp cannot tell the two
apart without the git feature that is missing.

## Examples

```text
$ /opt/git-2.18.5/bin/git add -N n.txt; echo change >> b.txt   # an ITA file and an unstaged change
$ pre-commit run
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patch1791403656-1781218.
…
An unexpected error has occurred: CalledProcessError: command: ('/opt/git-2.18.5/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '/s/patch1791403656-1781218')
…
    error: n.txt: already exists in working directory
$ snp run
An error has occurred: FatalError: git 2.18.5 cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged changes (found /opt/git-2.18.5/bin/git).
```

Proof: `crates/snp-stash/tests/behaviour/guard.rs::old_git_refuses_a_non_empty_stash_before_any_mutation` (the
refusal comes before any patch or checkout; a clean run proceeds); the scenario
`tests/differential/divergences/D-DATA-17.yaml` runs on the CI git 2.18.5 container.
