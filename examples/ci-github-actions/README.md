# CI with GitHub Actions

[`snp.yml`](snp.yml) is a workflow that runs every hook of [`.pre-commit-config.yaml`](.pre-commit-config.yaml) on
pushes to `main` and on pull requests, in two equivalent ways:

* **`grok-rs/snp-action@v1`**, the one-line replacement of `pre-commit/action`: `extra_args` has the same name and
  default (`--all-files`). The action downloads the release archive for the runner, verifies its SHA-256 checksum,
  caches snp's cache directory and runs `snp run --show-diff-on-failure --color=always ${extra_args}`;
* **without the action**: the release installer, `actions/cache` and the same `snp run` command.

Whichever you use, **change the cache key** when you switch from pre-commit. snp's cache is `~/.cache/snp`, not
`~/.cache/pre-commit`, and with GitHub's immutable caches a key that pre-commit already saved is restored on a hit and
never saved again: snp's cache would never persist. The `snp-1|…` key in the workflow is new to every pre-commit setup.
Do not put the snp version in the key; snp versions its cache internally.

The config also runs `check-github-workflows`, so a broken workflow file fails before it reaches GitHub.

## Use it

```sh
mkdir -p .github/workflows
cp examples/ci-github-actions/snp.yml .github/workflows/snp.yml
cp examples/ci-github-actions/.pre-commit-config.yaml .     # or keep your own config
snp run --all-files                                          # what CI will run, locally
```

Keep one of the two jobs. GitLab CI, Azure Pipelines, CircleCI, tox/nox and pre-commit.ci are covered in
[CI integration](../../docs/user/ci.md).
