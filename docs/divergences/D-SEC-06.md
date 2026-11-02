# D-SEC-06

one retry with the other `--object-format` after `mismatched algorithms`

* **Class:** SEC (COMPATIBILITY §5.3)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-SEC-06.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-18 (`snp_git::OwnedRepo::clone_rev`, `crates/snp-git/src/owned.rs`; 02 G.7 row 6).

## Register entry (COMPATIBILITY §5.3)

| pre-commit 4.7.0 | snp |
|---|---|
| clones fail when the user's git defaults to SHA-256 objects (hazard d) | one retry with the other `--object-format` after `mismatched algorithms` |

## Rationale

`store.clone` creates the clone with `git init --template=` and then fetches (`store.py:185-210`). `git init` picks
the object format from the user's configuration (`init.defaultObjectFormat`, git ≥ 2.49; the default of Git 3.0), a
fetch cannot change it, and git refuses to fetch between formats: `fatal: mismatched algorithms: client sha256;
server sha1`. With a SHA-256 default every SHA-1 hook repository — practically all of them — fails to clone (exit 3
on every run), and the same happens the other way round when a SHA-256 hook repository is used from a SHA-1 default.
`GIT_DEFAULT_HASH` cannot cause it (`no_git_env` strips it), so only the configuration matters (02 G.3 hazard d).

snp runs pre-commit's exact sequence first (identical argv, identical errors in every case that already works). When
it fails and git's stderr reports the mismatch (`mismatched algorithms`, or — in a translated message — both
algorithm names, the last one being the server's), and git supports `init --object-format` (≥ 2.29), the clone
directory is emptied and the whole sequence runs once more after `git init --template= --object-format=<server's
format>`. A second failure is reported as is. Nothing else changes: the retry only ever turns an exit-3 clone failure
into a working clone of exactly the requested revision.

## Examples

A SHA-256 hook repository (`git init --object-format=sha256`, one commit `<64-hex>`); the user's git has the default
SHA-1 object format; `.pre-commit-config.yaml` pins `rev: <64-hex>`; `pre-commit run --all-files`.

pre-commit 4.7.0 (oracle `pc470-py312`, git 2.43.0), exit **3**, stdout:

```
[INFO] Initializing environment for <root>/hooks.
An unexpected error has occurred: CalledProcessError: command: ('/usr/bin/git', 'fetch', 'origin', '--tags')
return code: 128
stdout: (none)
stderr:
    fatal: mismatched algorithms: client sha1; server sha256
Check the log at <store>/pre-commit.log
```

snp: `[INFO] Initializing environment for <root>/hooks.`, the clone is retried as SHA-256 and the hook runs (`x…
Passed`), exit **0**. The mirror case (a user `init.defaultObjectFormat=sha256` against SHA-1 hook repositories,
git ≥ 2.49) is the same code path; CI's latest-git job covers it.

Regression tests: `crates/snp-git/tests/behaviour/hazards.rs` `hazard_d_object_format_mismatch_is_retried_once` (real
git: a SHA-256 upstream cloned with a SHA-1 default ends up a SHA-256 clone at the requested commit; the upstream is
unchanged) and the unit test `owned::tests::object_format_from_mismatch_message` (English and translated messages).
