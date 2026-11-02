# D-DATA-07

value repaired (all lines equal and an existing directory ⇒ use one line)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-07.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::hook_impl::repair_hook_dir`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.7.0 | snp |
|---|---|
| an exported `CDPATH` corrupts `--hook-dir`; legacy hooks (git-lfs pre-push) are silently skipped | value repaired (all lines equal and an existing directory ⇒ use one line) |

## Rationale

The installed hook script computes its directory with `HERE="$(cd "$(dirname "$0")" && pwd)"`. With an exported
`CDPATH` that matches, bash's `cd` prints the directory it changed to, so `HERE` holds it twice (`/r/.git/hooks` +
newline + `/r/.git/hooks`). pre-commit receives that as `--hook-dir`, looks for
`/r/.git/hooks\n/r/.git/hooks/pre-push.legacy`, does not find it, and silently skips the legacy hook — for git-lfs users
this means `git push` no longer uploads LFS objects, and nothing says so.

snp repairs exactly that shape: when the value contains newlines, every non-empty line is identical and that line is
an existing directory, the line is used. Any other value is kept verbatim (it then behaves as in pre-commit).
On Windows the script runs in Git for Windows' MSYS2 shell, whose `pwd` prints `/d/a/x` for `D:\a\x`; MSYS2 converts
a single path argument for a native program but not the two-line value, so each line is read as `D:/a/x` there before
the comparison.

Why it is safe: the repaired value is the directory the script meant to pass; the condition cannot match a legitimate
directory name (a path made of two identical newline-separated copies of an existing directory is not a real hook
directory); every other input is untouched.

## Examples

```text
$ export CDPATH=.            # the script's `cd` now echoes the directory
$ git push                   # pre-push.legacy = git-lfs's hook
# pre-commit: the legacy hook is skipped, LFS objects are not pushed
# snp: --hook-dir is repaired, the legacy hook runs, then snp's pre-push hooks
```

Proof: `crates/snp-run/tests/pipeline/hook_impl.rs::d_data_07_cdpath_doubled_hook_dir` and the unit test
`snp_run::hook_impl::tests::d_data_07_cdpath_repair`; the scenario `tests/differential/divergences/D-DATA-07.yaml`.
