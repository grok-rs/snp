# D-DATA-13

per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run…

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-13.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| two concurrent runs in one worktree can stash over each other; a killed run leaves no recovery hint | per-worktree lock (nested runs exempt via token only while the lock is held), durable journal + git-dir pointer, patch re-verified before the checkout, next-run recovery notice on stderr (`git apply --3way`) |

## Rationale

To be written by WP-23: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-23: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-DATA-13.yaml` must keep reproducing the difference.
