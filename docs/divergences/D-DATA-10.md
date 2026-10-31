# D-DATA-10

the line keeps its \r (DEV-PY-1)

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-10.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`snp_config::revline::rewrite`, `crates/snp-config/src/revline.rs`; vectors `D-DATA-10 freeze crlf` / `D-DATA-10 unfreeze crlf` in `crates/snp-config/tests/vectors/revline.json`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| `autoupdate --freeze` rewrites a CRLF `rev:` line with LF (mixed endings) | the line keeps its `\r` (DEV-PY-1) |

## Rationale

`REV_LINE_RE` ends with `(.*)(\r?\n)$`; because `.` matches `\r`, a CRLF line's `\r` lands in group 5 (the
comment) and group 6 is always `\n` (15 §A4.6). When `--freeze` replaces the comment with `  # frozen: <tag>`, or an
un-freeze drops a `# frozen:` comment, the `\r` goes with it: the rewritten `rev:` line ends in LF amid CRLF
lines, a mixed-ending file that editors and `git diff` flag on every autoupdate.

snp appends the `\r` back when the comment it replaced ended with one (DEV-PY-1). Lines whose comment is kept are
byte-identical to pre-commit's (the `\r` is still in the kept group 5), and LF files are unaffected.

## Examples

`    rev: v1  # note␍␊` updated with `--freeze` to `bbbb…` (tag `v2`):

pre-commit 4.6.2 writes `    rev: bbbb…  # frozen: v2␊`; snp writes `    rev: bbbb…  # frozen: v2␍␊`.
Un-freezing `    rev: bbbb…  # frozen: v1␍␊` to `v2`: pre-commit `    rev: v2␊`, snp `    rev: v2␍␊`.
