# `repo: builtin`

**snp only.** `repo: builtin` names the hooks of pre-commit-hooks v6.0.0 that are compiled into snp: same ids, same
defaults and arguments, always native, without a `rev` and without a clone. See
[`repo: builtin`](../../docs/user/configuration.md#repo-builtin).

* `entry` and `additional_dependencies` are not allowed on a builtin hook, and `language` must be absent or `system`.
* Seven pre-commit-hooks ids have no builtin because they need CPython's parser or were removed in v6.0.0
  (`check-ast`, `check-builtin-literals`, `check-byte-order-marker`, `check-docstring-first`, `debug-statements`,
  `double-quote-string-fixer`, `fix-encoding-pragma`): keep them in an upstream block with a `rev`, as `check-ast` is
  here.
* pre-commit rejects `repo: builtin`, so the file is named `.snp.yaml`: snp looks for `.snp.yaml`, `.snp.yml`, then
  `.pre-commit-config.yaml` ([Config file name](../../docs/user/configuration.md#config-file-name)), and Python
  pre-commit never reads it. Use it only in repositories where everyone runs snp.

## Run it

```sh
cp examples/builtin/.snp.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install                   # the hook script names --config=.snp.yaml
snp run --all-files
```

An existing pre-commit config converts both ways in place: `snp migrate builtin` rewrites pre-commit-hooks blocks to
`repo: builtin` (blocks that use one of the seven ids above stay upstream, with a note), and `snp migrate upstream`
goes back.
