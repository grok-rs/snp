# D-SEC-12

more than 64 MiB fails closed before the legacy hook and the hooks run: An error has occurred: FatalError: snp: hook stdin exceeds 64 MiB; refusing to run…

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-12.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.6.2 | snp |
|---|---|
| pre-push stdin (and the `use_stdin` copy) is read without a limit | more than 64 MiB fails closed before the legacy hook and the hooks run: `An error has occurred: FatalError: snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list` (exit 1); never truncated |

## Rationale

To be written by WP-32: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-32: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-SEC-12.yaml` must keep reproducing the difference.
