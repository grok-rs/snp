# 08 — Evaluating the user's crates `cfgv` (cfgv-rs 0.2.0) and `file-identify` 0.4.0 as snp dependencies

Status: research spec for snp implementers. All paths are relative to `scratchpad/refs/` unless stated.
Everything marked **[verified]** was reproduced with a probe binary
(`scratchpad/tmp-user-crates/probe/src/main.rs`), `strace`, cross-compilation, or by running the reference
`pre-commit 4.1.0` (`~/.local/bin/pre-commit validate-config`) in throwaway repos
(`scratchpad/tmp-user-crates/pc-probe/`). Table diffs were produced by
`scratchpad/scripts/diff_tables.py` (parses the Python tables with `ast`, never imports them); raw output in
`scratchpad/tables_diff_latest.txt`.

---

## 0. TL;DR verdict

| | `cfgv` 0.2.0 (cfgv-rs) | `file-identify` 0.4.0 |
|---|---|---|
| Published == git HEAD? | yes (`.cargo_vcs_info.json` sha `99044e8`, src byte-identical) | yes (sha `c2faedb`, src byte-identical) |
| Python original ported | cfgv **3.5.0** (latest; has `KeyValueMap`) | identify ~2.6.1x era tables, **not** 2.6.20 (latest, 2026-09-26) |
| `unsafe` in crate | none (but no `#![forbid(unsafe_code)]`) | none (no `forbid` either) |
| Panics on bad input | none found (all indexing guarded) | none found in lib (pub `Index` on `ShebangTuple` panics on OOB by design) |
| Usable for snp as-is? | **No** — blocker gaps: no custom map validators, `check_regex` hard-wired to Rust `regex` (rejects valid Python regexes), several `repr`-based message mismatches, no `dict`/`list` type checks | **No** — tables drifted (31 tags missing from `ALL_TAGS` ⇒ valid configs rejected; 18 invented names ⇒ hooks run on wrong files), shebang parsing diverges (no shlex/nix-shell), wrong Windows exec semantics, perf/memory bugs |
| Verdict | Keep as the validation engine **after** patch set C-P0 (§8.1). Do **not** use its `yaml_serde` feature. | Keep as the identification engine **after** patch set F-P0 (§8.2); redesign `TagSet` as a bitset and table generation from upstream. |
| MSRV claim | `rust-version = "1.85"` is **false** — fails on 1.85 (let-chains need 1.88) **[verified]** | `1.94.0` (fine for snp's 1.98) |

Top-level rule for snp: both crates are the user's own and trusted; snp should depend on new releases
(`cfgv 0.3`, `file-identify 0.5`) produced from the patch plan below, using `[patch.crates-io]` git deps during
development.

---

## 1. Provenance & versions

| Item | Value |
|---|---|
| cfgv-rs crate | `cfgv` 0.2.0, published 2026-04-05, 60 downloads, repo github.com/grok-rs/cfgv-rs |
| file-identify crate | `file-identify` 0.4.0, published 2026-04-05, ~37.7k downloads, repo github.com/grok-rs/file-identify |
| Python cfgv | 3.5.0 (refs clone == PyPI latest). pre-commit 4.1.0's venv ships cfgv **3.4.0** (lacks `KeyValueMap`, otherwise identical) |
| Python identify | 2.6.20 (refs clone byte-identical to the PyPI 2.6.20 wheel for `identify.py`, `extensions.py`, `interpreters.py`) **[verified]**. pre-commit 4.1.0's venv ships 2.6.7 |
| pre-commit dependency spec | `cfgv>=2.0.0`, `identify>=1.0.0`, `pyyaml>=5.1` (`pre-commit/setup.cfg:20-25`) ⇒ real users get the *latest* identify; snp must track latest identify tables |
| yaml_serde | 0.10.7 (2026-08-18), "serde_yaml maintained by The YAML Organization", repo github.com/yaml/yaml-serde, publisher `ingydotnet`; fork of dtolnay's deprecated `serde_yaml 0.9.34+deprecated`; depends on `libyaml-rs ^0.3` = "libyaml transpiled to rust by c2rust" (fork of `unsafe-libyaml`). `libyaml-rs` 0.3.0 has 237 `unsafe` occurrences in ~11.9k lines; `yaml_serde` itself 60 `unsafe` occurrences **[verified, grep of ~/.cargo/registry]** |

---

## 2. `cfgv` (cfgv-rs 0.2.0)

### 2.1 Crate metadata

| Key | Value | Notes |
|---|---|---|
| edition | 2024 | |
| rust-version | "1.85" | **Wrong.** `src/validator.rs:379-381,448-450` use let-chains (`if … && let Some(v) = …`), stable since 1.88. `cargo +1.85.0 check` ⇒ `error[E0658]: let expressions in this position are unstable` at `src/validator.rs:380:24` and `:449:24` **[verified]** |
| CI MSRV job | matrix `[stable, "1.85"]` passes | False green: `rust-toolchain.toml` pins `channel = "stable"`; `dtolnay/rust-toolchain` only runs `rustup default <tc>` (action.yml:112-113) and the toolchain file overrides the default, so the "1.85" job compiles with stable |
| deps | `indexmap 2`, `regex 1`; optional `serde_json 1`, `yaml_serde 0.10` | `regex` is always compiled even if the embedder supplies its own regex check |
| features | `default = []`, `serde_json`, `yaml_serde` | each adds `From<…::Value> for Value` |
| unsafe | none; no `#![forbid(unsafe_code)]` | |
| lints | clippy clean with defaults; 55 `clippy::pedantic` warnings (28 `must_use`, 7 redundant closures, 5 missing `# Errors`, …) **[verified]** | |
| Send/Sync | `Value`, `Schema`, `Validator`, `CheckFn`, `ValidationError` are all `Send + Sync` (custom fns are `Arc<dyn Fn + Send + Sync>`) **[verified by static assert]** | |
| sizes (x86_64) | `Value` 72 B, `ValidationError` 32 B, `Schema` 120 B, `Validator` **352 B**, `CheckFn` 32 B **[verified]** | |
| tests | 77 integration tests (`tests/integration.rs`) + 2 doctests, all pass on 1.98 **[verified]**. Mirrors Python's 77 test fns except `test_load_from_filename_unicode_error` (the one that diverges) | |
| README | still says `cfgv = "0.1"` | |

### 2.2 Public API (complete)

```text
pub enum Value { Bool(bool), Int(i64), Float(f64), String(String), Bytes(Vec<u8>),
                 List(Vec<Value>), Map(IndexMap<Value, Value>), Null }
  fn type_name(&self) -> &'static str        // "bool","int","float","str","bytes","list","dict","NoneType"
  fn as_bool/as_int/as_str/as_list/as_map, into_map, into_list
  fn repr(&self) -> String                   // "Python repr()-style" (see §2.4 for divergences)
  impl Display (Python str()-style), PartialEq/Eq (floats by bits), Hash, Clone, Debug
  impl From<bool|i64|i32|f64|String|&str|Vec<u8>|Vec<Value>|IndexMap<Value,Value>>
  #[cfg(feature="serde_json")] impl From<serde_json::Value>
  #[cfg(feature="yaml_serde")] impl From<yaml_serde::Value>
macro value_map! { k => v, ... }             // uses hidden re-export cfgv::__IndexMap

pub struct ValidationError { message: String, ctx: Option<Box<ValidationError>> }
  fn new(impl Into<String>) -> Self; fn message(&self) -> &str
  fn with_context(self, ctx: impl Into<String>) -> Self   // outer node = ctx, inner = self
  fn trace_parts(&self) -> Vec<&str>          // [ctx..., leaf]
  impl Display  ("\n" + "==> {ctx}\n"* + "=====> {leaf}"); impl Error (source() = inner)
pub fn validate_context<T,F,C>(ctx: C, f: F) -> Result<T, ValidationError>
  where C: FnOnce() -> String, F: FnOnce() -> Result<T, ValidationError>   // lazy ctx, no alloc on success

pub enum ValueType { Bool, Int, Float, String, Bytes }      // NO List / Map / Null
pub type CustomCheckFn = Arc<dyn Fn(&Value) -> Result<(), ValidationError> + Send + Sync>;
pub enum CheckFn { Any, Type(ValueType, Option<&'static str>), OneOf(Vec<Value>), Regex,
                   Array(Box<CheckFn>), And(Vec<CheckFn>), Custom(CustomCheckFn) }
  fn check(&self, &Value) -> Result<(), ValidationError>
  ctors: check_bool, check_int, check_string ("string"), check_text ("text"), check_bytes,
         check_type(vt, name), check_one_of(Vec<Value>), check_regex, check_array(inner),
         check_and(Vec), custom(F)

pub enum ConditionValue { Exact(Value), Not(Value), In(Vec<Value>), NotIn(Vec<Value>) }
  fn matches(&self, Option<&Value>) -> bool   // None == MISSING; Not/In/NotIn never match MISSING
  fn describe_opposite(&self) -> String

pub type WarnCallback = Arc<dyn Fn(&[Value], &[Value], &IndexMap<Value,Value>) + Send + Sync>;
pub enum Validator {                          // CLOSED enum — no user-defined map validators
  Required{key,check_fn}, RequiredRecurse{key,schema}, Optional{key,check_fn,default},
  OptionalRecurse{key,schema,default}, OptionalNoDefault{key,check_fn},
  Conditional{key,check_fn,condition_key,condition_value,ensure_absent},
  ConditionalOptional{key,check_fn,default,condition_key,condition_value,ensure_absent},
  ConditionalRecurse{key,schema,condition_key,condition_value,ensure_absent},
  NoAdditionalKeys{keys}, WarnAdditionalKeys{keys,callback} }
  fn check(&self, &IndexMap<Value,Value>) -> Result<(), ValidationError>
  fn apply_default(&self, &mut IndexMap<..>); fn remove_default(&self, &mut IndexMap<..>)
  ctors: required, required_recurse, optional, optional_recurse, optional_no_default,
         conditional, conditional_with_absent, conditional_optional, conditional_optional_with_absent,
         conditional_recurse, conditional_recurse_with_absent, no_additional_keys, warn_additional_keys

pub enum Schema { Map{object_name: String, id_key: Option<Value>, items: Vec<Validator>},
                  Array{of: Box<Schema>, allow_empty: bool},
                  KeyValueMap{object_name: String, check_key_fn: CheckFn, value_schema: Box<Schema>} }
  fn check(&self,&Value)->Result<(),ValidationError>; fn apply_defaults(&self,&Value)->Value;
  fn remove_defaults(&self,&Value)->Value; ctors map, array, array_nonempty, key_value_map

pub fn validate(&Value, &Schema) -> Result<(), ValidationError>
pub fn apply_defaults(&Value, &Schema) -> Value          // returns a deep copy
pub fn remove_defaults(&Value, &Schema) -> Value
pub fn load_from_filename<F>(filename: &Path, schema: &Schema, load_strategy: F,
                             display_filename: Option<&str>) -> Result<Value, ValidationError>
  where F: Fn(&str) -> Result<Value, Box<dyn std::error::Error>>
```

Missing vs Python: `check_any` (≈`CheckFn::Any`), `check_type(tp)` for arbitrary types (only 5 scalar types),
`reraise_as` (trivial in Rust: `map_err`), `MISSING` sentinel (≈`Option`), `exc_tp` param (caller maps error).

### 2.3 Python cfgv 3.5.0 reference semantics (exact; engineers need not read `cfgv/cfgv.py`)

**Error rendering** (`cfgv/cfgv.py:10-23`): `str(e)` = `"\n"` + for each context level outer→inner
`"==> {ctx}\n"` + `"=====> {leaf_msg}"`. No trailing newline. pre-commit prints it as
`An error has occurred: InvalidConfigError: ` + str(e) (note trailing space before the newline) then
`Check the log at …/pre-commit.log`, exit 1 **[verified]**. `validate-config` prints just str(e) and exits 1.

**Context strings** (all verbatim):

| Where | String | Formatting of interpolated values |
|---|---|---|
| `_check_optional` (`:52-56`) | `At key: {key}` | `str(key)` |
| `check_array` (`:391-401`) | `At index {i}` | int |
| `Map.check` no id (`:239-240`) | `At {object_name}()` | |
| `Map.check` with id (`:241-243`) | `At {object_name}({id_key}={value!r})` | id_key `str`, value **`repr`**, `MISSING` if absent |
| `KeyValueMap.check` (`:275-280`) | `At {object_name}()`, then per entry `For key: {k}` (around key check) and `At key: {k}` (around value schema) | `str(k)` |
| `load_from_filename` (`:437`) | `File {display_filename}` | |

**Leaf messages** (verbatim):

| Source | Message |
|---|---|
| `_require_key` `:68-70` | `Missing required key: {key}` |
| `check_type` `:357-364` | `Expected {typename} got {type(v).__name__}` (typename: `string` for check_string, `text` for check_text, else `tp.__name__`, e.g. `dict`) |
| `Map.check` / `KeyValueMap.check` non-dict | `Expected a {object_name} map but got a {type(v).__name__}` |
| `Array.check` non-list | via `check_array(check_any)`: `Expected array but got {type(v).__name__!r}` → e.g. `Expected array but got 'dict'` |
| `Array` empty & `allow_empty=False` `:303-306` | `Expected at least 1 '{of.object_name}'` |
| `check_one_of` `:374-381` | `Expected one of {', '.join(str(x) for x in sorted(possible))} but got: {v!r}` |
| `check_regex` `:384-388` | `{v!r} is not a valid python regex` (`re.compile` must raise `re.error`) |
| `NoAdditionalKeys` `:144-152` | `Additional keys found: {extra}.  Only these keys are allowed: {keys}` — **two spaces** after `.`; `extra` = `sorted(set(dct)-set(keys))` joined `", "` with `str`; `keys` in declared order |
| ensure_absent `:113-120` | `Expected {key} to be absent when {condition_key} {explanation}, found {key}: {dct[key]!r}`; explanation: plain value ⇒ `is not {cv!r}`; `Not(v)` ⇒ `is {v!r}`; `NotIn(*vs)` ⇒ `is any of {vs!r}` (tuple repr); `In(*vs)` ⇒ `is not any of {vs!r}` |
| `load_from_filename` not file `:434-435` | `{display_filename} is not a file` (NOT inside `File …` context) |
| `load_from_filename` decode `:438-442` | `str(UnicodeDecodeError)`, e.g. `'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` (inside `File …`) **[verified]** |
| load_strategy exception `:444-447` | `str(e)` of whatever the loader raised (PyYAML `MarkedYAMLError` text for pre-commit) |

**Validator semantics** (`:48-224`):

| Validator | check | apply_default | remove_default |
|---|---|---|---|
| `Required(key, fn)` | missing ⇒ `Missing required key`; else `At key:` ctx + fn | noop | noop |
| `RequiredRecurse(key, schema)` | same with `validate(val, schema)` | `dct[key] = apply_defaults(dct[key], schema)` (KeyError if missing — only called after validate) | recurse |
| `Optional(key, fn, default)` | if present: `At key:` + fn | `dct.setdefault(key, default)` | `del` if `== default` (Python `==`; order of remaining keys preserved) |
| `OptionalRecurse(key, schema, default)` | if present: recurse | setdefault, then recurse apply | if present: recurse remove, then del if == default |
| `OptionalNoDefault(key, fn)` | like Optional | noop | noop |
| `Conditional(key, fn, ck, cv, ensure_absent=False)` | if `dct.get(ck, MISSING) == cv`: Required-check; elif `ck in dct and ensure_absent and key in dct`: ensure_absent error | noop | noop |
| `ConditionalOptional(key, fn, default, ck, cv, ensure_absent=False)` | if match: Optional-check; elif …: ensure_absent error | if match: setdefault | if match: del if == default |
| `ConditionalRecurse(key, schema, ck, cv, ensure_absent=False)` | if match: RequiredRecurse-check; elif …: ensure_absent | if match: recurse apply | if match: recurse remove |
| `NoAdditionalKeys(keys)` | error if extra keys | noop | noop |
| `WarnAdditionalKeys(keys, callback)` | `callback(sorted_extra, keys, dct)` if extra | noop | noop |

Condition matching: `cv` compared with Python `==` against the raw value; `Not(v).__eq__(o)` = `o is not MISSING and o != v`;
`NotIn(*vs)` = `o is not MISSING and o not in vs`; `In(*vs)` = `o is not MISSING and o in vs`; a plain
value matches with `==` (MISSING never equals anything). Python `==` coerces: `1 == 1.0 == True`, `0.0 == -0.0`.

**Containers**: `Map.check` iterates items in declaration order and stops at the first error (single error,
not aggregated). `Array.check`: no per-element context (`At index` is only from `check_array`).
`Map.apply_defaults`: shallow `v.copy()` then each item mutates. `KeyValueMap`/`Array` apply/remove map over
children. `validate(v, schema)` returns `v`.

**`load_from_filename(filename, schema, load_strategy, exc_tp=ValidationError, *, display_filename=None)`**
(`:424-450`): `os.path.isfile` (follows symlinks) → open UTF-8 **text mode** (universal newlines: `\r\n`/`\r` → `\n`)
→ `load_strategy(contents)` → `validate` → returns `apply_defaults(data)`; every `ValidationError` re-raised as
`exc_tp(e)`. Note a subtle consequence: if `load_strategy` itself raises a `ValidationError` (pre-commit's
`_load_manifest_forward_compat`, `pre-commit/pre_commit/clientlib.py:273-279`, calls `check_min_version('5')` when
the manifest is a dict), it is wrapped as `ValidationError(str(e))` whose message already starts with
`"\n=====> "` — i.e. the output contains a nested `=====>` line. Rust parity: `e.to_string()` of a cfgv
`ValidationError` reproduces this automatically.

### 2.4 Parity matrix — cfgv-rs vs Python

| # | Feature | Python | cfgv-rs | Status |
|---|---|---|---|---|
| 1 | Display format | `\n==> …\n=====> …` | identical (`src/error.rs:129-138`) | OK **[verified]** |
| 2 | `Map` context with id | `At Hook(id='x')` | identical for simple strings | OK |
| 3 | `repr(str)` quoting | uses `"…"` when string contains `'` and no `"` | always `'…'` (`src/value.rs:134`) | **BUG** — `Hook(id="it's")` vs `Hook(id='it's')` **[verified]** |
| 4 | `repr(str)` escaping | `\\`, `\'`/`\"`, `\t`, `\n`, `\r`, `\xNN` for C0/0x7f, non-printable non-ASCII as `\xNN`/`\uNNNN`/`\UNNNNNNNN` | none (raw newline printed, backslash single) | **BUG** — `'foo\\' is not a valid python regex` vs `'foo\' …`; `Hook(id='a\tbé​')` vs raw **[verified]** |
| 5 | `repr(float)` | `1.0`, `1e+16`, `1e-05`, `nan`, `inf`, `-0.0` | `1`, `100000000000000000000`, `0.00001`, `NaN`, `inf`, `-0` | **BUG** — `At Hook(id=1.0)` / `found rev: 1.0` **[verified pre-commit output]** |
| 6 | `repr(bytes)` | `b'ab'` | `[97, 98]` | BUG (rare) |
| 7 | `repr(date/datetime)`, type names `date`/`datetime` | `datetime.date(2020, 1, 1)`; `Expected string got date` | no such variants | **GAP** (needs loader + Value support) |
| 8 | big ints | arbitrary precision, type `int` | `i64` only; `yaml_serde` u64>i64::MAX ⇒ `Float` (`src/value.rs:282`) ⇒ "got float"; > u64 ⇒ yaml_serde hard error | GAP |
| 9 | ensure_absent explanation | `repr` of values; tuple repr (`('local', 'meta')`, single `('a',)`) | `Display` (unquoted) and no 1-tuple comma (`src/condition.rs:35-54`) | **BUG, hits pre-commit**: `repo is any of (local, meta)` vs `repo is any of ('local', 'meta')` **[verified both]** |
| 10 | `check_regex` dialect | Python `re` | Rust `regex` (`src/check.rs:112-120`) | **BLOCKER** — rejects valid Python: `a\Z`, `(?<!foo)bar`, `(?=x)`, `(a)\1`, `(?P<n>x)(?P=n)`, `a{`, `a{,3}`, `^(?!.*\.min\.js$).*\.js$` **[verified; pre-commit accepts `a\Z` and `(?<!foo)bar`]** |
| 11 | `check_regex` message quoting | `{v!r}` | `'{s}'` | BUG (see 3/4) — `"it's(" is not a valid python regex` **[verified]** |
| 12 | `check_one_of` order | `sorted(possible)` natural order (ints numeric) | sorted by `Display` string (`src/check.rs:101-104`) | minor BUG — `10, 2` vs `2, 10`; identical for the all-string lists pre-commit uses |
| 13 | `check_int(True)` | passes (`bool` ⊂ `int`) | `Expected int got bool` | minor; unused by pre-commit |
| 14 | equality | `1 == 1.0 == True`, `0.0 == -0.0`, `nan != nan` | variant-strict, floats by bits (`src/value.rs:87-101`) | minor |
| 15 | `Hash` vs `Eq` for maps | n/a | `Eq` is order-independent (IndexMap) but `Hash` is order-dependent (`src/value.rs:115-120`) ⇒ violates `a==b ⇒ hash(a)==hash(b)` | **BUG** (logic only; maps-as-keys) **[verified: MAP_EQ=true HASH_EQ=false]** |
| 16 | `remove_default` key order | `del` keeps order | `swap_remove` (`src/validator.rs:406,420,436`) moves last key into hole | BUG — `{'d': 3, 'b': 1, 'c': 2}` **[verified]**; pre-commit 4.6.2 never calls `remove_defaults` |
| 17 | `check_type(dict)` / `list` | supported (`Expected dict got int` — used for `ci:`) | `ValueType` has no Map/List/Null | **GAP** (pre-commit `clientlib.py:538`) **[verified message]** |
| 18 | user-defined map validators | any object with `check/apply_default/remove_default` | closed enum | **BLOCKER** — pre-commit defines 9 (see §3) |
| 19 | `load_from_filename` UTF-8 error | `'utf-8' codec can't decode byte 0xff in position 0: invalid start byte` | `stream did not contain valid UTF-8` | BUG **[verified]** |
| 20 | universal newlines | `\r\n`,`\r` → `\n` before parsing | raw | minor (affects loader line/col only) |
| 21 | `apply_defaults` on non-container | AttributeError (never happens post-validate) | silently returns `{}` / `[]` (`src/schema.rs:134,142,147`) | hazard (data loss if misused) |
| 22 | warnings during validation | arbitrary code (logger) | only `WarnAdditionalKeys` callback; no context passed to `CheckFn::Custom` | GAP |
| 23 | `yaml_serde` conversion | PyYAML SafeLoader (YAML 1.1) | YAML 1.2-ish via libyaml; tags dropped (`src/value.rs:299`) | **unsuitable** — see §2.7 |

### 2.5 Correctness / safety findings (cfgv-rs)

- No `unsafe`; no `unwrap/expect/panic!`; the only indexing `dct[key]` (`src/validator.rs:205`) is guarded by
  `contains_key`. Recursion depth of `check/apply_defaults` is bounded by **schema** depth, but `Value::{clone,
  drop, hash, eq, repr, Display}` recurse over **data** depth — the YAML loader must cap nesting (yaml_serde caps
  at 128; snp's own loader must enforce a limit too).
- MSRV declaration wrong (§2.1). Release CI does not catch it.
- `ValidationError::source()` returns the inner node ⇒ `anyhow`'s `{:#}`/`{:?}` chain printing would duplicate
  every level. snp must print cfgv errors with plain `Display` only.

### 2.6 Performance & memory (cfgv-rs)

| Aspect | Finding | Impact for snp |
|---|---|---|
| success path | `validate_context` takes a closure ⇒ no `format!` unless an error occurs | good |
| `NoAdditionalKeys`/`WarnAdditionalKeys` | allocate a `HashSet<&Value>` of allowed keys on **every** check | trivial (config sized) |
| `apply_defaults` | takes `&Value`, deep-clones the map (`as_map().cloned()`), then for every `*Recurse` item clones the child again (`dct.get(key).cloned()`) and the child clones itself ⇒ O(depth × size) copying; arrays `to_vec()` then clone each element again | negligible for configs (~1k nodes) but wasteful; add by-value API |
| `Validator` = 352 B | recurse variants embed `Schema` inline; pre-commit reuses `MANIFEST_HOOK_DICT.items` in 3 schemas ⇒ clones | use `Arc<Schema>` / `Arc<[Validator]>` |
| `Value::Map` | `IndexMap<Value,Value>` with SipHash `RandomState` (72-byte `Value`) | fine |
| `check_regex` | compiles then discards the regex; snp compiles again for matching | add a "validate & return compiled" path or cache |

### 2.7 `yaml_serde` feature — what it is and why snp must not rely on it

`yaml_serde` = YAML-org fork of `serde_yaml` (§1). It resolves scalars with YAML **1.2 core**-like rules,
while pre-commit uses PyYAML `CSafeLoader`/`SafeLoader` (YAML **1.1**). Probe results (cfgv-rs
`From<yaml_serde::Value>`) vs pre-commit 4.1.0 **[all verified]**:

| YAML input (`rev: …` unless noted) | PyYAML / pre-commit result | yaml_serde → cfgv-rs | Consequence |
|---|---|---|---|
| `always_run: yes` / `on` | `True` (bool) — config valid | `'yes'` string ⇒ `Expected bool got str` | valid configs rejected |
| `2020-01-01` | `date` ⇒ `Expected string got date` | string ⇒ accepted | invalid configs accepted |
| `2020-01-01 10:00:00` | `Expected string got datetime` | string | same |
| `017` | int 15 ⇒ `Expected string got int` | string `'017'` | accepted |
| `0o17` | **string** (YAML 1.1 has no `0o`) ⇒ accepted + mutable-ref warning | int 15 ⇒ rejected | divergence |
| `1:20` (sexagesimal) | int 80 ⇒ `got int` | string | divergence |
| `1_000` | int 1000 ⇒ `got int` | string | divergence |
| `!!binary aGVsbG8=` | bytes ⇒ `Expected string got bytes` | string (tag dropped) | divergence |
| `!foo bar` | error `could not determine a constructor for the tag '!foo'` + `in "<unicode string>", line 3, column 8` | `'bar'` | divergence |
| `12345678901234567890123` | int ⇒ `got int` | hard parse error | divergence |
| `18446744073709551615` | int | `Float` ⇒ `got float` | divergence |
| duplicate keys (`fail_fast` twice) | last wins, valid | `duplicate entry with key "fail_fast"` error | valid configs rejected |
| merge key `<<: *x` | merged (valid hook) | literal `'<<'` key kept | valid configs rejected |
| `1.10`, `.inf`, `~`, `0x1F`, `+1` | float 1.1, inf, None, 31, 1 | same | OK |
| empty document | `None` ⇒ `Expected a Config map but got a NoneType` | `Null` | OK |

Also: pervasive `unsafe` (c2rust libyaml), conflicting with snp's "no unsafe" goal. `saphyr-parser` 0.1.0
(pure Rust, 0 `unsafe` occurrences **[verified]**) is a candidate event parser on which snp can build a
PyYAML-SafeLoader-compatible resolver that emits `cfgv::Value` directly (scope of the YAML research doc).
**Recommendation:** deprecate/remove the `yaml_serde` feature in cfgv, or document it as "not PyYAML compatible".

### 2.8 Test coverage gaps (cfgv-rs)

Tests use only ints/bools in ensure_absent/one_of/regex-message tests, which hides bugs #3–#11. Missing tests:
string/float/bytes repr; ensure_absent with string & single-element `In`; `check_regex` on Python-only syntax;
unicode decode message; map hash/eq contract; key order after `remove_defaults`; custom validator; Send/Sync
static asserts; MSRV build.

---

## 3. What pre-commit needs from cfgv (drives the cfgv patch list)

From `pre-commit/pre_commit/clientlib.py` (4.6.2). Each line marks whether cfgv-rs 0.2.0 can express it.

| pre-commit construct | Lines | Semantics needed | cfgv-rs 0.2.0 |
|---|---|---|---|
| `check_string_regex = check_and(check_string, check_regex)` | 24 | Python-`re` validation; message `{v!r} is not a valid python regex` | ✗ (dialect + repr) |
| `check_type_tag(tag)` | 42-47 | `Type tag {tag!r} is not recognized.  Try upgrading identify and pre-commit?` (two spaces) against identify `ALL_TAGS` **[verified msg]** | ✓ via `CheckFn::custom` (needs correct `ALL_TAGS`, §4) |
| `check_min_version` | 55-61 | `pre-commit version {v} is required but version {C.VERSION} is installed.  Perhaps run \`pip install --upgrade pre-commit\`.` | ✓ custom |
| `StagesMigrationNoDefault(key, default)` | 112-133 | check: absent ⇒ ok; under `At key: {key}`: `check_array(check_any)`, map legacy names (`commit`→`pre-commit`, `merge-commit`→`pre-merge-commit`, `push`→`pre-push`), then `check_array(check_one_of(STAGES))`; apply_default: rewrite in place | ✗ needs custom map validator with apply_default |
| `StagesMigration` | 136-139 | apply_default: `setdefault(key, default)` then rewrite | ✗ |
| `DeprecatedStagesWarning(key)` | 142-165 | check (no ctx): `check_array(check_any)`; warn `hook id \`{id}\` uses deprecated stage names ({names}) which will be removed in a future version.  run: \`pre-commit migrate-config\` to automatically fix this.` | ✗ needs warning sink |
| `DeprecatedDefaultStagesWarning(key)` | 168-191 | same, message `top-level \`default_stages\` uses deprecated stage names (…) …` | ✗ |
| `LanguageMigration(key, check_fn)` | 201-219 | check under `At key:` on translated value (`system`→`unsupported`, `script`→`unsupported_script`); apply_default rewrites | ✗ |
| `LanguageMigrationRequired` | 222-227 | missing ⇒ `Missing required key: {key}` | ✗ |
| `WarnMutableRev(Conditional)` | 294-310 | Conditional + ensure_absent, then warning `The {key!r} field of repo {repo!r} appears to be a mutable reference (moving tag / branch).  Mutable references are never updated after first install and are not supported.  See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  Hint: \`pre-commit autoupdate\` often fixes this.` when `'.' not in rev and not re.match(r'^[a-fA-F0-9]+$', rev)` | ✗ (subclassing) |
| `OptionalSensibleRegexAtHook/AtTop` | 313-347 | OptionalNoDefault + warnings for `/*` and for substrings `[\\/]`, `[\/]`, `[/\\]` | ✗ |
| `NotAllowed(OptionalNoDefault)` | 401-404 | `{key!r} cannot be overridden` ⇒ `'entry' cannot be overridden` **[verified]** | ✗ |
| `cfgv.check_type(dict)` for `ci` | 538 | `Expected dict got int` **[verified]** | ✗ (`ValueType` lacks Map) |
| `WarnAdditionalKeys(…, warn_unknown_keys_root/repo)` | 358-373, 489, 520-533 | `Unexpected key(s) present at root: {", ".join(extra)}` / `Unexpected key(s) present on {repo}: …` | ✓ |
| `NoAdditionalKeys(language_names)` | 493 | `Additional keys found: pythn.  Only these keys are allowed: conda, …` **[verified]** | ✓ |
| `ConditionalRecurse(..., NotIn(LOCAL, META))`, `ConditionalOptional(key, check_any, value, 'id', hook_id)` | 413-490 | | ✓ (but message bug #9 for `WarnMutableRev`) |
| Introspection `item.key`, `item.check_fn` over `MANIFEST_HOOK_DICT.items` | 430-454 | derive CONFIG/META hook schemas from manifest schema | ✗ no accessors (snp can hand-build instead) |
| `cfgv.apply_defaults(hook, MANIFEST_HOOK_DICT)` on a config hook | `meta_hooks/check_useless_excludes.py:53` | includes custom `StagesMigration.apply_default` | ✗ |
| `cfgv.validate_context`, `cfgv.reraise_as` | `commands/migrate_config.py:118-123` | | ✓ / trivial |
| `remove_defaults` | — | unused by pre-commit 4.6.2 | n/a |

Exact `ensure_absent` output snp must reproduce **[verified, pre-commit 4.1.0]**:

```text
An error has occurred: InvalidConfigError: 
==> File .pre-commit-config.yaml
==> At Config()
==> At key: repos
==> At Repository(repo='local')
=====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1.0'
Check the log at /home/<user>/.cache/pre-commit/pre-commit.log
```

---

## 4. `file-identify` 0.4.0

### 4.1 Crate metadata

| Key | Value |
|---|---|
| edition / MSRV | 2024 / `1.94.0` (CI checks 1.94 — genuine, no toolchain file) |
| deps | `phf 0.12` (+macros), `smallvec 1.11`, `thiserror 2`; optional `clap 4` (`cli` feature, bin `file-identify`) |
| unsafe | none (no `#![forbid(unsafe_code)]`); `smallvec` itself contains unsafe |
| tests | 52 unit + 22 integration (`extensions_test` 4, `integration_tests` 12, `python_compatibility_test` 6) + 9 CLI + 10 doctests; all pass on Linux 1.98 **[verified]** |
| Windows | lib compiles for `x86_64-pc-windows-gnu` with 1 warning (`unused variable: metadata`, `src/lib.rs:968`; `-D warnings` would fail); **all test targets fail to compile** (7× `cannot find unix in os`) — tests & a doctest use `std::os::unix` unconditionally **[verified]**. CI is ubuntu-only |
| Send/Sync | `TagSet = HashSet<&'static str>`, `FileIdentifier`, `ShebangTuple`, `IdentifyError` are `Send + Sync` |

### 4.2 Public API (complete)

```text
pub type TagSet = std::collections::HashSet<&'static str>;          // tags.rs:16, 48 bytes + heap
pub mod tags { pub const DIRECTORY, SYMLINK, SOCKET, FILE, EXECUTABLE, NON_EXECUTABLE, TEXT, BINARY: &str;
               pub fn tags_from_array(&[&'static str]) -> TagSet;
               pub static TYPE_TAGS, MODE_TAGS, ENCODING_TAGS, ALL_TAGS: LazyLock<TagSet>;
               pub fn is_type_tag/is_mode_tag/is_encoding_tag(&str) -> bool;
               pub enum FileKind { Regular, Directory, Symlink, Socket } }
pub mod extensions { pub static EXTENSION_TAGS, EXTENSIONS_NEED_BINARY_CHECK_TAGS, NAME_TAGS:
                       phf::Map<&'static str, &'static [&'static str]>;
                     pub fn get_extension_tags/get_extensions_need_binary_check_tags/get_name_tags(&str) -> TagSet }
pub mod interpreters { pub static INTERPRETER_TAGS: phf::Map<..>; pub fn get_interpreter_tags(&str) -> TagSet }

pub fn tags_from_path<P: AsRef<Path>>(P) -> Result<TagSet>          // lib.rs:597
pub fn tags_from_filename(&str) -> TagSet                            // lib.rs:748
pub fn tags_from_interpreter(&str) -> TagSet                         // lib.rs:809
pub fn file_is_text<P: AsRef<Path>>(P) -> Result<bool>               // lib.rs:864
pub fn is_text<R: Read>(R) -> Result<bool>                           // lib.rs:894
pub fn parse_shebang_from_file<P: AsRef<Path>>(P) -> Result<ShebangTuple>  // lib.rs:964
pub fn parse_shebang<R: Read>(R) -> Result<ShebangTuple>             // lib.rs:1009
pub fn tags_from_info(&FileInfo<'_>) -> TagSet                       // lib.rs:677, I/O-free
pub struct FileInfo<'a> { filename: &'a str, file_kind: FileKind, is_executable: bool, content: Option<&'a [u8]> }
pub struct FileIdentifier { skip_content_analysis, skip_shebang_analysis, custom_extensions: Option<HashMap<String, TagSet>> }
  fn new(); skip_content_analysis(self); skip_shebang_analysis(self); with_custom_extensions(self, HashMap)
  fn identify<P>(&self, P) -> Result<TagSet>; fn identify_from(&self, &FileInfo) -> TagSet
pub struct ShebangTuple(Box<[String]>)  // len, is_empty, get, first, into_vec, iter, as_slice,
                                        // Index<usize> (PANICS on OOB), Display "('a', 'b')", FromIterator
pub type Result<T> = std::result::Result<T, IdentifyError>;
pub enum IdentifyError { PathNotFound{path} ("{path} does not exist."), IoError{source} ("IO error: {source}"),
                         InvalidPath{path} (never constructed), InvalidUtf8 (never constructed) }
```

Not ported (not needed by snp): `license_id`, `identify-cli` exact output (Rust CLI prints errors to stderr;
Python prints the `ValueError` to **stdout**, both exit 1).

### 4.3 Python identify 2.6.20 reference semantics (exact)

pre-commit uses exactly three identify entry points: `tags_from_path` (`pre-commit/pre_commit/commands/run.py:73-80`,
memoized per `Classifier` with `functools.cache`; `Classifier.__init__` first drops paths failing
`os.path.lexists`), `ALL_TAGS` (`clientlib.py:42-47`), and `parse_shebang_from_file` (`pre-commit/pre_commit/parse_shebang.py:15-19,65-85`:
**every hook command** goes through `normalize_cmd`: `cmd = parse_filename(exe) + (exe,) + cmd[1:]`, so the full
shebang token list — interpreter *and its args* — becomes the argv prefix on all platforms).

`tags_from_path(path)` (`identify/identify/identify.py:40-83`):
1. `sr = os.lstat(path)`; on `OSError`/`ValueError` raise `ValueError(f'{path} does not exist.')`.
2. `S_ISDIR` ⇒ `{'directory'}`; `S_ISLNK` ⇒ `{'symlink'}`; `S_ISSOCK` ⇒ `{'socket'}` (FIFOs/devices fall through as files).
3. `tags = {'file'}`; `executable = os.access(path, os.X_OK)` ⇒ add `executable` or `non-executable`.
   - Unix: `access(2)` with real uid/gid (owner/group/other bits, ACLs; root ⇒ any x bit).
   - **Windows: `os.access(…, X_OK)` ignores X_OK ⇒ True for every existing file ⇒ every file is `executable`.**
     (CPython `os_access_impl` on Windows only checks existence and, for W_OK, the read-only attribute; prek documents
     the same at `prek/crates/prek-identify/src/lib.rs:270-273`. Not runtime-verified here — Linux host.)
4. `t = tags_from_filename(os.path.basename(path))`; if non-empty ⇒ `tags |= t`; **elif** executable ⇒
   `shebang = parse_shebang_from_file(path)`; if non-empty ⇒ `tags |= tags_from_interpreter(shebang[0])`.
   (Shebang is consulted only when the filename yields nothing.)
5. If no `text`/`binary` tag yet ⇒ `file_is_text(path)` ⇒ add `text` or `binary`. (Happens for unknown names and
   for `EXTENSIONS_NEED_BINARY_CHECK` entries, whose tag sets carry no encoding tag.)

`tags_from_filename(path)` (`:86-105`): `filename = os.path.split(path)[1]` (basename; POSIX splits on `/`,
Windows on `/` and `\`); `ext = os.path.splitext(filename)[1]` (last `.` that has at least one non-dot char
before it in the basename: `.bashrc`⇒`''`, `..foo`⇒`''`, `foo.`⇒`'.'`, `a..b`⇒`'.b'`). For `part in [filename] + filename.split('.')`:
first `part in NAMES` ⇒ `ret |= NAMES[part]`, break. Then if `ext`: `ext = ext[1:].lower()` (Unicode lower);
`ext in EXTENSIONS` ⇒ union, **elif** in `EXTENSIONS_NEED_BINARY_CHECK` ⇒ union. Name tags and extension tags are
**unioned** (e.g. `Makefile.png` ⇒ `{binary,image,makefile,png,text}`).

`tags_from_interpreter(interp)` (`:108-118`): take text after last `/` (no `\` handling); loop: exact key in
`INTERPRETERS` ⇒ return it; else strip from the last `.` (`python3.12.1`→`python3.12`→`python3`); no `.` ⇒ `set()`.

`is_text(bytesio)` (`:121-132`): `bytesio.read(1024)` (buffered read ⇒ up to 1024 bytes, loops until full/EOF);
text iff every byte ∈ {7,8,9,10,11,12,13,27} ∪ 0x20..=0x7E ∪ 0x80..=0xFF. Empty ⇒ text.
`file_is_text(path)` (`:135-139`): `os.path.lexists` false ⇒ `ValueError('{path} does not exist.')`; open `rb`.

`parse_shebang(bytesio)` (`:178-202`):
1. `read(2) != b'#!'` ⇒ `()` (reads only 2 bytes for non-scripts).
2. `first_line = readline()` (rest of line incl. `\n`; **unbounded**); strict UTF-8 decode, failure ⇒ `()`.
3. Every char must be in `string.printable` = `0-9a-zA-Z` + punctuation + `' \t\n\r\x0b\x0c'`; else `()`.
4. `cmd = tuple(_shebang_split(first_line.strip()))` where `_shebang_split` = `shlex.split(line)` (POSIX,
   `whitespace_split=True`, **no comments**, whitespace `' \t\r\n'`), on `ValueError` (`No closing quotation`,
   `No escaped character`) fall back to `line.split()`.
5. `cmd[:2] == ('/usr/bin/env', '-S')` ⇒ `cmd[2:]`; elif `cmd[:1] == ('/usr/bin/env',)` ⇒ `cmd[1:]`
   (bare `env` is NOT stripped).
6. If `cmd == ('nix-shell',)` exactly ⇒ `_parse_nix_shebang` (`:154-175`): while the next 2 bytes are `#!`: read
   line, decode (fail ⇒ return cmd), printable check (fail ⇒ return cmd), tokenize; for every `-i` token not in last
   position set `cmd = (next_token,)` (last one wins; first token is not required to be `nix-shell`).
7. return `cmd`.

`parse_shebang_from_file(path)` (`:205-219`): `lexists` false ⇒ `ValueError`; `not os.access(X_OK)` ⇒ `()`
(never on Windows); open; `OSError` with `errno == EINVAL` ⇒ `()`, other errors propagate.

`ALL_TAGS` (`:32-37`) = 8 base tags ∪ all tags of the four tables = **323 tags** in 2.6.20.

### 4.4 Table diff vs identify 2.6.20 (exact) **[verified]**

Counts: EXTENSIONS py 343 / rs 315; NEED_BINARY_CHECK 5 / 2; NAMES 96 / 105; INTERPRETERS 25 / 22;
`ALL_TAGS` py 323 / rs **292**.

**EXTENSIONS missing in Rust (28):**
`cmd`→{batch,text}; `diff`→{diff,text}; `escript`→{erlang,text}; `hlsl`→{hlsl,text}; `hlsli`→{hlsl,text};
`ipp`→{c++,text}; `ipxe`→{ipxe,text}; `jbuilder`→{jbuilder,ruby,text}; `luau`→{luau,text}; `nu`→{nushell,text};
`piskel`→{json,piskel,text}; `pyw`→{python,text}; `qmd`→{quarto,text}; `robot`→{robot,text};
`slnx`→{msbuild,slnx,text,xml}; `tif`→{binary,image,tiff}; `tpp`→{c++,text}; `vala`→{text,vala};
`wixproj`→{text,wixproj,xml}; `wxi`→{text,wxi,xml}; `wxl`→{text,wxl,xml}; `wxs`→{text,wxs,xml};
`xaml`→{text,xaml,xml}; `xcconfig`→{text,xcconfig,xcodebuild}; `xcscheme`→{text,xcodebuild,xcscheme,xml};
`xcworkspacedata`→{text,xcodebuild,xcworkspacedata,xml}; `xlf`→{text,xliff,xml}; `xliff`→{text,xliff,xml}.

**EXTENSIONS different (1):** `xctestplan`: py {json,text,xcodebuild,xctestplan} / rs {json,text}.

**EXTENSIONS_NEED_BINARY_CHECK missing (3):** `entitlements`→{entitlements,plist}; `xcprivacy`→{plist,xcodebuild,xcprivacy};
`xcsettings`→{plist,xcodebuild,xcsettings}.

**NAMES missing in Rust (9):** `.profile`→{shell,text}; `Brewfile`→{ruby,text}; `Fastfile`→{ruby,text};
`SConscript`/`SConstruct`/`SCsub`→{scons,text}; `meson.options`→{meson,meson-options,text};
`pyproject.toml`→{pyproject,text,toml}; `uv.lock`→{text,toml,uv-lock}.

**NAMES extra in Rust — not in any identify version (18), must be removed:**
`.eslintrc`→{json,text}; `.eslintrc.js`→{javascript,text}; `.eslintrc.json`→{json,text}; `.eslintrc.yaml`/`.eslintrc.yml`→{text,yaml};
`.prettierrc`→{json,text}; `.prettierrc.json`→{json,text}; `.prettierrc.toml`→{text,toml}; `.prettierrc.yaml`/`.prettierrc.yml`→{text,yaml};
`.stylintrc`→{json,text}; `BUILD.bazel`/`WORKSPACE.bazel`→{bazel,text}; `composer.json`/`composer.lock`/`package-lock.json`/`package.json`→{json,text};
`yarn.lock`→{text,yaml}.
Harm examples: `yarn.lock` (v1 is not YAML) gets `yaml` ⇒ `check-yaml` would run on it and fail; `.eslintrc`
(may be YAML) gets `json` ⇒ `check-json` runs; `composer.lock` gets `json` though identify yields only `{text}`
(by content). Most of the others are harmless duplicates of extension results but still diverge for
`.eslintrc`, `.prettierrc`, `.stylintrc`, `composer.lock`, `yarn.lock`.

**NAMES different (5):** `Gemfile.lock` py {gemfile-lock,text} / rs {text}; `PKGBUILD` py {alpm,bash,pkgbuild,shell,text} /
rs lacks `shell`; `Pipfile.lock` py adds `pipfile-lock`; `meson_options.txt` py adds `meson-options`;
`poetry.lock` py adds `poetry-lock`.

**INTERPRETERS missing (3):** `escript`→{erlang}; `nu`→{nushell}; `nushell`→{nushell}.

**Tags absent from Rust `ALL_TAGS` (31)** ⇒ snp would reject valid configs with
`Type tag 'pyproject' is not recognized.  Try upgrading identify and pre-commit?` (pre-commit 4.1.0 itself
rejects `pyproject` because its venv has identify 2.6.7 **[verified]** — confirms the check is live):
`entitlements, gemfile-lock, hlsl, ipxe, jbuilder, luau, meson-options, nushell, pipfile-lock, piskel,
poetry-lock, pyproject, quarto, robot, scons, slnx, uv-lock, vala, wixproj, wxi, wxl, wxs, xaml, xcconfig,
xcodebuild, xcprivacy, xcscheme, xcsettings, xctestplan, xcworkspacedata, xliff`.

Table facts useful for codegen: all keys are ASCII; no uppercase extension keys; no extension key contains `.`;
max tags per entry 5; 323 tags fit in 6×u64.

### 4.5 Behavioral divergences (file-identify vs identify) **[verified unless noted]**

| # | Case | identify | file-identify | Severity |
|---|---|---|---|---|
| 1 | `tags_from_filename("dir/Dockerfile")` | {dockerfile,text} (basename first) | `[]` — no basename split (`lib.rs:752-758`); docs claim "can include path" | **BUG** (public API; `tags_from_path` itself uses `file_name()` so unaffected) |
| 2 | `..foo` extension | none | `foo` (Rust `Path::extension` semantics) | minor |
| 3 | non-UTF-8 filename (Unix) | works (surrogateescape str) | `file_name().to_str()` fails ⇒ **no name tags and no shebang parse** (`lib.rs:523`) | BUG |
| 4 | Windows executable | always `executable` | only `.exe/.bat/.cmd` (`lib.rs:502-511`) | **BUG** (and changes shebang/encoding path) |
| 5 | Unix executable | `access(2)` (uid/gid aware) | any `0o111` bit (`lib.rs:500`) | minor (non-owned files) |
| 6 | shebang quoting `#!"/usr/bin/my python" -x` | `['/usr/bin/my python','-x']` | `['"/usr/bin/my','python"','-x']` (`split_whitespace`, `lib.rs:1058`) | **BUG** — breaks `normalize_cmd` |
| 7 | `#!/usr/bin/env 'python3'` | `['python3']` | `["'python3'"]` | BUG |
| 8 | nix-shell 2-line shebang | `['python3']` | `['nix-shell']` | **BUG** — wrong interpreter + wrong argv |
| 9 | `\x0b`/`\x0c`/interior `\r` in shebang | accepted (printable) | rejected (`c.is_control()`, `lib.rs:1051-1055`) | minor |
| 10 | first line > 1024 bytes | parsed fully | truncated to 1024 before decoding (`lib.rs:1037-1039`) ⇒ wrong/garbled token | minor |
| 11 | non-`#!` executable, huge first line | reads 2 bytes | `read_until(b'\n')` reads the **entire first line into memory** before checking `#!` (`lib.rs:1016`): 50 MB newline-free exec ⇒ 6106 reads, maxRSS 51 MB | **BUG (memory/perf)** |
| 12 | `is_text` short read | `BufferedReader.read(1024)` loops | single `Read::read` (`lib.rs:922`); `Cursor("abc").chain(Cursor("\0\1"))` ⇒ `true` (Python: false) | BUG (no `Interrupted` retry either) |
| 13 | `tags_from_info` / `identify_from` with `content: None` or `skip_content_analysis` | n/a | tag set may lack an encoding tag — breaks identify invariant (`assert ENCODING_TAGS & tags`) | API hazard |
| 14 | error mapping | lstat OSError ⇒ `'{path} does not exist.'`; read errors propagate | same message for lstat (`lib.rs:602-608`); `file_is_text` on missing path ⇒ `IO error: …` instead of `does not exist.` | minor |
| 15 | FIFO in worktree | `open()` blocks forever | same (opens it) | both hang — snp should deliberately not open non-regular files |
| 16 | `Makefile.png`, `Dockerfile.xenial`, `.bashrc`, `a\b\c.py` (POSIX) | union / name-part / whole-name / ext py | identical | OK |

### 4.6 Syscalls & allocations per file (`strace`, Linux, `tags_from_path`) **[verified]**

| File | file-identify 0.4.0 | identify (Python) | Minimum achievable |
|---|---|---|---|
| `a.py` (known ext, non-exec) | 1 `statx` | lstat + access | **0** with git index mode + trusted existence, else 1 `statx` |
| `data.unknownext` (non-exec) | statx, openat, read(1024), close = 4 | lstat, access, lstat (lexists), open, read, close | statx + openat + read + close = 4 |
| `script` (exec, no ext, `#!/usr/bin/env python3`) | **2× statx, 2× openat, read(8192) + read(1024), 2× close = 8**; 8 KiB `BufReader` heap buffer + line `Vec` | ~10 | statx + openat + 1 read(1024) + close = 4 |
| 50 MB exec without newline | 2 statx + 6106 reads, 51 MB RSS | reads 2 bytes | 4 syscalls, 1 KiB stack |

CPU: `tags_from_filename` ≈ **307–319 ns/op** (HashSet allocations per lookup: every `get_*_tags` builds a fresh
`HashSet` with a fresh `RandomState`, plus `to_lowercase()` `String`); naive bitset prototype with std
`HashMap` ≈ 120 ns/op and zero allocations (phf + `Copy` bitset should be lower);
`tags_from_interpreter` ≈ 100 ns; `tags_from_path` (warm cache, 42 mixed files) ≈ 2.1 µs/file **[verified]**.

### 4.7 Other findings

- `IdentifyError::{InvalidPath, InvalidUtf8}` are dead variants; enum not `#[non_exhaustive]`.
- `ShebangTuple` re-implements `Vec<String>` with a Python-tuple `Display`; `Index` panics on OOB.
- `parse_shebang` swallows read errors as "no shebang" (Python propagates; acceptable).
- `smallvec` dependency is unnecessary (used for a 4-element temp vector).
- `FileIdentifier.custom_extensions` overrides *all* built-in name/extension logic — not identify semantics;
  snp does not need it.
- CHANGELOG has no 0.4.0 entry; HEAD commit "Add ALL_TAGS and make INTERPRETER_TAGS public".

---

## 5. prek-identify (j178/prek) — high-level comparison (lessons only; do not copy)

| Aspect | file-identify 0.4.0 | prek-identify | Lesson for snp/file-identify |
|---|---|---|---|
| Table source | hand-maintained, drifted | generated by `gen.py` (PEP 723 script importing installed identify ≥2.6.16) into checked-in `src/tags.rs`; **3 deliberate overrides** (`cts`,`mts`→`{text,ts}`, `jsonc`) | Generate from upstream; parse tables as data (ast), no overrides by default |
| TagSet | `HashSet<&'static str>` | `Copy` bitset `[u64; ceil(324/64)]`, `const fn new(&[u16])`, ids index a sorted `ALL_TAGS` | Adopt bitset idea (own implementation) |
| Config tag validation | via `ALL_TAGS` lookup | `serde::Deserialize for TagSet` with prek-specific message | snp needs pre-commit's message (cfgv custom check) |
| `tags_from_filename` | identify-like (bugs §4.5) | **changed semantics**: exact name + extension; name-part fallback only if nothing matched; NEED_BINARY_CHECK merged into EXTENSIONS | Keep exact identify semantics |
| Shebang use in `tags_from_path` | only when filename gave nothing (parity) | **always** for executables (extra I/O; `foo.sh` with python shebang gets both `shell` and `python`) | Keep parity |
| Shebang tokenizer | `split_whitespace` | `shlex` crate — treats `#` as a comment (`/bin/sh #foo` ⇒ `['/bin/sh']`, Python ⇒ `['/bin/sh','#foo']`) and `\r` as non-whitespace **[verified]** | Write a Python-`shlex` port (§8.2 F6) |
| env stripping | `/usr/bin/env[ -S]` | also bare `env[ -S]` | parity: only `/usr/bin/env` |
| nix-shell | missing | when *first* token is `nix-shell` (upstream: only when cmd == `('nix-shell',)`); requires program `nix-shell` on continuation lines (upstream doesn't) | exact port |
| Windows exec | `.exe/.bat/.cmd` | `true` (documented as identify parity) | `true` in compat mode |
| text detection | single `read`, errors propagate | single `read`, open/read error ⇒ binary | loop-read; propagate error |

---

## 6. lefthook ideas relevant to identification

lefthook `file_types` (`lefthook/internal/run/controller/filter/filter.go:205-322`): `text`, `binary`,
`executable`, `not executable`, `symlink`, `not symlink`, plus MIME types (`*/*`) via content sniffing
(`gabriel-vasile/mimetype`); executable = any perm x bit and not a symlink; text check uses `io.ReadFull`
(correct full read). Ideas for snp (opt-in extensions, never changing pre-commit semantics): negative tags
(`exclude_types` already covers), optional `mime:` predicates behind a cargo feature using a pure-Rust magic
sniffer on the same 1 KiB head buffer we already read.

---

## 7. How snp should use the crates

### 7.1 Config pipeline with cfgv

```text
read bytes ──► strict UTF-8 (Python-style error msg) ──► normalize \r\n,\r → \n
           ──► snp YAML-1.1 loader (SafeLoader-compatible) ──► cfgv::Value
           ──► cfgv::validate_with(&value, &CONFIG_SCHEMA, &mut cx)   // warnings collected in cx, emitted in order
           ──► cfgv::apply_defaults_owned(value, &CONFIG_SCHEMA)       // by value, no deep clones
           ──► typed structs (Config/Repo/Hook) via serde Deserializer for cfgv::Value (or hand-written FromValue)
```

- Build every schema once: `static CONFIG_SCHEMA: LazyLock<Schema>` (all types are `Send + Sync`).
- Implement pre-commit's 9 custom validators (§3) as `Validator::Custom(Arc<dyn MapItem>)` (after patch C1).
- `check_regex`: plug snp's Python-compatible regex validator (same engine used for matching), and cache the
  compiled regex keyed by pattern string so validation and matching compile once.
- `check_type_tag`: `CheckFn::custom(|v| …)` using file-identify's `Tag::from_str` (binary search over sorted
  `ALL_TAGS`), message verbatim (`Type tag {repr} is not recognized.  Try upgrading identify and pre-commit?`).
- Manifest merge (`{**manifest_hook, **config_hook}`) can be done on `IndexMap` before typed conversion.
- Print errors with `Display` only (never anyhow chain formatting).

### 7.2 File classification with file-identify

```rust
// one per run; shared by all hooks
pub struct FileTable {
    entries: Box<[FileEntry]>,          // order = git order
}
pub struct FileEntry {
    path: Box<str>,                     // repo-relative, '/'-separated (bytes/OsStr if non-UTF-8 support needed)
    git_mode: Option<u32>,              // from `git diff --cached --raw -z` / `git ls-files -s -z`
    tags: OnceLock<Result<TagSet, Arc<IdentifyError>>>,  // lazy, thread-safe memo (== functools.cache)
}
// hooks receive Arc<FileTable> + Vec<u32> indices, never cloned strings
```

- Compile each hook's `types`, `types_or`, `exclude_types` into `TagSet` at config load. Match =
  `tags.is_superset(types) && (types_or.is_empty() || tags.intersects(types_or)) && !tags.intersects(exclude_types)`
  (`pre-commit/pre_commit/commands/run.py:81-98` semantics; ~6 word ops).
- Compute tags lazily per hook candidate set (after `files`/`exclude` regex filtering), in parallel (rayon),
  memoized in `OnceLock` — identical observable results to pre-commit's lazy `functools.cache`.
- Existence filter (`os.path.lexists`, `run.py:75`) fused with identification's `symlink_metadata` ⇒ one
  `statx` per candidate file.
- Staged-files mode (default hook run): after the unstaged-changes stash, worktree files equal the index and
  all staged (ACMRTUXB) paths exist, so on Unix with `core.fileMode=true` the git mode is authoritative ⇒
  name-matched files need **zero** syscalls. Use `Meta::from_git_mode` there; use `lstat` for `--all-files`,
  `--files`, Windows, and `core.fileMode=false`.
- `normalize_cmd` parity: use the (patched) `parse_shebang_from_file` returning full `Vec<String>` argv prefix.
- Memory: 100k files ≈ (16 B path ptr + 48 B TagSet + memo/flags) ≈ 7–8 MB plus path bytes. Optional: intern
  distinct TagSets (few hundred per repo) and store `u16` ids.

### 7.3 What is missing that snp needs (summary)

cfgv: custom map validators with apply_default; warnings sink; pluggable regex check; exact Python `repr`
(str/float/bytes/date/datetime/big int); `ValueType::{Map,List,Null}` (`dict`, `list`, `NoneType`); Python
UTF-8 error text; ensure_absent message fix; by-value `apply_defaults`; serde `Deserializer` for `&Value`;
`Value` variants for `date`/`datetime`/big ints; correct MSRV.

file-identify: tables == identify 2.6.20 (+ codegen + sync CI); bitset `TagSet` + `Tag` ids + `Tag::from_str`;
basename/splitext parity; byte/OsStr filenames; Python-shlex port; nix-shell; bounded/2-byte-first shebang read;
full-read `is_text`; one-open classification; git-mode-aware `Meta`; Windows exec parity; never open non-regular
files; Windows-compilable tests + CI matrix.

---

## 8. Patch plan for the user's crates (with code sketches)

Priority: **P0** = required for pre-commit parity/safety, **P1** = performance/memory, **P2** = polish.

Sketch validation **[verified]**: `py_shlex_split` (20 cases incl. quotes, escapes, `#`, `\r`, `\x0b`, unterminated
quote, trailing backslash), `py_repr_float` (18 values incl. `1e16`, `1e-05`, `5e-324`, `0.1+0.2`), `py_utf8_error`
(11 byte strings incl. truncated, overlong, surrogate, >U+10FFFF) and ASCII `py_repr_str` (8 strings) were compiled and
compared against CPython's `shlex.split`/`repr`/`UnicodeDecodeError` — 0 mismatches (`scratchpad/tmp-user-crates/sketch/`).

### 8.1 cfgv → 0.3.0 (breaking)

**C1 (P0) Custom map validators + warning context.**

```rust
pub type Map = IndexMap<Value, Value>;
pub struct Cx<'a> { pub warn: &'a mut dyn FnMut(String) }   // ordered warning sink

pub trait MapItem: Send + Sync + std::fmt::Debug {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError>;
    fn apply_default(&self, _dct: &mut Map) {}
    fn remove_default(&self, _dct: &mut Map) {}
    fn key(&self) -> Option<&Value> { None }          // introspection used to derive schemas
    fn check_fn(&self) -> Option<&CheckFn> { None }
}
#[non_exhaustive]
pub enum Validator { /* existing variants */, Custom(Arc<dyn MapItem>) }

pub fn validate_with(v: &Value, s: &Schema, cx: &mut Cx<'_>) -> Result<(), ValidationError>;
pub fn validate(v: &Value, s: &Schema) -> Result<(), ValidationError> { validate_with(v, s, &mut Cx { warn: &mut |_| {} }) }
```
Thread `cx` through `Schema::check`/`Validator::check`; `WarnAdditionalKeys` may keep its callback.
Add `Validator::key()`/`check_fn()` accessors for built-ins too.

**C2 (P0) Pluggable regex check** (also make `regex` an optional default feature):

```rust
pub type RegexCheck = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;
pub enum CheckFn { /* … */ Regex(Option<RegexCheck>) }  // None ⇒ built-in `regex` (feature "regex")
impl CheckFn { pub fn check_regex_with(f: RegexCheck) -> Self { CheckFn::Regex(Some(f)) } }
// failure message: format!("{} is not a valid python regex", Value::String(s.into()).repr())
```

**C3 (P0) Exact Python `repr`.** Replace `Value::repr` with a writer-based implementation:

```rust
impl Value {
    pub fn repr(&self) -> String { let mut s = String::new(); self.write_repr(&mut s); s }
    pub fn write_repr(&self, out: &mut String) { match self {
        Value::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Value::Null => out.push_str("None"),
        Value::Int(i) => { let _ = write!(out, "{i}"); }
        Value::Float(f) => py_repr_float(*f, out),
        Value::String(s) => py_repr_str(s, out),
        Value::Bytes(b) => py_repr_bytes(b, out),
        Value::List(l) => { out.push('['); for (i, v) in l.iter().enumerate() { if i > 0 { out.push_str(", "); } v.write_repr(out); } out.push(']'); }
        Value::Map(m) => { out.push('{'); for (i, (k, v)) in m.iter().enumerate() { if i > 0 { out.push_str(", "); } k.write_repr(out); out.push_str(": "); v.write_repr(out); } out.push('}'); }
        // new variants (C7): Date ⇒ "datetime.date(2020, 1, 1)", DateTime ⇒ "datetime.datetime(2020, 1, 1, 10, 0)", BigInt ⇒ digits
    }}
}

/// CPython unicode_repr (Objects/unicodeobject.c).
fn py_repr_str(s: &str, out: &mut String) {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    out.push(q);
    for c in s.chars() {
        let n = c as u32;
        match c {
            _ if c == q || c == '\\' => { out.push('\\'); out.push(c); }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if n < 0x20 || n == 0x7f => { let _ = write!(out, "\\x{n:02x}"); }
            _ if n < 0x7f => out.push(c),
            _ if py_isprintable(c) => out.push(c),
            _ if n <= 0xff => { let _ = write!(out, "\\x{n:02x}"); }
            _ if n <= 0xffff => { let _ = write!(out, "\\u{n:04x}"); }
            _ => { let _ = write!(out, "\\U{n:08x}"); }
        }
    }
    out.push(q);
}
/// str.isprintable for one char: NOT in {Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs} (U+0020 is printable).
/// Needs a Unicode general-category table (e.g. a small generated table or a pure-Rust crate).
fn py_isprintable(c: char) -> bool { /* … */ }

/// CPython float_repr ('r' mode, Py_DTSF_ADD_DOT_0).
fn py_repr_float(x: f64, out: &mut String) {
    if x.is_nan() { out.push_str("nan"); return; }
    if x.is_infinite() { out.push_str(if x > 0.0 { "inf" } else { "-inf" }); return; }
    let s = format!("{x:e}");                       // shortest round-trip digits, e.g. "-1.2345e-7"
    let Some((mant, exp)) = s.split_once('e') else { out.push_str(&s); return; };
    let exp: i32 = exp.parse().unwrap_or(0);
    if mant.starts_with('-') { out.push('-'); }
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let decpt = exp + 1;                            // value = 0.DIGITS × 10^decpt
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 { out.push('.'); out.push_str(&digits[1..]); }
        let e = decpt - 1;
        let _ = write!(out, "e{}{:02}", if e < 0 { '-' } else { '+' }, e.unsigned_abs());
    } else if decpt <= 0 {
        out.push_str("0."); for _ in 0..(-decpt) { out.push('0'); } out.push_str(&digits);
    } else {
        let d = decpt as usize;
        if d >= digits.len() { out.push_str(&digits); for _ in digits.len()..d { out.push('0'); } out.push_str(".0"); }
        else { out.push_str(&digits[..d]); out.push('.'); out.push_str(&digits[d..]); }
    }
}
// expected: 1.0→"1.0", 0.0→"0.0", -0.0→"-0.0", 1e16→"1e+16", 1e15→"1000000000000000.0",
//           1e-5→"1e-05", 1e-4→"0.0001", 1.1→"1.1", 123.456→"123.456"
// py_repr_bytes: b'…' with same quote rule; escape \\, quote, \t \n \r; bytes <0x20 or >=0x7f ⇒ \xNN
```
`Display` for `String` stays unquoted (`str()`); lists/maps in `Display` use element `repr` (already correct).

**C4 (P0) ensure_absent explanation.**

```rust
pub fn describe_opposite(&self) -> String { match self {
    Self::Exact(v) => format!("is not {}", v.repr()),
    Self::Not(v) => format!("is {}", v.repr()),
    Self::In(vs) => format!("is not any of {}", py_tuple_repr(vs)),
    Self::NotIn(vs) => format!("is any of {}", py_tuple_repr(vs)),
}}
fn py_tuple_repr(vs: &[Value]) -> String { match vs {
    [one] => format!("({},)", one.repr()),
    _ => format!("({})", vs.iter().map(Value::repr).collect::<Vec<_>>().join(", ")),
}}
```
Also `check_one_of`: message `but got: {v.repr()}` is already repr; fix ordering to Python natural order
(numbers numerically, strings by code point, mixed ⇒ group by type) — strings already correct.

**C5 (P0) `ValueType::{Map, List, Null}`** with names `dict`, `list`, `NoneType`
(`check_type(ValueType::Map, None)` ⇒ `Expected dict got int`). Optionally `ValueType::Int` accepts `Bool`
(Python `isinstance(True, int)`) behind an explicit `IntOrBool` variant.

**C6 (P0) Python-compatible `load_from_filename`.**

```rust
let bytes = std::fs::read(path).map_err(|e| ValidationError::new(e.to_string()))?;   // inside File ctx
let text = std::str::from_utf8(&bytes).map_err(|e| ValidationError::new(py_utf8_error(&bytes, e)))?;
let text: Cow<str> = if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n").into() } else { text.into() };

fn py_utf8_error(b: &[u8], e: std::str::Utf8Error) -> String {
    let start = e.valid_up_to();
    let first = b.get(start).copied().unwrap_or(0);
    let (end, reason) = match e.error_len() {
        None => (b.len(), "unexpected end of data"),
        Some(n) => (start + n, if matches!(first, 0x80..=0xC1 | 0xF5..=0xFF) { "invalid start byte" } else { "invalid continuation byte" }),
    };
    if end - start <= 1 { format!("'utf-8' codec can't decode byte 0x{first:02x} in position {start}: {reason}") }
    else { format!("'utf-8' codec can't decode bytes in position {start}-{}: {reason}", end - 1) }
}
```
(Rust's `error_len` = CPython's maximal-subpart range, so ranges coincide.) Keep the `is not a file` check
outside the `File` context.

**C7 (P0) Value variants for PyYAML types:** `Date{y,m,d}` (type_name `date`), `DateTime{…, tz}` (`datetime`),
`BigInt(Box<str>)` (`int`, decimal digits). Construction is the YAML loader's job; cfgv just needs type names
and repr. Remove/feature-gate the lossy `From<yaml_serde::Value>` (tags dropped, u64→Float).

**C8 (P0) Hash/Eq contract:** for `Value::Map` hash only `len()` plus an order-independent combination
(`wrapping_add` of per-entry hashes computed with a fixed hasher), or just `len()`.

**C9 (P0) MSRV:** set `rust-version = "1.88"` (or 1.98 to match snp), fix CI: delete `channel = "stable"` from
`rust-toolchain.toml` or run `cargo +${{ matrix.toolchain }} test`.

**C10 (P1) By-value defaults:**

```rust
impl Default for Value { fn default() -> Self { Value::Null } }
impl Schema {
    pub fn apply_defaults_owned(&self, v: Value) -> Value { match (self, v) {
        (Schema::Map { items, .. }, Value::Map(mut m)) => { for it in items { it.apply_default(&mut m); } Value::Map(m) }
        (Schema::Array { of, .. }, Value::List(l)) => Value::List(l.into_iter().map(|x| of.apply_defaults_owned(x)).collect()),
        (Schema::KeyValueMap { value_schema, .. }, Value::Map(m)) =>
            Value::Map(m.into_iter().map(|(k, x)| (k, value_schema.apply_defaults_owned(x))).collect()),
        (_, other) => other,                    // never replace unvalidated data with {} / []
    }}
}
// *Recurse items: if let Some(slot) = dct.get_mut(key) { *slot = schema.apply_defaults_owned(std::mem::take(slot)); }
```
Keep `apply_defaults(&Value)` as `apply_defaults_owned(v.clone())`.

**C11 (P1) Share sub-schemas:** `Arc<Schema>` in recurse variants and `Arc<[Validator]>` for `Map.items`
(pre-commit reuses `MANIFEST_HOOK_DICT.items` 3×). `Validator` shrinks from 352 B.

**C12 (P1) `remove_default` uses `shift_remove`** (preserve order).

**C13 (P2) serde integration:** feature `serde`: `impl<'de> serde::Deserializer<'de> for &'de Value` (+
`IntoDeserializer`) so snp can `Hook::deserialize(&value)` after validation; `Serialize for Value` for
`migrate-config`/debug dumps.

**C14 (P2) Hygiene:** `#![forbid(unsafe_code)]`, `#[non_exhaustive]` on public enums, `#[must_use]`,
static `Send + Sync` assertions, tests for every message in §2.3 with non-trivial strings, proptest/fuzz
(`cargo fuzz` target: arbitrary `Value` × pre-commit schema ⇒ no panic), README version fix.

### 8.2 file-identify → 0.5.0 (breaking)

**F1 (P0) Generated tables == identify 2.6.20.** Add `xtask gen-identify <path-to-identify-src>` that parses
`extensions.py`/`interpreters.py` **as data** (Python `ast` mini-evaluator for dict/set literals,
`EXTENSIONS['x']` subscripts and `|` unions — see `scratchpad/scripts/diff_tables.py`; never import/execute
upstream) and writes `src/generated.rs`:

```rust
// @generated from identify 2.6.20 — DO NOT EDIT
pub const IDENTIFY_VERSION: &str = "2.6.20";
pub const ALL_TAGS: [&str; 323] = [ /* sorted */ ];
pub const TAG_FILE: Tag = Tag(…); /* …the 8 base tags… */
pub static EXTENSIONS: phf::Map<&str, TagSet> = phf_map! { "py" => TagSet::from_ids(&[…]), … };
pub static EXTENSIONS_NEED_BINARY_CHECK: phf::Map<&str, TagSet> = …;   // keep separate (elif semantics)
pub static NAMES: phf::Map<&str, TagSet> = …;
pub static INTERPRETERS: phf::Map<&str, TagSet> = …;
```
CI job (weekly): download latest identify wheel from PyPI into an empty dir, regenerate, fail/open PR on diff.
Remove the 18 invented names; add the 31 missing tags. Expose `IDENTIFY_VERSION` (snp can print it in `--version`).

**F2 (P0/P1) Bitset `TagSet` + `Tag`.**

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Tag(u16);
impl Tag { pub fn as_str(self) -> &'static str { ALL_TAGS[self.0 as usize] }
           pub fn from_str(s: &str) -> Option<Tag> { ALL_TAGS.binary_search(&s).ok().map(|i| Tag(i as u16)) } }

const WORDS: usize = ALL_TAGS.len().div_ceil(64);
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TagSet([u64; WORDS]);
impl TagSet {
    pub const EMPTY: Self = Self([0; WORDS]);
    pub const fn from_ids(ids: &[u16]) -> Self { /* const loop */ }
    pub const fn with(mut self, t: Tag) -> Self { self.0[t.0 as usize / 64] |= 1 << (t.0 % 64); self }
    pub fn contains(&self, t: Tag) -> bool { self.0[t.0 as usize / 64] >> (t.0 % 64) & 1 == 1 }
    pub fn is_superset(&self, o: &Self) -> bool { self.0.iter().zip(o.0).all(|(a, b)| b & !a == 0) }
    pub fn intersects(&self, o: &Self) -> bool { self.0.iter().zip(o.0).any(|(a, b)| a & b != 0) }
    pub fn union(self, o: Self) -> Self { let mut r = self; for i in 0..WORDS { r.0[i] |= o.0[i]; } r }
    pub fn iter(&self) -> impl Iterator<Item = Tag> + '_ { /* trailing_zeros walk, sorted by name */ }
    pub fn has_encoding(&self) -> bool { self.contains(TAG_TEXT) || self.contains(TAG_BINARY) }
}
impl FromStr for TagSet / TryFrom<&[&str]> (error lists unknown tags)
```
`TagSet` is `Copy`, 48 bytes, no heap; all lookups return `TagSet` by value. (Bounds: tag ids are generated,
so `t.0 < ALL_TAGS.len()` holds by construction — use `get()` anyway in `as_str` to stay panic-free.)

**F3 (P0) Python path semantics on bytes.**

```rust
/// os.path.basename: POSIX splits on '/', Windows on '/' and '\\' (and drive colon).
fn py_basename(p: &[u8]) -> &[u8] {
    let is_sep = |b: &u8| *b == b'/' || (cfg!(windows) && (*b == b'\\' || *b == b':'));
    match p.iter().rposition(is_sep) { Some(i) => &p[i + 1..], None => p }
}
/// os.path.splitext(name)[1][1:]  (None when there is no extension)
fn py_ext(name: &[u8]) -> Option<&[u8]> {
    let dot = name.iter().rposition(|&b| b == b'.')?;
    if name[..dot].iter().all(|&b| b == b'.') { return None; }   // ".bashrc", "..foo", "..."
    Some(&name[dot + 1..])                                         // "foo." ⇒ Some("") (never matches)
}
pub fn tags_from_filename(path: impl AsRef<[u8]>) -> TagSet {
    let name = py_basename(path.as_ref());
    let mut ret = TagSet::EMPTY;
    let whole = std::str::from_utf8(name).ok();
    if let Some(t) = whole.and_then(|n| NAMES.get(n)) { ret = *t; }
    else { for part in name.split(|&b| b == b'.') {
        if let Some(t) = std::str::from_utf8(part).ok().and_then(|p| NAMES.get(p)) { ret = *t; break; } } }
    if let Some(ext) = py_ext(name) {
        let t = lookup_ext_lower(ext);   // ASCII fast path into [u8; 32] stack buffer (> max key len ⇒ miss);
                                         // non-ASCII ⇒ str::to_lowercase (Unicode, e.g. U+212A KELVIN → 'k')
        ret = ret.union(t);
    }
    ret
}
```
On Unix accept `&OsStr` via `as_bytes()`; on Windows use `to_string_lossy` (git paths are UTF-8 there).

**F4 (P0) Windows/Unix executable semantics.**

```rust
pub enum ExecPolicy { PythonCompat, ModeBits }
// PythonCompat: unix ⇒ access(2)-like: euid==0 ? any x : owner/group/other bit by st_uid/st_gid vs
//   getuid()/getgid()/getgroups() (cached once; safe via rustix); windows ⇒ true for every regular file.
// ModeBits: mode & 0o111 != 0 (what git/lefthook use).
```

**F5 (P0) One-pass, git-mode-aware classification API.**

```rust
#[non_exhaustive]
pub enum Kind { Regular, Directory, Symlink, Socket, Other /* fifo, device: never opened */ }
#[derive(Clone, Copy)] pub struct Meta { pub kind: Kind, pub executable: bool }
impl Meta {
    pub fn from_metadata(md: &std::fs::Metadata, p: ExecPolicy) -> Self;
    /// 100644 ⇒ Regular, 100755 ⇒ Regular+exec, 120000 ⇒ Symlink, 160000 (gitlink) ⇒ Directory, else None
    pub fn from_git_mode(mode: u32) -> Option<Self>;
}
pub enum Plan { Done(TagSet), NeedHead { partial: TagSet, shebang: bool, encoding: bool } }
pub fn plan(meta: Meta, path: &[u8]) -> Plan;                       // pure, zero I/O
pub fn finish<R: BufRead>(partial: TagSet, shebang: bool, encoding: bool, head: &[u8], rest: &mut R) -> io::Result<TagSet>;

pub fn tags_from_path(path: &Path) -> Result<TagSet, IdentifyError> {
    let md = fs::symlink_metadata(path).map_err(|_| IdentifyError::NotFound(path.display().to_string()))?;
    match plan(Meta::from_metadata(&md, ExecPolicy::PythonCompat), path_bytes(path)) {
        Plan::Done(t) => Ok(t),
        Plan::NeedHead { partial, shebang, encoding } => {
            let mut f = fs::File::open(path)?;                      // exactly one open
            let mut head = [0u8; 1024];
            let n = read_up_to(&mut f, &mut head)?;                 // loop until full/EOF, retry Interrupted
            Ok(finish(partial, shebang, encoding, &head[..n], &mut BufReader::with_capacity(4096, f))?)
        }
    }
}
```
`Kind::Other` files: do not open (Python would block on a FIFO); return `file` + mode tag + name tags +
`binary` when no encoding tag — documented intentional divergence. Keep the old functions as thin wrappers.

**F6 (P0) Exact `parse_shebang` with a Python-`shlex` port.**

```rust
pub const MAX_SHEBANG_LINE: u64 = 64 * 1024;   // Python is unbounded; cap documented

pub fn parse_shebang<R: BufRead>(r: &mut R) -> io::Result<Vec<String>> {
    let mut magic = [0u8; 2];
    if read_up_to(r, &mut magic)? != 2 || magic != *b"#!" { return Ok(Vec::new()); }   // 2-byte probe first
    let mut line = Vec::new();
    r.by_ref().take(MAX_SHEBANG_LINE).read_until(b'\n', &mut line)?;
    let Ok(line) = std::str::from_utf8(&line) else { return Ok(Vec::new()) };
    if !line.bytes().all(is_py_printable) { return Ok(Vec::new()); }
    let mut cmd = shebang_split(line.trim());                    // trim == Python strip() on printable ASCII
    if cmd.len() >= 2 && cmd[0] == "/usr/bin/env" && cmd[1] == "-S" { cmd.drain(..2); }
    else if cmd.first().is_some_and(|c| c == "/usr/bin/env") { cmd.remove(0); }
    if cmd.len() == 1 && cmd[0] == "nix-shell" { return parse_nix(r, cmd); }
    Ok(cmd)
}
fn is_py_printable(b: u8) -> bool { matches!(b, 0x20..=0x7e | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) }

fn parse_nix<R: BufRead>(r: &mut R, mut cmd: Vec<String>) -> io::Result<Vec<String>> {
    loop {
        let mut m = [0u8; 2];
        if read_up_to(r, &mut m)? != 2 || m != *b"#!" { return Ok(cmd); }
        let mut l = Vec::new(); r.by_ref().take(MAX_SHEBANG_LINE).read_until(b'\n', &mut l)?;
        let Ok(l) = std::str::from_utf8(&l) else { return Ok(cmd) };
        if !l.bytes().all(is_py_printable) { return Ok(cmd); }
        let toks = shebang_split(l.trim());
        for w in toks.windows(2) { if w[0] == "-i" { cmd = vec![w[1].clone()]; } }   // last -i wins
    }
}

fn shebang_split(s: &str) -> Vec<String> {
    py_shlex_split(s).unwrap_or_else(|_| s.split(|c: char| c.is_ascii_whitespace() || c == '\x0b')
        .filter(|t| !t.is_empty()).map(str::to_owned).collect())     // == Python str.split() on printable ASCII
}

/// Python shlex.split(s): posix=True, whitespace_split=True, commenters='' ; whitespace = " \t\r\n".
pub fn py_shlex_split(s: &str) -> Result<Vec<String>, ShlexError> {
    #[derive(Clone, Copy)] enum Back { Word, Dq }
    #[derive(Clone, Copy)] enum St { Ws, Word, Sq, Dq, Esc(Back) }
    let ws = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    let (mut out, mut tok, mut quoted, mut st) = (Vec::new(), String::new(), false, St::Ws);
    for c in s.chars() {
        st = match st {
            St::Ws | St::Word if ws(c) => { if !tok.is_empty() || quoted { out.push(std::mem::take(&mut tok)); quoted = false; } St::Ws }
            St::Ws | St::Word if c == '\\' => St::Esc(Back::Word),
            St::Ws | St::Word if c == '\'' => { quoted = true; St::Sq }
            St::Ws | St::Word if c == '"' => { quoted = true; St::Dq }
            St::Ws | St::Word => { tok.push(c); St::Word }
            St::Sq if c == '\'' => St::Word,
            St::Sq => { tok.push(c); St::Sq }
            St::Dq if c == '"' => St::Word,
            St::Dq if c == '\\' => St::Esc(Back::Dq),
            St::Dq => { tok.push(c); St::Dq }
            St::Esc(Back::Word) => { tok.push(c); St::Word }
            St::Esc(Back::Dq) => { if c != '"' && c != '\\' { tok.push('\\'); } tok.push(c); St::Dq }
        };
    }
    match st {
        St::Sq | St::Dq => Err(ShlexError::NoClosingQuotation),
        St::Esc(_) => Err(ShlexError::NoEscapedCharacter),
        _ => { if !tok.is_empty() || quoted { out.push(tok); } Ok(out) }
    }
}
// Golden tests (Python 3 stdlib, verified): "/bin/sh #foo"→["/bin/sh","#foo"];
// "/usr/bin/env python # c"→["/usr/bin/env","python","#","c"]; "\"/usr/bin/my python\" -x"→["/usr/bin/my python","-x"];
// "a\\ b c"→["a b","c"]; "\"a\\\\b\\q\""→["a\\b\\q"]; "x\x0by"→["x\x0by"]; "x\ry"→["x","y"];
// "unterminated \"q"→Err⇒fallback ["unterminated","\"q"]; "trail\\"→Err⇒fallback ["trail\\"].
```
`parse_shebang_from_file(path)`: exec check per `ExecPolicy` (Windows ⇒ always), map `EINVAL` ⇒ empty,
`NotFound` ⇒ `does not exist.` error. Return `Vec<String>` (drop `ShebangTuple` or keep as alias).

**F7 (P0) `is_text`:** `pub fn is_text_bytes(head: &[u8]) -> bool` (const 256-entry table) plus
`is_text<R: Read>` that loops `read` until 1024 bytes or EOF, retrying `ErrorKind::Interrupted`.

**F8 (P1) Zero-alloc lookups:** all `get_*` return `TagSet` (Copy); no `HashSet`, no `RandomState`, no
lowercase `String` on the ASCII path; drop `smallvec`.

**F9 (P2) Hygiene & CI:** `#![forbid(unsafe_code)]`; remove dead `IdentifyError` variants, add
`#[non_exhaustive]`; gate unix-only tests with `#[cfg(unix)]`; CI matrix ubuntu/macos/windows; fix the
Windows `unused variable` warning; add golden tests generated from upstream identify's own test cases
(`identify/tests/identify_test.py` — read as data) and the probe cases in §4.5; fuzz `parse_shebang`/
`tags_from_filename` (no panic, bounded memory); CHANGELOG entry for 0.4.0/0.5.0; static Send/Sync asserts.

**F10 (P2, lefthook idea) optional `magic` feature:** MIME sniffing on the already-read 1 KiB head for
lefthook-style `file_types: [image/png]` predicates in snp extensions.

### 8.3 Release/rollout

1. cfgv 0.3.0 with C1–C12 (+C13 optional); file-identify 0.5.0 with F1–F9.
2. snp pins `cfgv = "0.3"`, `file-identify = { version = "0.5", default-features = false }`; during development
   `[patch.crates-io] cfgv = { git = "https://github.com/grok-rs/cfgv-rs", rev = "…" }`.
3. Add both crates to snp's conformance test-suite: run identical inputs through pre-commit (reference) and snp,
   diff outputs byte-for-byte (messages in §2.3/§3, tag sets, shebang argv).

---

## 9. Implementation notes for snp (Rust-oriented, with pitfalls)

1. **Never** feed configs through `serde_yaml`/`yaml_serde` for validation: YAML 1.2 vs 1.1 typing (`yes`, `on`,
   `017`, `1:20`, `1_000`, dates, `!!binary`), duplicate keys (PyYAML: last wins) and merge keys (`<<`) all
   diverge (§2.7). Build a SafeLoader-compatible resolver on a safe event parser and emit `cfgv::Value`.
2. Cap YAML nesting depth (e.g. 128–256) because `Value` drop/clone/eq/repr recurse; report a cfgv-style error.
3. All user-visible cfgv text must go through `Value::repr` (Python repr) — ids, repos, `found …`, `but got:`,
   regex errors. Golden-test with quotes, backslashes, tabs, `​`, floats (`1.0`, `1e+16`), ints.
4. One regex engine for both `check_regex` and matching (`files`, `exclude`, top-level ones). The Rust `regex`
   crate is **not** acceptable (rejects lookaround/backrefs/`\Z`/`a{`); decide engine per the regex research doc
   and plug it into cfgv via C2. Cache compiled regexes by pattern (`HashMap<Box<str>, Arc<Regex>>`).
5. Treat warnings as ordered data (`Cx.warn`) and print them exactly like pre-commit's logger
   (`[WARNING] …`) in validation order; validation stops at the first error.
6. Default `types: ['file']` means every candidate file needs classification ⇒ make it cheap: 1 `statx` per
   file, ≤1 `open`, 1 KiB stack buffer, `Copy` bitset; memoize with `OnceLock` per file (pre-commit memoizes
   with `functools.cache`).
7. Share the file table as `Arc<FileTable>`; pass `u32` indices to hook tasks; never clone path strings per hook.
8. Order: git order of filenames must be preserved through filtering (identify does not reorder; `TagSet`
   iteration order is irrelevant to output).
9. Windows parity: every regular file is `executable` (so unknown-name files get shebang-parsed); filenames are
   `/`-normalized before classification; symlinks may be checked out as text files (`core.symlinks=false`) ⇒
   only trust git mode 120000 when the worktree really has a symlink (use `lstat` on Windows).
10. `normalize_cmd` parity: the shebang argv prefix (including args and nix-shell resolution) of the resolved
    executable is prepended to every hook command — needs F6 exactness; otherwise hooks run with the wrong
    interpreter/args.
11. Never open non-regular files (FIFO hang); never read unbounded first lines (F6 cap); handle `EINTR`.
12. Errors from identification: pre-commit crashes (`An unexpected error has occurred: …`, exit 3) when
    `tags_from_path` raises; snp should surface the same class of failure rather than silently treating the
    file as binary (prek's choice).
13. Keep `ALL_TAGS` (323) as the single source of truth for `types` validation; embed `IDENTIFY_VERSION` and
    show it in `snp --version` to explain tag-availability differences.
14. Do not use `anyhow`'s alternate formatting on `ValidationError` (its `source()` chain would duplicate lines).
15. Avoid `HashSet<&str>` tag sets in hot paths; bitset ops are 6 words.
16. `#![forbid(unsafe_code)]` in snp crates; both user crates can satisfy it; avoid `yaml_serde`/`libyaml-rs`
    (c2rust `unsafe`) and `smallvec` where not needed.

---

## 10. Open questions

1. **Windows executable semantics**: replicate identify (`executable` for every file — pre-commit behavior) or
   use git index mode? Recommendation: parity by default (`ExecPolicy::PythonCompat`), opt-in `ModeBits`.
2. **Unix `access(2)` vs mode bits**: implement uid/gid-aware check (exact for non-ACL cases) or plain `0o111`?
   Recommendation: uid/gid-aware via a safe syscall wrapper, cached ids.
3. **Regex dialect** for `check_regex` (and matching): which Python-`re`-compatible engine/translation layer, and
   what to do when a backtracking engine hits its limit at match time (error vs no-match)?
4. **Message wording**: keep pre-commit-specific texts verbatim (`Try upgrading identify and pre-commit?`,
   `is not a valid python regex`, `pip install --upgrade pre-commit`) for byte-for-byte parity, or rebrand
   to snp? Recommendation: verbatim by default.
5. **PyYAML error texts** (`while scanning a simple key\n  in "<unicode string>", line …`) cannot realistically
   be reproduced byte-for-byte by a Rust parser — acceptable divergence?
6. **Shebang line cap** (64 KiB proposed) vs Python's unbounded `readline()`.
7. **FIFO/device handling**: deliberate divergence (no hang) — confirm.
8. **Identify version tracking**: always latest PyPI identify (recommended) vs pinning to the version a user's
   pre-commit would have installed; how fast to release file-identify after identify releases?
9. **Staged-files fast path** (trust git index mode, skip `lstat`): acceptable given `core.fileMode=false` and
   Windows exceptions, or always `lstat` for simplicity?
10. **cfgv `Value` numeric equality** (Python `1 == 1.0 == True`): implement Python semantics in `PartialEq`
    (affects `check_one_of`/conditions only; pre-commit uses strings) or leave strict?
11. Should cfgv keep `yaml_serde`/`serde_json` conversion features at all (lossy, unsafe deps), or move YAML
    loading into a separate safe crate (e.g. `pyyaml-compat`) owned by the user?
12. Unicode database for `str.isprintable` in `repr` (Python's UCD version varies by interpreter: 3.12 ⇒ 15.0,
    3.13 ⇒ 15.1, 3.14 ⇒ 16.0) — pick one and document.
