# Stages: pre-commit, commit-msg, pre-push, manual

One config that acts on several git hooks:

| Stage | Hooks | When it runs |
|---|---|---|
| `pre-commit` | whitespace fixers, `check-merge-conflict`, `no-commit-to-branch` | `git commit`, on the staged files |
| `commit-msg` | [conventional-pre-commit](https://github.com/compilerla/conventional-pre-commit) | `git commit`, on the message file |
| `pre-push` | `unit-tests` (`make test`) | `git push`, before anything is sent |
| `manual` | `integration-tests` (`make integration`) | only `snp run --hook-stage manual` |

* `default_install_hook_types` makes a plain `snp install` install all three git hook scripts (`snp install -t
  pre-push` installs one).
* `default_stages: [pre-commit]` keeps hooks without `stages:` off the other stages. Without it they would also run
  on `commit-msg` and `pre-push`.
* The `conventional-pre-commit` manifest already sets `stages: [commit-msg]`.

These are pre-commit's own keys and stages; the file uses no snp extension.

## Run it

```sh
cp examples/stages/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install --install-hooks                     # pre-commit, commit-msg and pre-push scripts
git commit -m "update things"                   # rejected by conventional-pre-commit
git commit -m "fix: handle empty input"         # accepted
snp run --hook-stage pre-push --all-files       # the push checks, without pushing
snp run --hook-stage manual integration-tests   # the on-demand hook
```

With snp only, `use_stdin: true` gives a `pre-push` hook git's ref lines, and `skip:`/`only:` conditions restrict a
hook to some branches: see the [extensions example](../extensions/README.md).
