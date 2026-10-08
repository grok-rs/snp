# D-DATA-17

a stash that would be non-empty on git < 2.19 is refused before any mutation: An error has occurred: FatalError: git <V> cannot stash intent-to-add files safely…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23 (contributing: WP-18); the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-17.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| git < 2.19 does not report intent-to-add files: a stash with an ITA file fails after the rollback (`already exists in working directory`, exit 3), leaving the unstaged work only in the patch (02 G.3); runs without a stash work | a stash that would be non-empty on git < 2.19 is refused before any mutation: `An error has occurred: FatalError: git <V> cannot stash intent-to-add files safely; snp requires git 2.19.0 or newer to stash unstaged changes (found <git>).` (exit 1); every run that needs no stash works as in pre-commit |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-17.yaml` must keep reproducing the difference.
