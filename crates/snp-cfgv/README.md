# snp-cfgv

A Rust port of Python [cfgv](https://github.com/asottile/cfgv) 3.5.0, the
validation engine of [pre-commit](https://pre-commit.com), as used by
[snp](https://github.com/grok-rs/snp): error messages, warnings, defaults and
`repr()` of every PyYAML value kind are **byte-identical** to Python cfgv on
CPython 3.12.

This crate is an internal part of snp (the library is named `cfgv`): it is
published with every snp release at snp's version and has no API stability
promise of its own. It was absorbed from the standalone
[`cfgv`](https://crates.io/crates/cfgv) crate 0.3.0 with its full history
(snp's ADR-116); use that crate in other projects.

## Usage

```rust
use cfgv::{CheckFn, ConditionValue, Schema, Validator, apply_defaults, validate, value_map};

let schema = Schema::map("Repository", Some("repo".into()), vec![
    Validator::required("repo", CheckFn::check_string()),
    Validator::conditional_with_absent(
        "rev",
        CheckFn::check_string(),
        "repo",
        ConditionValue::NotIn(vec!["local".into(), "meta".into()]),
        true,
    ),
    Validator::optional("fail_fast", CheckFn::check_bool(), false),
]);

let repo = value_map! {"repo" => "https://github.com/example/repo", "rev" => "v1.0"};
validate(&repo, &schema).unwrap();
let with_defaults = apply_defaults(&repo, &schema); // adds fail_fast: False

let err = validate(&value_map! {"repo" => "local", "rev" => "v1.0"}, &schema).unwrap_err();
assert_eq!(
    err.to_string(),
    "\n==> At Repository(repo='local')\n\
     =====> Expected rev to be absent when repo is any of ('local', 'meta'), found rev: 'v1.0'",
);
```

## Custom items and warnings

Python validators may log warnings while validating. cfgv threads a `Cx`
context through validation that collects warnings in order; validation
stops at the first error but keeps the warnings emitted before it.

```rust
use cfgv::{CheckFn, Cx, Map, MapItem, Schema, ValidationError, Validator, Value, validate_with, value_map};

/// Like pre-commit's `WarnMutableRev`: extends a built-in validator.
#[derive(Debug)]
struct WarnMutableRev(Validator);

impl MapItem for WarnMutableRev {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.0.check(dct, cx)?;
        if let Some(rev) = dct.get(&Value::from("rev")).and_then(Value::as_str) {
            if !rev.contains('.') {
                cx.warn(format!("The 'rev' field appears to be a mutable reference: {rev}"));
            }
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        self.0.key()
    }
}

let schema = Schema::map("Repository", Some("repo".into()), vec![
    Validator::custom(WarnMutableRev(Validator::optional_no_default("rev", CheckFn::check_string()))),
]);
let mut warnings = Vec::new();
let repo = value_map! {"repo" => "r", "rev" => "main"};
validate_with(&repo, &schema, &mut Cx::collect(&mut warnings)).unwrap();
assert_eq!(warnings, ["The 'rev' field appears to be a mutable reference: main"]);
```

`MapItem` also has `apply_default`/`remove_default` hooks (pre-commit's
stage and language migrations rewrite values there) and `key`/`check_fn`
for introspection, so one schema can be derived from another's items.

## Regular expressions

`check_regex` must accept exactly what Python's `re` accepts. Plug a validator
in with `CheckFn::check_regex_with(|pattern| ...)`.
A rejected `str` or `bytes` pattern gives `{pattern!r} is not a valid python
regex` (`bytes` patterns reach the validator decoded as Latin-1, as Python's
parser reads them). `check_regex_try_with` receives a `RegexPattern` (`Str`
or `Bytes`) and returns `Result<bool, ValidationError>`, so it can also
reproduce `re.compile` raising something other than `re.error`, which cfgv
does not catch:

```rust
use cfgv::{CheckFn, RegexPattern, ValidationError, Value};

let check = CheckFn::check_regex_try_with(|pattern| match pattern {
    RegexPattern::Str(p) if p.contains("{4294967296}") => Err(ValidationError::uncaught(
        "OverflowError",
        "the repetition number is too large",
    )),
    _ => Ok(true),
});
let err = check.check(&Value::from("a{4294967296}")).unwrap_err();
assert_eq!(err.to_string(), "OverflowError: the repetition number is too large");
```

As in Python, a value that is neither `str` nor `bytes` makes `check_regex`
raise an uncaught `TypeError`; pre-commit always runs `check_string` first.

## Exceptions that escape cfgv

Python validation code sometimes raises exceptions other than
`ValidationError`: `re.compile` (`OverflowError`, `TypeError`), `repr()` of
an integer with more than 4300 digits (`ValueError`), or pre-commit's own
check functions (`int()` in `check_min_version`). cfgv's `validate_context`
does not catch them, so they carry no `==> ...` context and
`load_from_filename` does not re-raise them as `exc_tp`; pre-commit reports
them as a crash.

`ValidationError::uncaught(type, message)` represents such an exception.
It travels through the same `Result`s as validation errors, ignores
`with_context`, exposes `uncaught_type()`, and displays like the last line
of a Python traceback:

```rust
use cfgv::{CheckFn, Schema, ValidationError, Validator, Value, validate, value_map};

let schema = Schema::map("Config", None, vec![Validator::optional_no_default(
    "minimum_pre_commit_version",
    CheckFn::custom(|v| match v.as_str().map(str::parse::<u32>) {
        Some(Ok(_)) => Ok(()),
        _ => Err(ValidationError::uncaught(
            "ValueError",
            format!("invalid literal for int() with base 10: {}", v.repr()),
        )),
    }),
)]);
let err = validate(&value_map! {"minimum_pre_commit_version" => "x"}, &schema).unwrap_err();
assert!(err.is_uncaught());
assert_eq!(err.to_string(), "ValueError: invalid literal for int() with base 10: 'x'");
```

cfgv itself raises uncaught errors where Python would: `TypeError` from a
bare `check_regex`, and CPython's `ValueError` wherever a message (or a
context Python formats eagerly, such as `At Hook(id=...)`) would contain
an integer beyond the 4300-digit limit.

## Differences from Python

* `NaN == NaN` (so `Eq` stays lawful for map keys), sets keep insertion
  order (CPython iterates them in hash order), and sorting values Python
  cannot order (`check_one_of` choices or unknown keys of mixed types)
  groups them by type instead of raising `TypeError`.
* Strings with lone surrogates cannot be represented (see the `Value` docs).
* `Value::repr` and `Display` cannot fail, so they write integers beyond
  CPython's 4300-digit limit in hexadecimal; the fallible
  `Value::try_repr`/`try_to_string` and validation itself fail like
  CPython.
* `check_regex` needs a validator of Python's `re` syntax, plugged in with
  `CheckFn::check_regex_with` or `CheckFn::check_regex_try_with` (snp uses
  `snp_pyre::cfgv_is_valid`).

## Testing

The Python-parity vectors in `tests/vectors/` are produced by CPython 3.12 and
the real cfgv 3.5.0 of snp's pre-commit 4.7.0 oracle (`cargo xtask
cfgv-vectors`); the fuzz targets are snp's `cfgv_*` targets.

## License

MIT (see `LICENSE`).
