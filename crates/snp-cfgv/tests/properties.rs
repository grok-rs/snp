//! Property tests: no panics on arbitrary input, round trips, and the
//! `Eq`/`Hash` contract.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    reason = "test code: failing loudly is the point"
)]

use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

use cfgv::{
    BigInt, CheckFn, ConditionValue, Cx, Date, DateTime, Map, Schema, TzOffset, Validator, Value,
    py, validate_with,
};
use proptest::prelude::*;

fn hash(v: &Value) -> u64 {
    BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
}

fn big_int() -> impl Strategy<Value = BigInt> {
    ("-?[1-9][0-9]{0,40}").prop_map(|s| s.parse().expect("valid digits"))
}

fn date() -> impl Strategy<Value = Date> {
    (1i64..=9999, 1u32..=12, 1u32..=28).prop_map(|(y, m, d)| Date::new(y, m, d).expect("valid"))
}

fn datetime() -> impl Strategy<Value = DateTime> {
    (
        date(),
        0u32..24,
        0u32..60,
        0u32..60,
        prop_oneof![Just(0u32), 0u32..1_000_000],
        prop::option::of(-86_399i64..86_400),
    )
        .prop_map(|(d, h, m, s, us, tz)| {
            let tz = tz.map(|t| TzOffset::from_seconds(t).expect("in range"));
            DateTime::new(d, h, m, s, us, tz).expect("valid")
        })
}

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        big_int().prop_map(Value::from),
        any::<f64>().prop_map(Value::Float),
        any::<String>().prop_map(Value::String),
        "[a-z'\"\\\\ \t\n.]{0,8}".prop_map(Value::String),
        prop::collection::vec(any::<u8>(), 0..8).prop_map(Value::Bytes),
        date().prop_map(Value::Date),
        datetime().prop_map(Value::DateTime),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(3, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::List),
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Tuple),
            prop::collection::vec((inner.clone(), inner.clone()), 0..6)
                .prop_map(|pairs| Value::Map(pairs.into_iter().collect())),
            prop::collection::vec(inner, 0..6)
                .prop_map(|items| Value::Set(items.into_iter().collect())),
        ]
    })
}

