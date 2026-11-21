# Example configurations

Each directory holds a config to copy into a repository, with a README that says what it shows and how to run it.
Every config is checked by `snp validate-config --strict` in the test suite (`examples` in `crates/snp/tests/it`),
and every remote hook is pinned to a released tag.

| Example | Shows | pre-commit can read it |
|---|---|---|
| [python](python/README.md) | ruff (lint + format), mypy, pre-commit-hooks | yes |
| [rust](rust/README.md) | `cargo fmt` and `cargo clippy` as local hooks, typos | yes |
| [javascript-typescript](javascript-typescript/README.md) | prettier, ESLint and `tsc` from `node_modules` | yes |
| [monorepo](monorepo/README.md) | per-directory `files:` patterns, a top-level `exclude:` | yes |
| [fast-native](fast-native/README.md) | only pre-commit-hooks hooks that snp runs natively | yes |
| [stages](stages/README.md) | `pre-commit`, `commit-msg`, `pre-push` and `manual` stages | yes |
| [ci-github-actions](ci-github-actions/README.md) | a GitHub Actions workflow, with and without `snp-action` | yes |
| [builtin](builtin/README.md) | `repo: builtin`: hooks compiled into snp, no `rev`, no clone | no (`.snp.yaml`) |
| [extensions](extensions/README.md) | snp's extension keys, converted from a `lefthook.yml` | warns (`.snp.yaml`) |

"yes" means the file uses no snp extension: pre-commit 4.7.0 and snp run it the same way, so a team can switch tool by
tool. The last two are named `.snp.yaml`, which snp reads before `.pre-commit-config.yaml` and pre-commit never reads.

Copy a config to your repository root, check it, then install and run:

```sh
cp examples/python/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp validate-config .pre-commit-config.yaml   # nothing printed, exit 0: the file is valid
snp install --install-hooks
snp run --all-files
```

The keys themselves are documented in [Configuration](../docs/user/configuration.md); pre-commit's own keys in
[pre-commit's documentation](https://pre-commit.com/#plugins).
