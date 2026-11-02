# D-EXT-02

the local overlay .pre-commit-config.local.yaml / SNP_LOCAL_CONFIG

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-02.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::overlay::{locate, load, apply_root, HookPatch}`, `merge_hook`/`finalize_local` patches, `crates/snp-config/src/overlay.rs`; acceptance tests `overlay_discovery_rules`, `overlay_patches_merge_after_manifest_and_config`). The CLI flag `--no-local-config` and the resolver plumbing are WP-31/WP-33's.

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| the local overlay `.pre-commit-config.local.yaml` / `SNP_LOCAL_CONFIG` |

## Rationale

lefthook's `lefthook-local.yml` (05 §18.4 A3) gives every developer a git-ignored file for personal tweaks —
skip a slow hook, wrap an entry in `docker compose run`, set an environment variable — without editing the shared
config; pre-commit's only escape hatch is `SKIP=`. snp reads `<config dir>/.pre-commit-config.local.yaml` (or
`.yml`, or the file named by `SNP_LOCAL_CONFIG`) and applies it on top of the config: root items (`skip_groups`,
`fail_on_changes`, appended `repos`) to the `Config`, hook entries keyed by alias or id after the manifest ∪
config merge, so `{entry}` expands against the merged entry of remote hooks too and the patched hook is what the
fast path and the install key see (ARCHITECTURE §8.4).

pre-commit never reads the file. The overlay is ignored when it is tracked by git (it is per developer by
definition) or when `CI` is truthy unless named explicitly, and `SNP_COMPAT_STRICT=1` disables it, so shared and CI
runs behave exactly like the config alone. Its use is always announced on stderr (D-EXT-03).

## Examples

`.pre-commit-config.local.yaml` (untracked) next to a config with a local `pytest` hook:

```yaml
hooks:
  pytest:
    entry: docker compose run --rm app {entry}
    args: [-x]
```

pre-commit 4.7.0 runs `pytest -q …` as configured. snp prints `snp: using local overrides from
.pre-commit-config.local.yaml` on stderr and runs `docker compose run --rm app pytest -q -x …`.
