# D-PERF-03

native implementations (fast path, ADR-031); output byte-identical; Tier A repos are never cloned (FD-8)

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-31 (contributing: WP-26, WP-27); the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-31: the gates G1–G6 and the tiers in `crates/snp-repo/src/fastpath.rs`, decided inside
  `StoreResolver::all_hooks` (`crates/snp-repo/src/resolve.rs`); the `keys-v1` markers and the virtual python
  `InstallKey` that keep the transcript (`crates/snp-repo/src/install.rs`, ADR-032). The native hooks themselves are
  WP-26 / WP-27 (`snp_hooks::native`). Tests: `crates/snp-repo/tests/acceptance.rs`
  (`tier_a_cold_and_warm_transcripts_without_a_clone` against the committed pc462-py312 golden,
  `tier_b_identical_fork_is_native_and_a_modified_one_delegates`, `g4_overrides_from_the_config_and_from_an_overlay_patch`,
  `tier_a_hook_whose_preflight_fails_is_cloned_in_all_hooks_and_installed_later`) and end to end in
  `crates/snp/tests/it/repository.rs`.

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.6.2 | snp |
|---|---|
| pre-commit-hooks run from a cloned venv | native implementations (fast path, ADR-031); output byte-identical; Tier A repos are never cloned (FD-8) |

## Rationale

`pre-commit/pre-commit-hooks` is by far the most used hook repository. pre-commit 4.6.2 clones it, builds a
virtualenv, `pip install`s the package and then starts one CPython interpreter per xargs partition for every hook. The
owner decision FD-8 makes snp run these hooks natively instead (ARCHITECTURE §7.2, ADR-031): no clone, no venv, no
interpreter start-up, and — for Tier A revisions (a known tag or its full commit) — no network at all.

The divergence is confined to **what is on disk and what is spawned**; everything a user or a script can read from the
run is unchanged:

* hook output, exit codes, file modifications and their order are byte-identical (every native hook passes its
  parity corpus against the pinned upstream module; caveat hooks stay delegated until their CI parity gate passes);
* the cold transcript is byte-identical too: `[INFO] Initializing environment for {repo}.` prints when pre-commit
  would insert its `db.db` row (a `keys-v1` marker records the first sight of `(repo, rev)`), and the `Installing
  environment` triplet prints exactly once for the python environment pre-commit would have built — a *virtual*
  install (ADR-032). The warm transcript prints neither;
* any doubt delegates to pre-commit's behaviour: a Tier B revision is cloned and its module plus import closure must
  match `VERIFIED_BLOBS`; a hook whose config or local overlay sets `language`, `entry` or non-empty
  `additional_dependencies` (G4), whose arguments the native preflight does not accept (G5), whose epoch predates
  v3.0.0 (G6), or that the user excluded (`--no-fast-path`, `SNP_NO_FAST_PATH`, `fast_path: off`,
  `fast_path_exclude`, G1) is cloned and installed exactly like pre-commit, at pre-commit's step.

It is safe because the only observable differences are the store contents (no `repo*` clone, no `py_env-*`) and the
absence of the child python processes — neither is part of pre-commit's documented interface, and both make the run
faster and offline-capable without changing a byte of output. A moved upstream tag cannot inject code: Tier A uses the
embedded manifest and profile, Tier B reads only blob ids.

## Examples

```yaml
repos:
-   repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v6.0.0
    hooks:
    -   id: trailing-whitespace
    -   id: end-of-file-fixer
    -   id: check-json
```

`run --all-files` over `a.json` = `{}\n` on a cold store. pre-commit 4.6.2 (pc462-py312) and snp print the same bytes,
exit 0 (`crates/snp-repo/tests/goldens/pc462-py312-pch-v6-cold.txt`):

```
[INFO] Initializing environment for https://github.com/pre-commit/pre-commit-hooks.
[INFO] Installing environment for https://github.com/pre-commit/pre-commit-hooks.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
check json...............................................................Passed
```

The stores differ. pre-commit's `$PRE_COMMIT_HOME` afterwards holds `db.db`, `README` and one `repo<random>/` with
the clone and its `py_env-python3/`; `git fetch` of the upstream URL ran. snp's store holds no
`repos-v1/` entry and no environment directory — only the `keys-v1` markers — and spawned neither git nor python.
The scenario in `tests/differential/divergences/D-PERF-03.yaml` asserts exactly that: identical stdout/stderr/exit,
different store.
