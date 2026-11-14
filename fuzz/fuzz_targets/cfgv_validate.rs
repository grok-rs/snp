//! Arbitrary `Value`s against a pre-commit-like schema: validation,
//! defaults, formatting, typed conversion and the `Eq`/`Hash` contract must
//! never panic or disagree.
//! Regexes are checked with `snp_pyre::cfgv_is_valid`, as snp-config does (ADR-116 dropped the `regex`-crate
//! backed `check_regex()`).
#![no_main]

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
use std::sync::LazyLock;

use arbitrary::{Result, Unstructured};
use cfgv::{
    BigInt, CheckFn, ConditionValue, Cx, Date, DateTime, Map, Schema, Set, TzOffset, Validator,
    Value, ValueType, apply_defaults, remove_defaults, validate_with,
};
use libfuzzer_sys::fuzz_target;

const KEYS: &[&str] = &[
    "repos",
    "repo",
    "rev",
    "hooks",
    "id",
    "name",
    "entry",
    "language",
    "files",
    "exclude",
    "types",
    "args",
    "stages",
    "always_run",
    "fail_fast",
    "minimum_pre_commit_version",
    "ci",
    "local",
    "meta",
    "pre-commit",
    "",
];

fn string(u: &mut Unstructured<'_>) -> Result<String> {
    if u.ratio(1, 2)? {
        Ok((*u.choose(KEYS)?).to_owned())
    } else {
        u.arbitrary()
    }
}

fn value(u: &mut Unstructured<'_>, depth: u32) -> Result<Value> {
    let leaf = depth == 0 || u.is_empty();
    let kind = if leaf {
        u.int_in_range(0..=8)?
    } else {
        u.int_in_range(0..=13)?
    };
    Ok(match kind {
        0 => Value::Null,
        1 => Value::Bool(u.arbitrary()?),
        2 => Value::Int(u.arbitrary()?),
        3 => {
            let digits: String = u.arbitrary()?;
            let radix = u.int_in_range(2..=36)?;
            BigInt::from_str_radix(&digits, radix).map_or(Value::Null, Value::from)
        }
        4 => {
            // Occasionally beyond CPython's 4300-digit limit.
            let len = u.int_in_range(1..=5000)?;
            let digit = u.choose(&["f", "1", "7"])?;
            Value::from(
                BigInt::from_str_radix(&digit.repeat(len), 16).unwrap_or_else(|_| BigInt::from(0)),
            )
        }
        5 => Value::Float(f64::from_bits(u.arbitrary()?)),
        6 => Value::String(string(u)?),
        7 => Value::Bytes(u.arbitrary()?),
        8 => {
            let date = Date::new(
                u.int_in_range(1..=9999)?,
                u.int_in_range(1..=12)?,
                u.int_in_range(1..=28)?,
            );
            match date {
                Ok(d) if u.ratio(1, 2)? => Value::Date(d),
                Ok(d) => {
                    let tz = if u.arbitrary()? {
                        TzOffset::from_seconds(u.int_in_range(-86_399..=86_399)?).ok()
                    } else {
                        None
                    };
                    DateTime::new(
                        d,
                        u.int_in_range(0..=23)?,
                        u.int_in_range(0..=59)?,
                        0,
                        0,
                        tz,
                    )
                    .map_or(Value::Null, Value::DateTime)
                }
                Err(_) => Value::Null,
            }
        }
        9 => Value::List(items(u, depth)?),
        10 => Value::Tuple(items(u, depth)?),
        11 => Value::Set(items(u, depth)?.into_iter().collect::<Set>()),
        _ => {
            let mut map = Map::new();
            for _ in 0..u.int_in_range(0..=6)? {
                let key = if u.ratio(4, 5)? {
                    Value::String(string(u)?)
                } else {
                    value(u, 0)?
                };
                map.insert(key, value(u, depth - 1)?);
            }
            Value::Map(map)
        }
    })
}

fn items(u: &mut Unstructured<'_>, depth: u32) -> Result<Vec<Value>> {
    (0..u.int_in_range(0..=5)?)
        .map(|_| value(u, depth - 1))
        .collect()
}

