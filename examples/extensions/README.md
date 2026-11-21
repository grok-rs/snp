# Extensions and migrating from lefthook

**snp only.** [`lefthook.yml`](lefthook.yml) is a small lefthook configuration; [`.snp.yaml`](.snp.yaml) is what
`snp migrate lefthook` made of it, tidied by hand (`types:` instead of the generated glob regexes, readable names) and
extended with more of snp's extension keys. Every key is described in
[Configuration](../../docs/user/configuration.md#hook-level-extension-keys).

| lefthook | snp key in `.snp.yaml` | What it does |
|---|---|---|
| `parallel: true` | `priority` (+ root `priorities`) | Hooks with equal priority form a bucket and run concurrently; lower runs first |
| `stage_fixed: true` | `stage_fixed: true` | Re-stages the files a fixer changed, after checking that the patch applies |
| `root: web/` | `root: web` | Runs the hook in `web/` with only the files under it, relative to it |
| `env:` | `env:` | Literal environment variables for the hook |
| `skip: [merge, rebase]` | `skip: [merge, rebase]` | Skips the hook in those git states (prints `Skipped`) |
| `only: [{ref: main}]` | `only:` | Runs the hook only when a condition holds: `ref`, `env`, `ci`, `run`, git states |
| `fail_text:` | `fail_text:` | Adds a hint line to the failure block |
| lefthook's default | root `fail_on_changes: never` | A hook that modifies files does not fail for that reason |
| — | `groups:` | `snp run --group quick`, `--no-group rust`, `SNP_SKIP_GROUPS=rust` |
| — | `timeout: 10m` | Kills the hook (TERM, then KILL) and fails it after the limit |
| — | `use_stdin: true` | Passes git's stdin (the `pre-push` ref lines) to the hook |
| — | root `output: summary` | A footer with the wall time and per-hook durations |
| — | root `minimum_snp_version` | Older snp versions refuse the file with a clear message |

Two of the keys there change pre-commit semantics on purpose: `fail_on_changes: never` (pre-commit fails a hook that
modified files) and `stage_fixed` (pre-commit never stages for you). Drop them to keep pre-commit's behaviour.

## Run it

```sh
snp migrate lefthook -c examples/extensions/lefthook.yml -o -   # see the raw conversion and its report
cp examples/extensions/.snp.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install                   # installs pre-commit and pre-push (default_install_hook_types)
snp run --all-files
snp run --group quick         # only fmt and lint
```

In your own repository, `snp migrate lefthook --local --install` converts `lefthook.yml` (and `lefthook-local.yml`
into the local overlay `.snp.local.yaml`), removes lefthook's hook scripts and installs snp's; `--portable` writes a
`.pre-commit-config.yaml` with only the keys pre-commit 4.x understands. See
[Coming from prek or lefthook](../../docs/user/migration.md#coming-from-prek-or-lefthook).
