# D-PLAT-02

falls back to the interpreter's basename on PATH (Git for Windows semantics)

* **Class:** PLAT (COMPATIBILITY §5.5)
* **Owner:** WP-15; the first package of the register row owns this file and
  `tests/differential/divergences/D-PLAT-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** stub, created by WP-00. The owner replaces the two sections below.

## Register entry (COMPATIBILITY §5.5)

| pre-commit 4.6.2 | snp |
|---|---|
| Windows: a missing absolute shebang interpreter (`#!/bin/sh`) fails the hook / crashes the legacy hook | falls back to the interpreter's basename on PATH (Git for Windows semantics) |

## Rationale

To be written by WP-15: why snp deviates from pre-commit 4.6.2 here, and why the deviation is safe.

## Examples

To be written by WP-15: a minimal reproduction with the exact pre-commit 4.6.2 bytes and the exact snp bytes; the
scenario in `tests/differential/divergences/D-PLAT-02.yaml` must keep reproducing the difference.