static SCHEMA: LazyLock<Schema> = LazyLock::new(|| {
    let hook = Schema::map(
        "Hook",
        Some("id".into()),
        vec![
            Validator::required("id", CheckFn::check_string()),
            Validator::optional("name", CheckFn::check_string(), "x"),
            Validator::optional(
                "files",
                CheckFn::check_and([CheckFn::check_string(), CheckFn::check_regex_with(snp_pyre::cfgv_is_valid)]),
                "",
            ),
            Validator::optional("exclude", CheckFn::check_regex_with(snp_pyre::cfgv_is_valid), "^$"),
            Validator::optional(
                "stages",
                CheckFn::check_array(CheckFn::check_one_of(["pre-commit", "pre-push", "manual"])),
                Value::List(Vec::new()),
            ),
            Validator::optional(
                "args",
                CheckFn::check_array(CheckFn::check_string()),
                Value::List(Vec::new()),
            ),
            Validator::optional("always_run", CheckFn::check_bool(), false),
            Validator::optional_no_default("priority", CheckFn::check_int()),
            Validator::optional_no_default("ci", CheckFn::check_type(ValueType::Map, None)),
            Validator::no_additional_keys(
                [
                    "id",
                    "name",
                    "files",
                    "exclude",
                    "stages",
                    "args",
                    "always_run",
                    "priority",
                    "ci",
                ]
                .into_iter()
                .map(Value::from)
                .collect(),
            ),
        ],
    );
    let repo = Schema::map(
        "Repository",
        Some("repo".into()),
        vec![
            Validator::required("repo", CheckFn::check_string()),
            Validator::conditional_with_absent(
                "rev",
                CheckFn::check_string(),
                "repo",
                ConditionValue::NotIn(vec!["local".into(), "meta".into()]),
                true,
            ),
            Validator::conditional_recurse_with_absent(
                "hooks",
                Schema::array(hook.clone()),
                "repo",
                ConditionValue::Exact("local".into()),
                false,
            ),
            Validator::optional_recurse(
                "defaults",
                Schema::key_value_map("Env", CheckFn::check_any(), hook),
                Map::new(),
            ),
            Validator::warn_additional_keys(
                vec!["repo".into(), "rev".into(), "hooks".into()],
                |extra, _, _, cx| {
                    cx.warn(format!("{} extra", extra.len()));
                },
            ),
        ],
    );
    Schema::map(
        "Config",
        None,
        vec![
            Validator::required_recurse("repos", Schema::array_nonempty(repo)),
            Validator::optional("fail_fast", CheckFn::check_bool(), false),
            Validator::optional_no_default(
                "minimum_pre_commit_version",
                CheckFn::check_regex_with(|p| !p.contains('(')),
            ),
        ],
    )
});

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct Config {
    repos: Vec<Repo>,
    #[serde(default)]
    fail_fast: bool,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code, clippy::struct_field_names)]
struct Repo {
    repo: String,
    rev: Option<String>,
    #[serde(default)]
    hooks: Vec<Hook>,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct Hook {
    id: String,
    name: String,
    args: Vec<String>,
    always_run: bool,
    priority: Option<i64>,
}

fn hash(v: &Value) -> u64 {
    BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let Ok(v) = value(&mut u, 6) else {
        return;
    };
    // Formatting never panics; the fallible forms agree with each other.
    let _ = v.repr();
    let _ = v.to_string();
    assert_eq!(v.try_repr().is_ok(), v.try_to_string().is_ok());
    // Eq/Hash contract.
    let copy = v.clone();
    assert_eq!(v, copy);
    assert_eq!(hash(&v), hash(&copy));

    let mut warnings = Vec::new();
    let result = validate_with(&v, &SCHEMA, &mut Cx::collect(&mut warnings));
    match result {
        Ok(()) => {
            let applied = apply_defaults(&v, &SCHEMA);
            assert!(validate_with(&applied, &SCHEMA, &mut Cx::discard()).is_ok());
            let removed = remove_defaults(&applied, &SCHEMA);
            assert_eq!(apply_defaults(&removed, &SCHEMA), applied);
            let _ = cfgv::from_value::<Config>(&applied);
        }
        Err(err) => {
            let _ = err.to_string();
            if err.is_uncaught() {
                assert_eq!(err.contexts().len(), 0);
            }
        }
    }
});
