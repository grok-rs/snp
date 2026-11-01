# snp-action

Run [snp](https://github.com/grok-rs/snp) — a drop-in replacement for pre-commit 4.6.2 — in GitHub Actions. It
replaces [`pre-commit/action`](https://github.com/pre-commit/action) with a one-line change:

```yaml
- uses: actions/checkout@v4
- uses: grok-rs/snp-action@v1
```

| Input | Default | Meaning |
|---|---|---|
| `extra_args` | `--all-files` | options passed to `snp run` (same name and default as `pre-commit/action`) |
| `version` | `latest` | the snp release, e.g. `2.0.0` |
| `cache` | `true` | cache `~/.cache/snp` between runs |

The action needs no Python. It downloads the release archive for the runner (Linux, macOS, Windows; x86_64 and arm64),
verifies its SHA-256 checksum, adds `snp` to `PATH` (output `snp-path`), restores and saves snp's cache under the key
`snp-1|<os>|<arch>|<pythonLocation>|<hash of .pre-commit-config.yaml>`, and runs
`snp run --show-diff-on-failure --color=always <extra_args>`.

The cache key deliberately differs from `pre-commit/action`'s: GitHub caches are immutable, so a key that pre-commit
already saved would be restored on every run and snp's cache would never be saved. If you set `SNP_HOME`, disable the
built-in cache (`cache: "false"`) and cache that directory yourself.

Output and exit codes are those of `pre-commit run`; only the tool's name differs in its own messages (for example
``reproduce locally with: `snp run --all-files`.``).

This directory is the source of the `grok-rs/snp-action` repository; it lives in the snp repository so that it is
reviewed and versioned with snp. See the [CI integration guide](https://github.com/grok-rs/snp/blob/main/docs/user/ci.md).
