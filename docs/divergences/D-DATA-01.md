# D-DATA-01

one stderr line after the envelope: [ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --whitespace=nowarn <patch>, plus what to do when it does not apply (X13-STASH-02)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-23; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-23 (`snp_stash::StashGuard::restore`, the K+ hint of `PyExc::hint`; the envelope
  prints it, `snp::envelope::report`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| final stash-restore failure prints only a traceback; the user does not learn where the unstaged work is | one stderr line after the envelope: `[ERROR] Your unstaged changes are saved in <patch>; restore them with: git apply --whitespace=nowarn <patch>`, plus what to do when it does not apply (X13-STASH-02) |

## Rationale

When `git apply` fails twice, then again after the rollback checkout (`staged_files_only.py:85-96`), pre-commit
raises the last `CalledProcessError` (exit 3). At that point the user's unstaged changes exist **only** in
`<store>/patch<ts>-<pid>`; the exception text names the patch only as an argument of the failed command, and the
crash log is the only further hint. Users who do not recognise this lose their work.

snp keeps every byte of pre-commit's stdout (line 1 of the envelope is the same `CalledProcessError` text, exit 3) and
adds one line on **stderr** after it (ADR-045): where the work is and the command that restores it. The command is
pre-commit's own restore, `git apply --whitespace=nowarn <patch>` (ADR-084): it changes the worktree only and is
all-or-nothing. `--3way` (ADR-067's first choice) is wrong here: it implies `--index`, so it stages the user's
unstaged changes, and it refuses every path whose worktree differs from the index (`does not match index`) — exactly
what a failed rollback leaves. When the hooks' edits are still in the worktree the plain apply refuses, so the line
goes on: `; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes or
later edits): keep a copy with git diff --binary <tree> > <patch>.now, put them back with git --literal-pathspecs
restore --source=<tree> --worktree -- <patch paths…>, then apply it again` (`git restore` leaves the index alone; a
long or unparseable list uses `-- :/`). The patch, the journal and the git-dir pointer are kept, and the next run in
that worktree repeats the notice (D-DATA-13).

Why it is safe: stdout and the exit code are unchanged (K2 holds for stdout), the line goes to stderr only, and it is
printed only on the path where pre-commit has already failed with exit 3.

## Examples

A restore that keeps failing (a hook left a file the patch cannot apply to, and the rollback cannot help either):

```text
$ pre-commit run          # pre-commit 4.6.2, stdout
[WARNING] Unstaged files detected.
[INFO] Stashing unstaged files to /s/patch1791403656-1781218.
…
[WARNING] Stashed changes conflicted with hook auto-fixes... Rolling back fixes...
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', '-c', 'core.autocrlf=false', 'apply', '--whitespace=nowarn', '/s/patch1791403656-1781218')
return code: 1
…
Check the log at /s/pre-commit.log
(stderr: empty)

$ snp run                 # stdout identical; stderr:
[ERROR] Your unstaged changes are saved in /s/patches/patch1791403656-1781218; restore them with: git apply --whitespace=nowarn /s/patches/patch1791403656-1781218; if git apply reports that the patch does not apply, those files changed after the stash (hook fixes or later edits): keep a copy with git diff --binary 4b825dc642cb6eb9a060e54bf8d69288fbee4904 > /s/patches/patch1791403656-1781218.now, put them back with git --literal-pathspecs restore --source=4b825dc642cb6eb9a060e54bf8d69288fbee4904 --worktree -- foo, then apply it again
```

The scenario `tests/differential/divergences/D-DATA-01.yaml` makes every `git apply` fail through a `git` wrapper on
`PATH`; the oracle's stderr stays empty while snp's carries the line. The unit-level proof is
`crates/snp-stash/tests/catalog.rs::x13_stash_02_apply_fails_twice_k_plus` (the hinted command restores the work).
