# D-PERF-05

on Linux a proven stash restore renames the preserved originals back instead of running `git apply`

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-23 (ADR-152, the owner's decision D2 of the wave-6 plan). The backups and the restore
  are in `crates/snp-stash/src/guard.rs` (`Inner::make_backups`, `Inner::rename_restore`), the proof in
  `crates/snp-stash/src/diff_oracle.rs` (`DiffOracle::restore_proof`). Tests: `crates/snp-stash/tests/behaviour/rename.rs`,
  `crates/snp-run/tests/pipeline/restore_rename.rs`, FI-31…FI-34 (`tests/fault.rs`) and the kill -9 rows of
  `tests/driver.rs`. The K9 model proves the outcome equal (docs/VERIFICATION.md §9.2, T7).

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.7.0 | snp |
|---|---|
| the stash's restore runs `git apply`, which writes every file of the patch anew | on Linux, when the DiffOracle proves the hooks left those files as the checkout wrote them and nothing converts them (ADR-152), the originals hard-linked before the checkout are renamed back: same bytes, mode, git state and output; the inode and birth time are the original's, one process fewer runs, and an autosave landing between the final check and the rename is overwritten instead of merged; `SNP_STASH_RESTORE=apply` restores `git apply` |

## Rationale

pre-commit restores the unstaged changes with `git apply --whitespace=nowarn <patch>` (`staged_files_only.py:26-33`):
git reads the whole patch, checks each file's pre-image and writes each file again (unlink, then `open(O_CREAT|O_EXCL)`
with `0666` or `0777` minus the umask). For a large unstaged change that is the longest step after the diff itself
(about 140 ms for a 10 MB rewrite). The stash's checkout unlinks and recreates every file it writes too, so a hard
link made just before it keeps the user's original file, byte for byte, under another name.

When the diff oracle proves that every file of the patch still holds exactly what the checkout wrote (its index
blob: ADR-119's proof of the post-checkout state, and every comparison after the hooks found it unchanged) and that
no attribute or setting converts it, `git apply` would succeed and leave each file holding exactly the user's
original bytes. snp then renames the originals back instead, after checking every file again (stat and, when racy,
content; one link; owner, group; no extended attributes; no mode change in the patch), and gives each the mode and
times `git apply`'s fresh file would have (the checkout's fresh file has that mode already). Any doubt, error or
partial failure falls back to `git apply` (a partial rename first checks the files out again).

## What differs

* The restored file is the original inode: its inode number and birth time are the user's file's, not a new file's.
  Its content, mode, owner, group and modification time (now) are what `git apply` leaves, and `git status`,
  `git diff` and the index report exactly the same.
* One process fewer (no restore `git apply`).
* A write to a file of the patch that lands between snp's final check of that file and its rename (microseconds) is
  overwritten. `git apply` has a window of the same kind (it reads, computes, then unlinks and recreates the file),
  but merges an edit outside the user's hunks that lands before it reads; with the rename such an edit is merged only
  when it lands before the final check (which then falls back to `git apply`).
* Not on macOS, Windows or other systems, and never with `SNP_STASH_RESTORE=apply`.

## Reproduction

Inode identity and the race window are not visible to the differential harness, which compares bytes, modes, the
index and the transcripts (all equal by design): the scenario file is `status: unit` and names the tests that pin it.
