# Rust project

A config for a Rust crate or workspace:

* `cargo fmt` and `cargo clippy` as `repo: local` hooks with `language: system`, so they use the toolchain the project
  already pins (`rust-toolchain.toml`) and snp installs nothing for them. Both take `pass_filenames: false`, because
  cargo works on the whole workspace;
* [typos](https://github.com/crate-ci/typos) for spelling;
* generic checks from [pre-commit-hooks](https://github.com/pre-commit/pre-commit-hooks) v6.0.0, which snp runs
  natively.

`cargo fmt --all` rewrites files and the hook then fails with `files were modified by this hook`, as under pre-commit:
review the change, `git add` it and commit again. With snp only, the `stage_fixed` extension re-stages the fix for you
(see the [extensions example](../extensions/README.md)).

## Run it

```sh
cp examples/rust/.pre-commit-config.yaml /path/to/your/repo/
cd /path/to/your/repo
snp install --install-hooks
snp run --all-files
```

Clippy on every commit can be slow on a large workspace; move it to `pre-push` as shown in the
[stages example](../stages/README.md).
