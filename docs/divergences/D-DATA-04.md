# D-DATA-04

gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| restored stash patches are kept forever | gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE |

## Rationale

pre-commit writes the unstaged changes of every stashing run to `<store>/patch<epoch>-<pid>`
(`staged_files_only.py:68`) and never deletes them, not even in `gc` (03 §7.10): long-lived stores accumulate one
file per commit with unstaged changes, each possibly holding secrets the user never committed.

snp's stash (WP-23) writes its patches to `<store>/patches/` and a journal `run-<pid>-<rand>.snp-journal` that names
the patch; the journal is deleted when the restore completes. `Store::sweep` (run by gc under the exclusive store
lock) removes a `patch*` file only when it is older than 30 days **and** no remaining journal mentions it (a whole
file-name match). A patch whose restore failed or was interrupted — the user's only copy of their work — keeps its
journal and is never removed; journals are never removed by gc. `clean` likewise keeps journals and the patches they
mention.

Safe: only patches whose content is already back in the worktree are deleted, and only after a month in which the
`[INFO] Restored changes from <patch>.` line pointed at them.

## Examples

Not reproducible through the CLI within a test run (the patch must be 30 days old); the behaviour is pinned by
`crates/snp-store/tests/support/behaviour.rs::sweep_keeps_unrestored_patches_and_referenced_tools`, which calls
`Store::sweep` with a clock 31 days ahead: `patch1700000000-41` (no journal) is removed, `patch1700000000-42`
(mentioned by `run-42-abcd.snp-journal`, state `failed`) and the journal stay. pre-commit 4.6.2 keeps both patches
(`gc` prints `0 repo(s) removed.` and never looks at them); the scenario file records `status: unit`.
