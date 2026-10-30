# D-MISC-06

Regular-expression corner cases where snp's engines cannot reproduce CPython 3.12 `re` exactly (01 Addendum
§A.6.13 D1–D6). None occurs in the 4 858-pattern corpus.

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-13 (`snp_pyre`); the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-06.yaml` (WBS "Divergence files", ADR-071).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| regex corner cases (01 Addendum §A.6.13 D1–D6): IGNORECASE backreferences across `K`/`k`, `ſ`/`s`, `ς`/`σ`; code points unassigned in UCD 15.0 treated as UCD 16; `\N{…}` without the `py-re-names` feature; `(?L)` under a single-byte non-UTF-8 locale; patterns too large for the engines (`Unsupported`, reported as the validation error plus a stderr hint) | as stated (corpus frequency 0) |

## Rationale

snp parses every pattern with an exact port of CPython 3.12's `re/_parser.py` (accept/reject, message, position
and `FutureWarning`s equal CPython on every committed vector) and lowers it to explicit `regex` / `fancy-regex`
syntax. Matching is then exact except where the engines themselves differ, which the lowering cannot paper over
without a third, backtracking engine of snp's own:

* **D1 — IGNORECASE back-references.** CPython compares a back-reference under IGNORECASE character by character
  with `lower(a) == lower(b)` and without its `_EXTRA_CASES` table; fancy-regex compares simple case folds of equal
  UTF-8 length. So `(?i)(k)\1` matches `kK` (U+212A KELVIN SIGN, 3 bytes) in CPython but not in snp, and
  `(?i)(σ)\1` matches `σς` in snp but not in CPython (likewise `ſ`/`s`, `İ`, and the Latin-1 lane of bytes
  patterns for U+00C0–U+00FE). 40 of 18 382 fuzzed patterns, 0 corpus patterns; every other IGNORECASE construct is
  lowered to explicit classes and is exact (`crates/snp-pyre/tests/vectors.rs` tolerates exactly these rows).
* **D2 — Unicode version.** Python 3.12 ships UCD 15.0; `regex-syntax` ships UCD 16. Code points assigned only in
  16.0 are `\w` / case pairs in snp (as in Python 3.14) — the delta is allow-listed in `src/tests.rs`.
* **D3 — `\N{…}` without the `py-re-names` feature** (default on, ≈ 1.3 MB of names): `PyReError::Unsupported`.
* **D4 — `(?L)`** on bytes patterns follows the C/UTF-8 locale (no byte ≥ 0x80 is alphanumeric or cased); under a
  single-byte locale such as `de_DE.ISO-8859-1` CPython would follow the locale's `isalnum`.
* **D5 / D6 — engine limits.** A repeat count that unrolls past the 64 MiB `regex` size budget (`a{100000}`), or a
  backtracking (tier-2) pattern nested 64 or more groups deep, compiles in CPython but is `Unsupported` in snp,
  reported as cfgv's `… is not a valid python regex` validation error plus a stderr hint — never a crash, never a
  silent mismatch.

All six are safe: they are deterministic, need deliberately exotic patterns, and either fail loudly (D3, D5, D6) or
change the answer only for case-insensitive back-references between rare letters (D1) or for characters newer than
UCD 15.0 (D2).

## Examples

D1, the scenario of `tests/differential/divergences/D-MISC-06.yaml`: a local hook `language: fail`,
`entry: the files pattern matched`, `files: '(?i)(σ)\1'`; the repository holds the empty file `σς.txt`;
`run --all-files`.

pre-commit 4.6.2 (CPython 3.12.3: `re.search(r'(?i)(σ)\1', 'σς.txt')` is `None`; bytes captured with the same code
of pre-commit 4.1.0 on 3.12.3), exit 0:

```text
matched..............................................(no files to check)Skipped
```

snp (fancy-regex folds `ς` and `σ` to the same 2-byte `σ`, so the file matches), exit 1:

```text
matched..................................................................Failed
- hook id: matched
- exit code: 1

the files pattern matched

σς.txt

```

D5: `files: 'a{100000}'` — CPython compiles it; snp rejects the config with cfgv's
`=====> 'a{100000}' is not a valid python regex` and the hint `the pattern's repeat counts expand beyond the regular
expression engines' size budget`. D6: `files: '(((…(?=a)a…)))'` with 70 nested groups — CPython compiles it; snp
reports `the pattern nests too deeply for the backtracking engine (…)`.
