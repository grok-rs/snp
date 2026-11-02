# D-EXT-03

local-overlay notices on stderr: snp: using local overrides from {path}, the ignored-overlay note (tracked by git / CI), snp: local config overrides unknown…

* **Class:** EXT (COMPATIBILITY §5.7)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-EXT-03.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (texts in `crates/snp-config/src/messages.rs`: `overlay_active`, `overlay_tracked`, `overlay_ci`, `overlay_missing`, `overlay_unknown_hook`; emitted through `DiagSink` by `overlay::apply_root` and returned by `overlay::locate` / `Overlay::unmatched` for the caller to print).

## Register entry (COMPATIBILITY §5.7)

| Divergence |
|---|
| local-overlay notices on stderr: `snp: using local overrides from {path}`, the ignored-overlay note (tracked by git / CI), `snp: local config overrides unknown hook '<id>'` |

## Rationale

A per-developer overlay silently changing what a commit runs would make "works on my machine" bugs invisible,
so every effect of the overlay is announced — on stderr only, so stdout (hook lines, warnings, CI greps) stays
pre-commit's. The lines exist only when an overlay file exists next to the config or `SNP_LOCAL_CONFIG` names one;
a repository without an overlay prints nothing new.

## Examples

* active: `snp: using local overrides from .pre-commit-config.local.yaml`
* tracked by git: `snp: ignoring local config .pre-commit-config.local.yaml: it is tracked by git (local overrides
  are per developer; untrack it or name it in SNP_LOCAL_CONFIG)`
* CI: `snp: ignoring local config .pre-commit-config.local.yaml: CI is set (name it in SNP_LOCAL_CONFIG to use it in
  CI)`
* `SNP_LOCAL_CONFIG` names a missing file: `snp: ignoring local config <path>: SNP_LOCAL_CONFIG names a file that does
  not exist`
* an overlay hook matching no hook: `snp: local config overrides unknown hook 'nonexistent'`

pre-commit 4.7.0 prints none of these (it never reads the overlay).
