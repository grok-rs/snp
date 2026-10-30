# D-DATA-08

keyed by the canonical absolute path; messages unchanged

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-22; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-08.yaml` (WBS "Divergence files", ADR-071).
* **Status:** complete (WP-22).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| relative local-path repos (`repo: ../tools`) share one cache entry across projects | keyed by the canonical absolute path; messages unchanged |

## Rationale

pre-commit keys its `repos` table by the `repo:` string verbatim (`db_repo_name`, `store.py:125-171`). Two projects
whose configs both say `repo: ../tools`, with `rev: HEAD` (or the same tag name), share one row: the second project
silently runs the first project's hooks — different code than its config names, from another directory. The same
happens to one project moved or cloned twice.

snp derives repository keys only through `RepoId::from_config(repo, project_root)` (ADR-070): a `repo:` value that
names an existing local directory (relative values resolved against the project root, `os.path.isdir` semantics) is
identified by its `realpath` (`abs:` + path bytes in the blake3 key), every other value by the string verbatim. gc
resolves tracked configs against their recorded project roots (ADR-075). Messages keep the verbatim string:
`[INFO] Initializing environment for ../tools.`.

Safe: each project runs the hooks its own config names. The cost is one extra clone per distinct directory, which is
what the configs ask for.

## Examples

Recorded with pre-commit 4.6.2 on CPython 3.12.3 (a venv built from the hash lock `xtask/oracle/requirements-pc462.txt`); `<W>` is the scratch root, `<store>` the store directory.

Projects `one/proj` and `two/proj`, each with `repo: ../tools`, `rev: HEAD` and a hook `which` whose entry echoes the
project name of its `tools` directory; `run --all-files` in `one/proj`, then in `two/proj`
(`tests/differential/divergences/D-DATA-08.yaml`). pre-commit 4.6.2 in `two/proj`, exit 0:

```text
[WARNING] The 'rev' field of repo '../tools' appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.
which....................................................................Passed
- hook id: which
- duration: 0s

tools of project one

```

snp in `two/proj`, exit 0 (its own `tools`, newly initialised):

```text
[WARNING] The 'rev' field of repo '../tools' appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: `pre-commit autoupdate` often fixes this.
[INFO] Initializing environment for ../tools.
which....................................................................Passed
- hook id: which
- duration: 0s

tools of project two

```

Unit coverage: `crates/snp-store/tests/support/behaviour.rs::repo_id_from_config_uses_the_project_root`.
