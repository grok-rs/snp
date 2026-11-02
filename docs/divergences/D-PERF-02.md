# D-PERF-02

placeholder install skipped

* **Class:** PERF (COMPATIBILITY §5.6)
* **Owner:** WP-24; the first package of the register row owns this file and
  `tests/differential/divergences/D-PERF-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** filled by WP-24 (`crates/snp-lang/src/langs/python.rs`).

## Register entry (COMPATIBILITY §5.6)

| pre-commit 4.7.0 | snp |
|---|---|
| local `language: python` hooks without deps `pip install` a placeholder package | placeholder install skipped |

## Rationale

A `repo: local` hook's prefix is the store's local-template directory (03 §7.7): the 4.7.0 `empty_template_*` files
for every language, `setup.py` among them (`setup(name='pre-commit-placeholder-package', version='0.0.0',
py_modules=[])`). pre-commit's python install always runs `python -mpip install . *deps` in the prefix
(`python.py:214-228`), so a local python hook **without** dependencies installs that empty placeholder package — a
setuptools build in an isolated build environment (downloading setuptools when it is not cached) for nothing: the
package has no modules, no entry points and no dependencies.

The step is also fragile: every language builds in the same shared local prefix, and before 4.5.1 a `target/` left by
a local rust hook made `pip install .` fail with `Multiple top-level packages discovered in a flat-layout` (04a §1.10;
the reason `py_modules=[]` was added). And it is the only part of such an install that needs the network.

snp skips the install when the hook is local and has no `additional_dependencies` (04a Q11), with uv and with the pip
backend alike; with dependencies the uv backend installs only the dependencies and the pip backend keeps pre-commit's
`. *deps`. The environment, its interpreter, its seeded pip and the hook's activation are unchanged; the only trace is
that the distribution `pre-commit-placeholder-package` is absent from the environment, which no hook can sensibly
depend on.

## Examples

A local python hook without dependencies that lists the distributions named like the placeholder
(`tests/differential/divergences/D-PERF-02.yaml`):

pre-commit 4.7.0, exit 0 (the four install lines, then):

```
placeholder..............................................................Passed
- hook id: placeholder
- duration: 0.03s

['pre_commit_placeholder_package']
```

snp, exit 0:

```
placeholder..............................................................Passed
- hook id: placeholder
- duration: 0.01s

[]
```
