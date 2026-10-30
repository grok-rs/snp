# D-DATA-16

end of file treated as a line end; comments kept

* **Class:** DATA (COMPATIBILITY §5.2)
* **Owner:** WP-21; the first package of the register row owns this file and
  `tests/differential/divergences/D-DATA-16.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented by WP-21 (`revline::lines` treats the end of the file as a line end, `crates/snp-config/src/revline.rs`; vector `D-DATA-16 no final newline`, unit test `missing_final_newline_is_a_line_end`).

## Register entry (COMPATIBILITY §5.2)

| pre-commit 4.6.2 | snp |
|---|---|
| autoupdate: a config without a final newline triggers a full PyYAML re-dump that drops all comments | end of file treated as a line end; comments kept |

## Rationale

`REV_LINE_RE` requires a newline, so a `rev:` on the last line of a file without a final newline never matches;
the rev-line count then differs from the repo count and autoupdate re-dumps the whole file through PyYAML
(`yaml_dump(yaml_load(original))`), dropping every comment and reformatting every line (03 §11.3; observed with a
`# keep me` header). Editors that omit the final newline are common.

snp matches the unterminated last line as if it ended with `\n` and writes it back without one. Every other file
gets pre-commit's rewrite; the re-dump fallback stays for configs whose rev lines really cannot be located
(flow style, `rev:` at column 0, extra `rev:` keys).

## Examples

```
# keep me
repos:
-   repo: a
    hooks: []
    rev: v1  # c        ← no final newline
```

pre-commit 4.6.2 `autoupdate` (to `v2`) writes `repos:\n-   repo: a\n    hooks: []\n    rev: v2\n` — the header and
the comment are gone. snp writes the file unchanged except `    rev: v2  # c` (still no final newline).
