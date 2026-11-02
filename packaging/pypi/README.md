# snp-cli

**snp** ("You Shell Not Pass") is a drop-in replacement for [pre-commit](https://pre-commit.com) 4.7.0, written in
Rust: the same `.pre-commit-config.yaml`, the same commands, options, output and exit codes, with optional
lefthook-inspired extensions and native fast paths for the most popular hooks.

This distribution contains the `snp` executable only:

```sh
pipx install snp-cli
uv tool install snp-cli
uvx --from snp-cli snp run --all-files
```

```console
$ snp --version
snp 2.0.1
```

Notes:

* The PyPI project `snp` is an unrelated placeholder: `pip install snp` and `uvx snp` do not install this tool.
* The wheel does not install a `pre-commit` command and does not replace the `pre-commit` package. To make
  `pre-commit` run snp, use `snp self alias install`.
* snp does not use pre-commit's cache; it keeps its own in `~/.cache/snp` (or `$SNP_HOME`).

Documentation: <https://github.com/grok-rs/snp/tree/main/docs/user> — installation, migrating from pre-commit, CI
integration, configuration, troubleshooting.