/// Parses a Python `str` literal as written by `repr()`.
fn unrepr_str(lit: &str) -> Option<String> {
    let quote = lit.chars().next()?;
    let body = lit.strip_prefix(quote)?.strip_suffix(quote)?;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let hex = |n: usize, chars: &mut std::str::Chars<'_>| {
            let digits: String = chars.take(n).collect();
            u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
        };
        match chars.next()? {
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'x' => out.push(hex(2, &mut chars)?),
            'u' => out.push(hex(4, &mut chars)?),
            'U' => out.push(hex(8, &mut chars)?),
            c @ ('\\' | '\'' | '"') => out.push(c),
            _ => return None,
        }
    }
    Some(out)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn repr_and_str_never_panic_and_eq_is_reflexive(v in value()) {
        let repr = v.repr();
        prop_assert!(!repr.is_empty());
        let _ = v.to_string();
        let _ = v.type_name();
        let copy = v.clone();
        prop_assert_eq!(&copy, &v);
        prop_assert_eq!(hash(&copy), hash(&v));
    }

    #[test]
    fn str_repr_round_trips(s in any::<String>()) {
        let repr = py::repr_str(&s);
        prop_assert_eq!(unrepr_str(&repr), Some(s.clone()));
        // The output never contains raw control characters.
        prop_assert!(!repr.chars().any(char::is_control));
        // A quote is only escaped when it is the delimiter.
        let quote = repr.chars().next().unwrap_or('\'');
        prop_assert!(quote == '\'' || (s.contains('\'') && !s.contains('"')));
    }

    #[test]
    fn float_repr_round_trips(x in any::<f64>()) {
        let repr = py::repr_float(x);
        if x.is_nan() {
            prop_assert_eq!(repr, "nan");
        } else {
            let parsed: f64 = repr.parse().expect("Rust parses Python float reprs");
            prop_assert_eq!(parsed.to_bits(), x.to_bits());
            prop_assert!(repr.contains('.') || repr.contains('e') || repr.ends_with("inf"));
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss, reason = "the rounding is what is tested")]
    fn numeric_equality_is_exact(i in any::<i64>(), f in any::<f64>()) {
        let as_float = Value::Float(i as f64);
        // `i as f64` rounds; Python compares exactly.
        #[allow(clippy::cast_possible_truncation)]
        let exact = (i as f64).is_finite() && (i as f64) as i128 == i128::from(i);
        prop_assert_eq!(Value::Int(i) == as_float, exact);
        if exact {
            prop_assert_eq!(hash(&Value::Int(i)), hash(&as_float));
        }
        let a = Value::Float(f);
        let b = Value::from(BigInt::from(i));
        if a == b {
            prop_assert_eq!(hash(&a), hash(&b));
        }
    }

    #[test]
    fn map_equality_and_hash_ignore_order(pairs in prop::collection::vec((any::<i32>(), any::<String>()), 0..8)) {
        let forward: Map = pairs.iter().map(|(k, v)| (Value::from(*k), Value::from(v.as_str()))).collect();
        let backward: Map = forward.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect();
        let (a, b) = (Value::Map(forward), Value::Map(backward));
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn utf8_decoding_agrees_with_std(bytes in prop::collection::vec(any::<u8>(), 0..32)) {
        match (py::decode_utf8(&bytes), std::str::from_utf8(&bytes)) {
            (Ok(a), Ok(b)) => prop_assert_eq!(a, b),
            (Err(e), Err(std_err)) => {
                prop_assert_eq!(e.start(), std_err.valid_up_to());
                prop_assert!(e.start() < e.end() && e.end() <= bytes.len());
                prop_assert_eq!(Some(e.first_byte()), bytes.get(e.start()).copied());
                prop_assert!(e.to_string().starts_with("'utf-8' codec can't decode byte"));
            }
            _ => prop_assert!(false, "disagreement on {:?}", bytes),
        }
    }

    #[test]
    fn universal_newlines_matches_reference(s in "[a\r\n]{0,16}") {
        let expected = s.replace("\r\n", "\n").replace('\r', "\n");
        prop_assert_eq!(py::universal_newlines(&s), expected);
    }

    #[test]
    fn big_int_agrees_with_i128(a in any::<i128>(), b in any::<i128>()) {
        let (x, y) = (BigInt::from(a), BigInt::from(b));
        prop_assert_eq!(x.to_i128(), Some(a));
        prop_assert_eq!(x.cmp(&y), a.cmp(&b));
        prop_assert_eq!(x.to_string().parse::<BigInt>().ok(), Some(x.clone()));
        prop_assert_eq!(Value::from(x.clone()), Value::from(a));
        let hex = format!("{:x}", a.unsigned_abs());
        let parsed = BigInt::from_str_radix(&hex, 16).expect("hex digits");
        prop_assert_eq!(parsed.to_string(), a.unsigned_abs().to_string());
    }

    #[test]
    fn big_int_matches_u128_in_every_radix(n in any::<u128>(), negative in any::<bool>(), radix in 2u32..=36) {
        // Reference digits of `n` in `radix`.
        let mut digits = Vec::new();
        let mut rest = n;
        loop {
            digits.push(char::from_digit(u32::try_from(rest % u128::from(radix)).unwrap(), radix).unwrap());
            rest /= u128::from(radix);
            if rest == 0 {
                break;
            }
        }
        let text: String = digits.iter().rev().collect();
        let signed = if negative { format!("-{text}") } else { text.to_uppercase() };
        let big = BigInt::from_str_radix(&signed, radix).unwrap();
        let expected = if negative && n != 0 { format!("-{n}") } else { n.to_string() };
        prop_assert_eq!(big.to_string(), expected.clone());
        prop_assert_eq!(big.to_decimal_string().unwrap(), expected);
        prop_assert_eq!(big.is_negative(), negative && n != 0);
        prop_assert_eq!(big.bit_length(), u64::from(128 - n.leading_zeros()));
        if !negative {
            prop_assert_eq!(big.to_u128(), Some(n));
            prop_assert_eq!(big, BigInt::from(n));
        }
    }

    #[test]
    fn big_hex_round_trips_and_orders_like_text(
        a in "[1-9a-f][0-9a-f]{0,300}",
        b in "[1-9a-f][0-9a-f]{0,300}",
        zeros in 0usize..40,
    ) {
        let x = BigInt::from_str_radix(&format!("{}{a}", "0".repeat(zeros)), 16).unwrap();
        let y = BigInt::from_str_radix(&b, 16).unwrap();
        prop_assert_eq!(format!("{x:x}"), a.clone());
        prop_assert_eq!(format!("{:#X}", y), format!("0x{}", b.to_uppercase()));
        let by_text = a.len().cmp(&b.len()).then_with(|| a.cmp(&b));
        prop_assert_eq!(x.cmp(&y), by_text);
        prop_assert_eq!(x == y, a == b);
        let (vx, vy) = (Value::from(x.clone()), Value::from(y.clone()));
        prop_assert_eq!(vx == vy, a == b);
        if vx == vy {
            prop_assert_eq!(hash(&vx), hash(&vy));
        }
        // Decimal text round trips (well within the 4300-digit limit).
        let decimal = x.to_decimal_string().unwrap();
        prop_assert_eq!(decimal.parse::<BigInt>().unwrap(), x);
    }

    #[test]
    fn big_int_parsing_never_panics(s in "[-+]?[0-9a-zA-Z_ ]{0,60}", radix in 0u32..40) {
        let _ = BigInt::from_str_radix(&s, radix);
    }

    #[test]
    fn huge_floats_equal_their_integer(exp in 64u32..960, mantissa in 1u64..(1 << 53)) {
        #[allow(clippy::cast_precision_loss, reason = "the mantissa fits in 53 bits")]
        let f = (mantissa as f64) * 2f64.powi(i32::try_from(exp).unwrap());
        let text = format!("{f:.0}");
        let big = Value::from(text.parse::<BigInt>().unwrap());
        let float = Value::Float(f);
        prop_assert_eq!(&big, &float);
        prop_assert_eq!(hash(&big), hash(&float));
        let next = Value::from(format!("{text}1").parse::<BigInt>().unwrap());
        prop_assert_ne!(&next, &float);
    }

    #[test]
    fn aware_datetimes_compare_by_instant(dt in datetime(), shift_hours in -12i64..12) {
        let Some(tz) = dt.tz() else { return Ok(()); };
        let hour = i64::from(dt.hour()) + shift_hours;
        let offset = i64::from(tz.seconds()) + shift_hours * 3600;
        if !(0..24).contains(&hour) || offset.abs() >= 86_400 {
            return Ok(());
        }
        let shifted = DateTime::new(
            dt.date(),
            u32::try_from(hour).unwrap_or(0),
            u32::from(dt.minute()),
            u32::from(dt.second()),
            dt.microsecond(),
            Some(TzOffset::from_seconds(offset).expect("in range")),
        ).expect("valid");
        let (a, b) = (Value::from(dt), Value::from(shifted));
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn validation_never_panics_and_defaults_are_idempotent(v in prop_oneof![value(), config_like()]) {
        let schema = config_schema();
        let mut warnings = Vec::new();
        if validate_with(&v, &schema, &mut Cx::collect(&mut warnings)).is_ok() {
            let applied = schema.apply_defaults(&v);
            prop_assert!(validate_with(&applied, &schema, &mut Cx::discard()).is_ok());
            prop_assert_eq!(&schema.apply_defaults(&applied), &applied);
            let removed = schema.remove_defaults(&applied);
            prop_assert_eq!(&schema.apply_defaults(&removed), &applied);
        }
        // The same holds for data that never validates.
        let _ = schema.apply_defaults_owned(v.clone());
        let _ = schema.remove_defaults_owned(v);
    }
}

fn config_schema() -> Schema {
    let hook = Schema::map(
        "Hook",
        Some("id".into()),
        vec![
            Validator::required("id", CheckFn::check_string()),
            Validator::optional(
                "args",
                CheckFn::check_array(CheckFn::check_string()),
                Vec::<Value>::new(),
            ),
            Validator::optional("always_run", CheckFn::check_bool(), false),
            Validator::optional("n", CheckFn::check_one_of([1, 2, 3]), 1),
            Validator::optional_no_default(
                "files",
                CheckFn::check_regex_with(|p| !p.contains('(')),
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
            Validator::conditional_recurse(
                "hooks",
                Schema::array(hook),
                "repo",
                ConditionValue::Not(Value::Null),
            ),
            Validator::warn_additional_keys(
                vec!["repo".into(), "rev".into(), "hooks".into()],
                |extra, _, _, cx| {
                    cx.warn(format!("extra: {}", extra.len()));
                },
            ),
        ],
    );
    Schema::map(
        "Config",
        None,
        vec![
            Validator::optional_recurse("repos", Schema::array(repo), Vec::<Value>::new()),
            Validator::optional("fail_fast", CheckFn::check_bool(), false),
            Validator::optional_recurse(
                "env",
                Schema::key_value_map(
                    "Env",
                    CheckFn::check_string(),
                    Schema::map(
                        "Var",
                        None,
                        vec![Validator::optional("v", CheckFn::check_any(), Value::Null)],
                    ),
                ),
                cfgv::value_map! {},
            ),
            Validator::no_additional_keys(vec!["repos".into(), "fail_fast".into(), "env".into()]),
        ],
    )
}

/// Values shaped like configs for `config_schema`, valid or nearly valid.
fn config_like() -> impl Strategy<Value = Value> {
    let string = || {
        prop_oneof![
            3 => "[a-z(]{0,3}".prop_map(Value::String),
            1 => leaf(),
        ]
    };
    let hook = (
        string(),
        prop::option::of(prop::collection::vec(string(), 0..3)),
        prop::option::of(prop_oneof![any::<bool>().prop_map(Value::Bool), leaf()]),
        prop::option::of(prop_oneof![(0i64..4).prop_map(Value::Int), leaf()]),
        prop::option::of(string()),
    )
        .prop_map(|(id, args, always_run, n, files)| {
            let mut map = Map::new();
            map.insert("id".into(), id);
            let optional = [
                ("args", args.map(Value::List)),
                ("always_run", always_run),
                ("n", n),
                ("files", files),
            ];
            for (key, value) in optional {
                if let Some(value) = value {
                    map.insert(key.into(), value);
                }
            }
            Value::Map(map)
        });
    let repo = (
        prop_oneof![
            Just(Value::from("local")),
            Just(Value::from("meta")),
            string()
        ],
        prop::option::of(string()),
        prop::option::of(prop::collection::vec(hook, 0..3)),
        prop::option::of(leaf()),
    )
        .prop_map(|(repo, rev, hooks, extra)| {
            let mut map = Map::new();
            map.insert("repo".into(), repo);
            if let Some(rev) = rev {
                map.insert("rev".into(), rev);
            }
            if let Some(hooks) = hooks {
                map.insert("hooks".into(), Value::List(hooks));
            }
            if let Some(extra) = extra {
                map.insert("extra".into(), extra);
            }
            Value::Map(map)
        });
    (
        prop::option::of(prop::collection::vec(repo, 0..3)),
        prop::option::of(prop_oneof![any::<bool>().prop_map(Value::Bool), leaf()]),
        prop::option::of(prop::collection::vec(
            (string(), prop::option::of(leaf())),
            0..3,
        )),
    )
        .prop_map(|(repos, fail_fast, env)| {
            let mut map = Map::new();
            if let Some(repos) = repos {
                map.insert("repos".into(), Value::List(repos));
            }
            if let Some(fail_fast) = fail_fast {
                map.insert("fail_fast".into(), fail_fast);
            }
            if let Some(env) = env {
                let vars = env.into_iter().map(|(k, v)| {
                    let mut var = Map::new();
                    if let Some(v) = v {
                        var.insert("v".into(), v);
                    }
                    (k, Value::Map(var))
                });
                map.insert("env".into(), Value::Map(vars.collect()));
            }
            Value::Map(map)
        })
}
