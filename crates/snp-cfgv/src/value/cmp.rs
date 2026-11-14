//! Python-compatible equality, hashing and sort order for [`Value`].

use std::cmp::Ordering;
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hash, Hasher};

use super::{BigInt, Value};

/// A numeric view of `bool`, `int` and `float` values.
#[derive(Clone, Copy)]
enum Num<'a> {
    Int(i64),
    Big(&'a BigInt),
    Float(f64),
}

impl Value {
    fn as_num(&self) -> Option<Num<'_>> {
        match self {
            Self::Bool(b) => Some(Num::Int(i64::from(*b))),
            Self::Int(i) => Some(Num::Int(*i)),
            Self::BigInt(b) => Some(b.to_i64().map_or(Num::Big(b), Num::Int)),
            Self::Float(f) => Some(Num::Float(*f)),
            _ => None,
        }
    }
}

/// `2**63` as an `f64` (exact).
const TWO_POW_63: f64 = f64::from_bits(0x43E0_0000_0000_0000);

/// The `i64` equal to `f`, if `f` is integral and in range.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "exact: `f` is integral and within the `i64` range"
)]
fn f64_as_i64(f: f64) -> Option<i64> {
    (f.is_finite() && f.fract() == 0.0 && (-TWO_POW_63..TWO_POW_63).contains(&f))
        .then_some(f as i64)
}

fn num_eq(a: Num<'_>, b: Num<'_>) -> bool {
    match (a, b) {
        (Num::Int(a), Num::Int(b)) => a == b,
        (Num::Int(i), Num::Float(f)) | (Num::Float(f), Num::Int(i)) => f64_as_i64(f) == Some(i),
        // A `Num::Big` never fits in an `i64` (see `as_num`).
        (Num::Int(_), Num::Big(_)) | (Num::Big(_), Num::Int(_)) => false,
        (Num::Big(a), Num::Big(b)) => a == b,
        (Num::Big(b), Num::Float(f)) | (Num::Float(f), Num::Big(b)) => {
            BigInt::from_integral_f64(f).is_some_and(|x| x == *b)
        }
        (Num::Float(a), Num::Float(b)) => a == b || (a.is_nan() && b.is_nan()),
    }
}

const TAG_NULL: u8 = 0;
const TAG_INT: u8 = 1;
const TAG_BIG: u8 = 2;
const TAG_FLOAT: u8 = 3;
const TAG_NAN: u8 = 4;
const TAG_STR: u8 = 5;
const TAG_BYTES: u8 = 6;
const TAG_LIST: u8 = 7;
const TAG_TUPLE: u8 = 8;
const TAG_MAP: u8 = 9;
const TAG_SET: u8 = 10;
const TAG_DATE: u8 = 11;
const TAG_DATETIME: u8 = 12;

fn hash_num<H: Hasher>(n: Num<'_>, state: &mut H) {
    match n {
        Num::Int(i) => {
            state.write_u8(TAG_INT);
            i.hash(state);
        }
        Num::Big(b) => {
            state.write_u8(TAG_BIG);
            b.hash(state);
        }
        Num::Float(f) => {
            if f.is_nan() {
                state.write_u8(TAG_NAN);
            } else if let Some(i) = f64_as_i64(f) {
                state.write_u8(TAG_INT);
                i.hash(state);
            } else if let Some(big) = BigInt::from_integral_f64(f) {
                state.write_u8(TAG_BIG);
                big.hash(state);
            } else {
                state.write_u8(TAG_FLOAT);
                f.to_bits().hash(state);
            }
        }
    }
}

