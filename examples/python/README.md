# Python project

A config for a typical Python repository:

* generic checks from [pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) v6.0.0. snp runs all of them
  natively except `debug-statements`, which needs CPython's parser and runs as the real Python hook (see
  [The fast path](../../docs/user/configuration.md#the-fast-path));
* [ruff](https://github.com/astral-sh/ruff-pre-commit) to lint (with `--fix`) and format, with
  [black](https://github.com/psf/black-pre-commit-mirror) as a commented-out alternative to `ruff-format`;
* [mypy](https://github.com/pre-commit/mirrors-mypy) with a stub package in `additional_dependencies`.

The file uses no snp extension, so teammates who still run pre-commit 4.7.0 can share it unchanged.

## Run it

```sh
cp examples/python/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install --install-hooks   # hook script + every hook environment
snp run --all-files
```

The first run builds Python environments for ruff and mypy, as pre-commit does; `python3` must be on `PATH`. Update
the pinned `rev`s with `snp autoupdate`.
