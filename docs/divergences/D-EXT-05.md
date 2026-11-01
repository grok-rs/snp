# D-EXT-05

SNP=0/false makes hook-impl skip snp's hooks (the legacy hook still runs)

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-32; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-05.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-32 (`snp_run::hook_impl`, after the legacy phase; ADR-038).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| `SNP=0`/`false` makes `hook-impl` skip snp's hooks (the legacy hook still runs) |

## Rationale

lefthook's `LEFTHOOK=0` kill switch (05 §18.4) lets a user bypass the managed hooks for one command without
`--no-verify`, which would also skip every other hook git runs. snp offers the same as `SNP=0` (or `false`, any case),
read by `hook-impl` only, after the legacy phase: the migration-mode hook (`<type>.legacy`, e.g. git-lfs's `pre-push`)
still runs and its return code is the hook's result, so `SNP=0 git push` still uploads LFS objects.

A team can forbid it with `allow_disable: false` at the root of the config; the config is read leniently here (a
missing or invalid config counts as `true`, so a broken config can never trap a user). When it is forbidden, one stderr
line `snp: SNP=0 ignored: <config> sets allow_disable: false` explains why the hooks ran anyway. `SNP_COMPAT_STRICT=1`
disables the switch (pre-commit semantics).

Why it is safe: it is opt-in per command, visible in the environment, never active unless set, overridable by the
repository, and it never skips the legacy hook.

## Examples

```text
$ SNP=0 git commit -m wip                 # hook-impl --hook-type=pre-commit: legacy hook only, rc = legacy rc
$ SNP=0 git push                          # git-lfs's pre-push.legacy still runs
$ cat .pre-commit-config.yaml
allow_disable: false
…
$ SNP=0 git commit -m wip
snp: SNP=0 ignored: .pre-commit-config.yaml sets allow_disable: false
passing..................................................................Passed
```

pre-commit ignores `SNP` and runs its hooks.

Proof: `crates/snp-run/tests/pipeline/hook_impl.rs::d_ext_05_kill_switch`; the scenario
`tests/differential/divergences/D-EXT-05.yaml`.
