# Polyglot monorepo

One config for a repository that holds a Python service, a Rust service, a TypeScript front end, shell scripts and
GitHub Actions workflows. Each language's hooks see only their own directory:

* `files:` is a Python regular expression matched (with `re.search`) against each file's path relative to the
  repository root; `^services/api/` keeps ruff inside the Python service. Combined with `types`/`types_or`, a file must
  match both;
* the top-level `exclude:` removes vendored and generated directories from every hook;
* the pre-commit-hooks checks have no `files:` and run on the whole repository;
* `repo: local` hooks run in the repository root and receive root-relative paths, so `cargo fmt` gets
  `--manifest-path` and ESLint gets `--config`.

The file uses no snp extension, so teammates who still run pre-commit 4.7.0 can share it unchanged. With snp only, the
`root:` extension runs a hook inside its directory and passes paths relative to it, and `groups:` lets you run one
area's hooks (`snp run --group web`): see the [extensions example](../extensions/README.md).

## Run it

```sh
cp examples/monorepo/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install --install-hooks
snp run --all-files
snp run ruff-check --files services/api/app.py   # one hook, chosen files
```

Adjust the directory regexes to your layout. `snp run --all-files --verbose` prints, for every hook, how long it took;
a hook whose `files:` matches nothing prints `(no files to check)Skipped`. The
[`check-useless-excludes`](https://pre-commit.com/#meta-check-useless-excludes) meta hook reports `exclude:` patterns
that match no file.
