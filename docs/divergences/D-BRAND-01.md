# D-BRAND-01

snp brand: snp 2.0.0, the §3.3 substitutions and argparse texts rendered with prog snp; compat brand: byte-identical

* **Class:** BRAND (COMPATIBILITY §5.4)
* **Owner:** WP-12 (contributing: all packages emitting branded texts); the first package of the register row owns this file and
  `tests/differential/divergences/D-BRAND-01.yaml` (WBS "Divergence files", ADR-071).
* **Status:** implemented for the CLI surface by WP-12 (`snp_cli::parse`, `SNP_SPEC`); the non-argparse texts of the
  `brand_map` are emitted by the packages that own them.

## Register entry (COMPATIBILITY §5.4)

| pre-commit 4.6.2 | snp |
|---|---|
| `--version` and texts | snp brand: `snp 2.0.0`, the §3.3 substitutions and argparse texts rendered with prog `snp`; compat brand: byte-identical |

## Rationale

FD-1 fixes the product identity: invoked under any name but `pre-commit`, the tool is snp and says so. Under the
`pre-commit` name (a symlink, hard link or copy; COMPATIBILITY §3.1) every byte stays pre-commit 4.6.2's, so scripts,
CI greps and `pre-commit --version` parsers keep working (ADR-023).

In the snp brand the argparse texts are **rendered**, not substituted (ADR-058): argparse wraps usage, help and usage
errors relative to `usage: {prog} {cmd} `, so the 7-character-shorter prog `snp` moves wrap points and continuation
indents (`init-templatedir`: 35 → 28 columns). `snp_argparse` renders them from `SNP_SPEC` with prog `snp`; the goldens
(`crates/snp-cli/tests/goldens/snp.json`, `cargo xtask cli-goldens`) are produced by a real CPython 3.13.14 argparse
built from `export_json(SNP_SPEC, "snp")` and, for every parser identical to pre-commit's, cross-checked against the
prog-patched oracle `target/oracle/pc462-py313/bin/pre-commit-snp-prog`. The help strings of `main.py` that name the
tool (`main.py:222, 245, 273, 290, 299, 335`) read `snp` in the snp brand (COMPATIBILITY §3.3).

`--version` is the one argparse text that is not a pure prog substitution: `%(prog)s 4.6.2` would print `snp 4.6.2`,
which names a version of snp that does not exist; the snp brand's version action is `%(prog)s 2.0.0`.

Why it is safe: the divergence is confined to the snp brand, which pre-commit users only reach by typing `snp`; the
compat brand reproduces pre-commit 4.6.2 byte for byte (`crates/snp-cli/tests/goldens/compat.json`, 490 cases at
the 12 §A.16 width matrix, all generated from pre-commit 4.6.2 on CPython 3.13.14). Exit codes and streams are the
same in both brands.

## Examples

`--version` (stdout, exit 0):

```text
$ pre-commit --version          # pre-commit 4.6.2, and snp through a `pre-commit` link
pre-commit 4.6.2
$ snp --version
snp 2.0.0
```

`init-templatedir -h` (first lines; the continuation indent follows the prog):

```text
$ pre-commit init-templatedir -h
usage: pre-commit init-templatedir [-h] [--color {auto,always,never}]
                                   [-c CONFIG] [--no-allow-missing-config]
$ snp init-templatedir -h
usage: snp init-templatedir [-h] [--color {auto,always,never}] [-c CONFIG]
                            [--no-allow-missing-config]
```

Usage errors name the prog of the reporting parser:

```text
$ pre-commit sample-config --color bogus
usage: pre-commit sample-config [-h] [--color {auto,always,never}]
pre-commit sample-config: error: argument --color: invalid use_color value: 'bogus'
$ snp sample-config --color bogus
usage: snp sample-config [-h] [--color {auto,always,never}]
snp sample-config: error: argument --color: invalid use_color value: 'bogus'
```

Help strings that name the tool (`snp -h`, commands block):

```text
    clean               Clean out snp files.
    install             Install the snp script.
```

The scenario `tests/differential/divergences/D-BRAND-01.yaml` runs these commands in the snp brand against the
unpatched oracle, where they must keep differing.
