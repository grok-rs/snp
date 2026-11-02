# D-DIAG-01

git < 2.31: `[WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.` (once per process; 02 G.4)

* **Class:** DIAG (COMPATIBILITY §5.8)
* **Owner:** WP-18; the first package of the register row owns this file and
  `tests/differential/divergences/D-DIAG-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-18 (`snp_git::GitExe::version_gate`, `crates/snp-git/src/exe.rs`; ADR-063).

## Register entry (COMPATIBILITY §5.8)

| ID | Diagnostic (stderr only) | WP |
|---|---|---|
| D-DIAG-01 | git < 2.31: `[WARNING] git <V> is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.` (once per process; 02 G.4) | 18 |

## Rationale

pre-commit 4.7.0 has no git version check, yet needs git ≥ 2.31 for `ls-files --deduplicate` and ≥ 2.19 for
intent-to-add detection in the stash (02 G.0 item 1). snp never refuses a git (ADR-063: below 2.31 it falls back —
D-CRASH-26 — and only a stash that would need intent-to-add detection is refused below 2.19, D-DATA-17), but a user on
a warning-band git runs combinations snp's CI tests only as smoke tests (Debian 11's 2.30.2, Ubuntu 20.04's 2.25.1).
One stderr line per process says so and how to silence it; stdout and the exit code stay byte-identical to
pre-commit, so the line is a DIAG divergence (always on, stderr only).

The version comes from `git --version` (`git version 2.30.2` → `2.30.2`; suffixes such as `(Apple Git-154)`,
`.windows.2`, `.rc0` ignored; an unparseable version is treated as the newest and never warns). `[WARNING]` is
coloured like pre-commit's log lines when colour is on (`\x1b[43;30m[WARNING]\x1b[m`). The text names snp in both
brands. `SNP_ALLOW_OLD_GIT` set to any non-empty value silences it.

## Examples

`pre-commit run` (compat alias) on Debian 11 (git 2.30.2), a staged file and a passing hook.

pre-commit 4.7.0: stdout `x........Passed`, no stderr, exit 0.

snp: the same stdout bytes and exit 0; stderr:

```
[WARNING] git 2.30.2 is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.
```

With `SNP_ALLOW_OLD_GIT=1`: no stderr — byte-identical to pre-commit.

Regression tests: `crates/snp-git/tests/behaviour/exe.rs` `version_gate_never_refuses_and_warns_once` (exactly one
line on stderr for repeated calls, none with `SNP_ALLOW_OLD_GIT=1`, none for ≥ 2.31 or an unparseable version,
nothing on stdout), `messages::tests::old_git_warning_both_brands_and_colours`, and the version-parser table
`exe::tests::version_parser_table`. The scenario runs on the CI git 2.30 container.
