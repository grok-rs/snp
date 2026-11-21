# Fast native hooks

A minimal, language-independent config built only from
[pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) hooks that snp runs natively. For a known tag
(v3.0.0 to v6.0.0) snp uses its own Rust port of each hook instead of cloning the repository, building a virtualenv and
starting Python: the output, file changes and exit codes stay those of the Python hook, and the `Initializing` /
`Installing` lines still print when pre-commit would print them. See [The fast path](../../docs/user/configuration.md#the-fast-path).

Five ids always run the real Python hook, because they inspect Python source with CPython's own parser: `check-ast`,
`check-builtin-literals`, `check-docstring-first`, `debug-statements` and `double-quote-string-fixer`. Adding one of
them is fine: that hook alone gets a Python environment, the others stay native.

The file is a plain pre-commit config: pre-commit 4.7.0 runs it with the Python hooks.

## Run it

```sh
cp examples/fast-native/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp doctor --fast-path        # explains, per hook, whether it runs natively and why
snp install
snp run --all-files
```

`no-commit-to-branch` blocks commits on `main`; change `--branch` or drop the hook. To turn the fast path off for a
run, use `snp run --no-fast-path` or `SNP_NO_FAST_PATH=1`. With snp only, `repo: builtin` drops the `rev` and the
Python fallback altogether: see the [builtin example](../builtin/README.md).