/// Order-independent hash of a collection: the wrapping sum of the
/// individual hashes, computed with a fixed-key hasher.
fn unordered_hash<I, T>(items: I) -> u64
where
    I: IntoIterator<Item = T>,
    T: Hash,
{
    let build = BuildHasherDefault::<DefaultHasher>::default();
    items
        .into_iter()
        .fold(0u64, |acc, item| acc.wrapping_add(build.hash_one(item)))
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        if let (Some(a), Some(b)) = (self.as_num(), other.as_num()) {
            return num_eq(a, b);
        }
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::List(a), Self::List(b)) | (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (Self::Set(a), Self::Set(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(n) = self.as_num() {
            return hash_num(n, state);
        }
        match self {
            Self::String(s) => {
                state.write_u8(TAG_STR);
                s.hash(state);
            }
            Self::Bytes(b) => {
                state.write_u8(TAG_BYTES);
                b.hash(state);
            }
            Self::List(l) => {
                state.write_u8(TAG_LIST);
                l.hash(state);
            }
            Self::Tuple(t) => {
                state.write_u8(TAG_TUPLE);
                t.hash(state);
            }
            Self::Map(m) => {
                state.write_u8(TAG_MAP);
                state.write_usize(m.len());
                state.write_u64(unordered_hash(m.iter()));
            }
            Self::Set(s) => {
                state.write_u8(TAG_SET);
                state.write_usize(s.len());
                state.write_u64(unordered_hash(s.iter()));
            }
            Self::Date(d) => {
                state.write_u8(TAG_DATE);
                d.hash(state);
            }
            Self::DateTime(d) => {
                state.write_u8(TAG_DATETIME);
                d.hash(state);
            }
            // Numbers were handled above.
            Self::Null | Self::Bool(_) | Self::Int(_) | Self::BigInt(_) | Self::Float(_) => {
                state.write_u8(TAG_NULL);
            }
        }
    }
}

// --- Sorting ---------------------------------------------------------------

/// Exact comparison of a (possibly huge) integer with a float.
fn cmp_big_f64(b: &BigInt, f: f64) -> Ordering {
    if f.is_nan() {
        return Ordering::Less;
    }
    if f.is_infinite() {
        return if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let trunc = f.trunc();
    let trunc_big = BigInt::from_integral_f64(trunc).unwrap_or_else(|| BigInt::from(0));
    b.cmp(&trunc_big).then_with(|| {
        // `b` is an integer equal to `trunc(f)`.
        if f > trunc {
            Ordering::Less
        } else if f < trunc {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    })
}

fn cmp_i64_f64(i: i64, f: f64) -> Ordering {
    if f.is_nan() {
        return Ordering::Less;
    }
    if f >= TWO_POW_63 {
        return Ordering::Less;
    }
    if f < -TWO_POW_63 {
        return Ordering::Greater;
    }
    let trunc = f.trunc();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::as_conversions,
        reason = "exact: `trunc` is integral and within the `i64` range"
    )]
    let trunc_i = trunc as i64;
    i.cmp(&trunc_i).then_with(|| {
        if f > trunc {
            Ordering::Less
        } else if f < trunc {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    })
}

/// Numeric order; NaN sorts after every other number.
fn num_cmp(a: Num<'_>, b: Num<'_>) -> Ordering {
    match (a, b) {
        (Num::Int(a), Num::Int(b)) => a.cmp(&b),
        (Num::Int(i), Num::Float(f)) => cmp_i64_f64(i, f),
        (Num::Float(f), Num::Int(i)) => cmp_i64_f64(i, f).reverse(),
        (Num::Int(_), Num::Big(b)) => {
            if b.is_negative() {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Num::Big(b), Num::Int(_)) => {
            if b.is_negative() {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Num::Big(a), Num::Big(b)) => a.cmp(b),
        (Num::Big(b), Num::Float(f)) => cmp_big_f64(b, f),
        (Num::Float(f), Num::Big(b)) => cmp_big_f64(b, f).reverse(),
        (Num::Float(a), Num::Float(b)) => match (a.is_nan(), b.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        },
    }
}

/// Rank of each group of mutually comparable types.
const fn type_rank(v: &Value) -> u8 {
    match v {
        Value::Null => 0,
        Value::Bool(_) | Value::Int(_) | Value::BigInt(_) | Value::Float(_) => 1,
        Value::String(_) => 2,
        Value::Bytes(_) => 3,
        Value::List(_) => 4,
        Value::Tuple(_) => 5,
        Value::Map(_) => 6,
        Value::Set(_) => 7,
        Value::Date(_) => 8,
        Value::DateTime(_) => 9,
    }
}

fn seq_cmp<'a>(
    a: impl IntoIterator<Item = &'a Value>,
    b: impl IntoIterator<Item = &'a Value>,
) -> Ordering {
    let mut b = b.into_iter();
    for x in a {
        match b.next() {
            None => return Ordering::Greater,
            Some(y) => match py_cmp(x, y) {
                Ordering::Equal => {}
                other => return other,
            },
        }
    }
    if b.next().is_some() {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// A total order that agrees with Python's `<` wherever Python can compare
/// the values: numbers numerically, strings by code point, bytes, lists and
/// tuples lexicographically, dates chronologically. Values Python cannot
/// order (mixed types, dicts, `None`) are grouped by type instead of raising
/// `TypeError`.
pub(crate) fn py_cmp(a: &Value, b: &Value) -> Ordering {
    if let (Some(x), Some(y)) = (a.as_num(), b.as_num()) {
        return num_cmp(x, y);
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bytes(x), Value::Bytes(y)) => x.cmp(y),
        (Value::List(x), Value::List(y)) | (Value::Tuple(x), Value::Tuple(y)) => seq_cmp(x, y),
        (Value::Map(x), Value::Map(y)) => x
            .len()
            .cmp(&y.len())
            .then_with(|| seq_cmp(x.keys(), y.keys()))
            .then_with(|| seq_cmp(x.values(), y.values())),
        (Value::Set(x), Value::Set(y)) => x.len().cmp(&y.len()).then_with(|| seq_cmp(x, y)),
        (Value::Date(x), Value::Date(y)) => x.cmp(y),
        (Value::DateTime(x), Value::DateTime(y)) => x.py_cmp(*y),
        _ => type_rank(a).cmp(&type_rank(b)),
    }
}

/// Stable sort in Python's `sorted()` order (see [`py_cmp`]).
pub(crate) fn py_sort<T: std::borrow::Borrow<Value>>(values: &mut [T]) {
    values.sort_by(|a, b| py_cmp(a.borrow(), b.borrow()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(v: &Value) -> u64 {
        BuildHasherDefault::<DefaultHasher>::default().hash_one(v)
    }

    fn big(s: &str) -> Value {
        Value::BigInt(s.parse().unwrap())
    }

    #[test]
    fn numeric_equality_matches_python() {
        let one = [
            Value::Bool(true),
            Value::Int(1),
            Value::Float(1.0),
            big("1"),
        ];
        for a in &one {
            for b in &one {
                assert_eq!(a, b);
                assert_eq!(hash(a), hash(b));
            }
        }
        assert_eq!(Value::Float(0.0), Value::Float(-0.0));
        assert_eq!(hash(&Value::Float(0.0)), hash(&Value::Float(-0.0)));
        assert_eq!(Value::Float(-0.0), Value::Bool(false));
        assert_ne!(Value::Int(1), Value::Float(1.5));
        // 2**53 + 1 is not representable: Python compares exactly.
        assert_ne!(
            Value::Int(9_007_199_254_740_993),
            Value::Float(9_007_199_254_740_992.0)
        );
        assert_eq!(Value::Float(1e20), big("100000000000000000000"));
        assert_eq!(
            hash(&Value::Float(1e20)),
            hash(&big("100000000000000000000"))
        );
        assert_ne!(Value::Float(1e20), big("100000000000000000001"));
        assert_eq!(Value::Float(f64::NAN), Value::Float(f64::NAN));
        assert_ne!(Value::Int(1), Value::from("1"));
        assert_ne!(Value::List(vec![]), Value::Tuple(vec![]));
    }

    #[test]
    fn map_hash_is_order_independent() {
        let a = crate::value_map! {"a" => 1, "b" => 2};
        let b = crate::value_map! {"b" => 2, "a" => 1};
        assert_eq!(a, b);
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn sort_order() {
        let mut v = vec![
            Value::Int(10),
            Value::Float(2.5),
            Value::Int(2),
            big("-100000000000000000000"),
            Value::Bool(true),
            Value::Float(f64::NAN),
            Value::Float(f64::NEG_INFINITY),
            big("100000000000000000000"),
            Value::Float(1e19),
        ];
        py_sort(&mut v);
        let reprs: Vec<String> = v.iter().map(Value::repr).collect();
        assert_eq!(
            reprs,
            [
                "-inf",
                "-100000000000000000000",
                "True",
                "2",
                "2.5",
                "10",
                "1e+19",
                "100000000000000000000",
                "nan"
            ]
        );
        let mut s = vec![
            Value::from("b"),
            Value::from("B"),
            Value::from("é"),
            Value::from("a"),
        ];
        py_sort(&mut s);
        assert_eq!(
            s,
            [
                Value::from("B"),
                Value::from("a"),
                Value::from("b"),
                Value::from("é")
            ]
        );
    }

    fn number() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let small_float = (-5_i32..5).prop_map(|i| Value::Float(f64::from(i) / 2.0));
        prop_oneof![
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::Int),
            (-5i64..5).prop_map(Value::Int),
            small_float,
            any::<f64>().prop_map(Value::Float),
            (-1e25f64..1e25).prop_map(|f| Value::Float(f.trunc())),
            "-?[1-9][0-9]{0,25}"
                .prop_map(|s| Value::BigInt(s.parse().unwrap_or_else(|_| BigInt::from(0)))),
        ]
    }

    proptest::proptest! {
        #[test]
        fn number_order_is_total_and_consistent_with_eq(a in number(), b in number(), c in number()) {
            use proptest::prop_assert_eq;
            prop_assert_eq!(py_cmp(&a, &b), py_cmp(&b, &a).reverse());
            prop_assert_eq!(py_cmp(&a, &b) == Ordering::Equal, a == b);
            if py_cmp(&a, &b).is_le() && py_cmp(&b, &c).is_le() {
                proptest::prop_assert!(py_cmp(&a, &c).is_le());
            }
            if a == b {
                prop_assert_eq!(hash(&a), hash(&b));
            }
        }

        #[test]
        fn string_order_is_code_point_order(a in ".*", b in ".*") {
            let by_chars = a.chars().cmp(b.chars());
            proptest::prop_assert_eq!(py_cmp(&Value::from(a.as_str()), &Value::from(b.as_str())), by_chars);
        }
    }

    #[test]
    fn exact_int_float_comparison() {
        assert_eq!(
            cmp_i64_f64(9_007_199_254_740_993, 9_007_199_254_740_992.0),
            Ordering::Greater
        );
        assert_eq!(cmp_i64_f64(i64::MAX, TWO_POW_63), Ordering::Less);
        assert_eq!(cmp_i64_f64(i64::MIN, -TWO_POW_63), Ordering::Equal);
        assert_eq!(cmp_i64_f64(-3, -2.5), Ordering::Less);
        assert_eq!(cmp_i64_f64(-2, -2.5), Ordering::Greater);
    }
}
