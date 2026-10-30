# D-MISC-06

Regular-expression corner cases where snp's engines cannot reproduce CPython 3.12 `re` exactly (01 Addendum
§A.6.13 D1–D6, plus D7–D8 found in the WP-13 review and pending a §A.6.13 / COMPATIBILITY row update by the
integration lead). None occurs in the 4 858-pattern corpus.

* **Class:** TIERB (COMPATIBILITY §5.9)
* **Owner:** WP-13 (`snp_pyre`); the first package of the register row owns this file and
  `tests/differential/divergences/D-MISC-06.yaml` (WBS "Divergence files", ADR-071).

## Register entry (COMPATIBILITY §5.9)

| pre-commit 4.6.2 | snp |
|---|---|
| regex corner cases (01 Addendum §A.6.13 D1–D6): IGNORECASE backreferences across `K`/`k`, `ſ`/`s`, `ς`/`σ`; code points unassigned in UCD 15.0 treated as UCD 16; `\N{…}` without the `py-re-names` feature; `(?L)` under a single-byte non-UTF-8 locale; patterns too large for the engines (`Unsupported`, reported as the validation error plus a stderr hint); D7: `\U` above 0x7FFFFFFF in a str pattern (`OverflowError` in CPython, `bad escape` in snp); D8: nullable loop bodies that are bounded, possessive, or `{2,}` with captures a back-reference or conditional observes | as stated (corpus frequency 0) |

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

* **D7 — `\U` past a C int.** In a str pattern `\U80000000` … `\UFFFFFFFF` (also inside a set) makes `chr()` in
  `re/_parser.py` raise `OverflowError: Python int too large to convert to C int` instead of the `re.error`
  `bad escape \U…` of the values 0x110000–0x7FFFFFFF. snp reports the `bad escape` (position included):
  `snp_pyre::PyReError::Overflow` carries the fixed repetition text, and a message-carrying variant would change
  the frozen ARCH §1.4.5 contract. Both are rejections; only pygrep's last traceback line differs (and the class
  pre-commit would crash with on a config pattern, D-CRASH-04). Bytes patterns have no `\U` escape and are exact.
  The edge vectors carry the three shapes; `tests/vectors.rs` tolerates exactly this class.
* **D8 — empty iterations snp cannot end like `MAX_UNTIL`.** CPython leaves a loop after an iteration that matched
  the empty string, with that iteration's priority (`_sre` `last_ptr`). snp runs unbounded greedy loops over a
  nullable body on fancy-regex's epsilon-checked VM loop (and `X{m,}` as `X{m}` + such a loop), which is exact
  (`(?:x*|y)+` on `xyx` is `(0, 1)`, as `pygrep --multiline` needs). Left: bounded `X{m,n}` and possessive loops over
  a nullable body, and `X{m,}` (m ≥ 2) when a back-reference or conditional can observe X's captures. There the match
  *end* may differ (never whether `re.search` finds a match), and a conditional on a group of the loop can see an
  iteration CPython never runs: `(?x)((a)?(?<!\n)(?(1)[a-z]|(?:i|^))){0,2}` fullmatches `ab` in snp, not in CPython
  (this is the "conditional re-entry" class of `gen_re_vectors.py`, about one random pattern in 4 000). The same
  span difference remains for a tier-1 pattern nested 63 or more groups deep, whose exact form fancy-regex cannot
  build.

All eight are safe: they are deterministic, need deliberately exotic patterns, and either fail loudly (D3, D5, D6, D7: a
rejected pattern stays rejected) or change the answer only for case-insensitive back-references between rare
letters (D1), for characters newer than UCD 15.0 (D2), or for empty iterations of exotic nullable loops (D8).

Fixed in the WP-13 review (exact now, with regression tests in `crates/snp-pyre/tests/acceptance.rs` and edge
vectors): spans of unbounded nullable loops; fancy-regex 0.19's tree rewrites of lazy bodies (`(X+?)*` → `(X+?)?`,
`a*b??a*` → `a*(?:ba*)?`); the search-only charset prefix of a leading scoped `(?a:…)` set (`re.search(r'(?a:\W)',
'é')` is `None` while `re.match` matches); and pygrep's own recursion depth for bytes patterns (494 nested groups
compile, 495 raise `RecursionError`, against 484/485 for config patterns).

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
