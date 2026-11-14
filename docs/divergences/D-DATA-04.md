# D-DATA-04

gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE

* **Class:** STORE (COMPATIBILITY §5.4)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-04.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.7.0 | snp |
|---|---|
| restored stash patches are kept forever | gc removes *restored* patches older than 30 days (never unrestored ones) — class STORE |

## Rationale

pre-commit writes the unstaged changes of every stashing run to `<store>/patch<epoch>-<pid>`
(`staged_files_only.py:68`) and never deletes them, not even in `gc` (03 §7.10): long-lived stores accumulate one
file per commit with unstaged changes, each possibly holding secrets the user never committed.

snp's stash (WP-23) writes its patches to `<store>/patches/` and a journal `run-<pid>-<rand>.snp-journal` that names
the patch; the journal is deleted when the restore completes. `Store::sweep` (run by gc under the exclusive store
lock) removes a `patch*` file only when it is older than 30 days **and** no remaining journal that is not in state
`restored`/`done` names it (a journal left behind by a crash between the restore and its deletion protects nothing:
that patch's content is back in the worktree). A patch whose restore failed or was interrupted — the user's only copy
of their work — keeps its journal and is never removed; journals are never removed by gc. The protection is fail
closed:

* each journal is decoded with the stash's byte-string codec: its `patch` field is a JSON string, or `{"hex": …}`
  for a store at a non-UTF-8 path, and the patch is the file that path names;
* an unrestored patch protects its `<patch>.*` siblings as well: the `<patch>.now` backup the recovery hint tells the
  user to write, and the kept `<patch>.ita` and `<patch>.paths` lists the notices name (`patch<s>-<pid>-<n>` is
  another patch, not a sibling);
* a journal is read in full (up to snp-stash's 64 MiB journal limit; a long intent-to-add list can push `patch` past
  any smaller cap);
* a journal that cannot be read (other than one deleted meanwhile by a completed restore), is bigger than that limit,
  is not a JSON object with a string `state`, or names its patch in a form that does not decode keeps **every** patch,
  and gc says so on stderr: `[WARNING] the stash journal <journal> cannot be read; every stash patch is kept.`
  (D-DIAG-05). The same holds when `patches/` cannot be listed. An empty or truncated journal left by a crash
  while the stash was creating it (the stash's recovery ignores such a file) keeps every patch until the user
  removes it; the warning names it. A journal caught in the instant between its creation and its first write gives
  the same warning once, harmlessly.

`clean` (pre-commit: `rmtree` of the whole store, `commands/clean.py:10-16`) removes snp's own layout and likewise
keeps recovery data, so a differential `tree` comparison of `clean` differs in these entries, all of them deliberate:

* every journal, the patches of unrestored journals with their `<patch>.*` siblings, and every `patch*` younger than
  an hour — a run stashing in another worktree creates its patch before it journals it (ARCHITECTURE §4 steps 5–6) and
  takes the shared store lock only at its first repository use, so such a patch may be about to be restored; the
  journals are re-read right before each removal, and a journal that cannot be read keeps every patch (with the same
  stderr warning);
* `.lock`, so that processes already waiting on the global lock keep excluding each other (removing it would let a
  waiter and a newcomer lock two different inodes);
* files outside snp's layout (anything a user put into `SNP_HOME`), and snp 1.x names unless the README carries the
  1.x signature: snp deletes only what it can prove it created.

Safe: only patches whose content is already back in the worktree are deleted, and only after a month in which the
`[INFO] Restored changes from <patch>.` line pointed at them.

## Examples

Not reproducible through the CLI within a test run (the patch must be 30 days old); the behaviour is pinned by
`crates/snp-store/tests/support/behaviour.rs::sweep_keeps_unrestored_patches_and_referenced_tools`, which calls
`Store::sweep` with a clock 31 days ahead: `patch1700000000-41` (no journal) is removed, `patch1700000000-42`
(mentioned by `run-42-abcd.snp-journal`, state `failed`) and the journal stay, and `patch1700000000-43` (mentioned by
the `restored` journal `run-43-abcd.snp-journal`) is removed. `clean_retires_the_layout_but_keeps_unknown_and_recovery_data`
pins the `clean` entries above. The fail-closed rules are pinned by
`sweep_and_clean_keep_every_patch_when_a_journal_is_unreadable` (a journal that is a directory, a truncated journal),
`sweep_protects_the_patch_of_a_journal_over_one_mebibyte`, `sweep_and_clean_protect_a_patch_journaled_as_hex` (Unix)
and `sweep_and_clean_protect_the_siblings_of_an_unrestored_patch` in the same file, by the unit tests of
`crates/snp-store/src/maintenance.rs` (the 64 MiB limit included), and through the CLI by
`crates/snp/tests/it/gc.rs::an_unreadable_stash_journal_keeps_every_patch_with_a_warning`. pre-commit 4.7.0 keeps both patches
(`gc` prints `0 repo(s) removed.` and never looks at them); the scenario file records `status: unit`.
