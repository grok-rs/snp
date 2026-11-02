//! The decoded Python objects of `pretty-format-json` (lone surrogates kept), Python's `==`/`<` between them, and
//! the hook's `pairs_first` (`pretty_format_json.py:19-25`).
//!
//! Owner: WP-17.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::{DumpOpts, JsonValue, PyMinor};
use crate::pysort;
use crate::pystr;

/// A decoded JSON value as the Python object `json.loads` builds. Deliberately not `Clone`: a derived clone
/// recurses once per nesting level (and copying subtrees made `dumps_pretty` quadratic in depth).
#[derive(Debug)]
pub(super) enum Val {
    /// `None`.
    Null,
    /// `True` / `False`.
    Bool(bool),
    /// `int`: its decimal repr.
    Int(Box<str>),
    /// `float`. Every `NaN` comes from the module constant, so all of them are one object (identity-equal).
    Float(f64),
    /// `str` in generalized UTF-8.
    Str(Box<[u8]>),
    /// `list`.
    List(Vec<Self>),
    /// `dict` (after `pairs_first`) or the raw pairs: keys in generalized UTF-8.
    Dict(Vec<(Box<[u8]>, Self)>),
}

impl Val {
    /// The Python class name used in `TypeError` texts.
    fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::List(_) => "list",
            Self::Dict(_) => "dict",
        }
    }

    /// A [`JsonValue`] as the Python object it stands for (iterative: no recursion on nesting).
    pub(super) fn from_json(v: &JsonValue) -> Self {
        enum Work<'a> {
            Visit(&'a JsonValue),
            List(usize),
            Dict(Vec<Box<[u8]>>),
        }
        let mut work = vec![Work::Visit(v)];
        let mut out: Vec<Self> = Vec::new();
        while let Some(w) = work.pop() {
            match w {
                Work::Visit(v) => match v {
                    JsonValue::Null => out.push(Self::Null),
                    JsonValue::Bool(b) => out.push(Self::Bool(*b)),
                    JsonValue::Int(i) => out.push(Self::Int(i.clone())),
                    JsonValue::Float(f) => out.push(Self::Float(*f)),
                    JsonValue::Str(s) => out.push(Self::Str(s.as_bytes().into())),
                    JsonValue::Array(items) => {
                        work.push(Work::List(items.len()));
                        work.extend(items.iter().rev().map(Work::Visit));
                    }
                    JsonValue::Object(pairs) => {
                        work.push(Work::Dict(
                            pairs.iter().map(|(k, _)| k.as_bytes().into()).collect(),
                        ));
                        work.extend(pairs.iter().rev().map(|(_, v)| Work::Visit(v)));
                    }
                },
                Work::List(n) => {
                    let items = split_tail(&mut out, n);
                    out.push(Self::List(items));
                }
                Work::Dict(keys) => {
                    let vals = split_tail(&mut out, keys.len());
                    out.push(Self::Dict(keys.into_iter().zip(vals).collect()));
                }
            }
        }
        out.pop().unwrap_or(Self::Null)
    }

    /// The public [`JsonValue`] (lone surrogates become U+FFFD), iteratively.
    pub(super) fn into_json(self) -> JsonValue {
        enum Work {
            Visit(Val),
            List(usize),
            Dict(Vec<String>),
        }
        let mut work = vec![Work::Visit(self)];
        let mut out: Vec<JsonValue> = Vec::new();
        while let Some(w) = work.pop() {
            match w {
                Work::Visit(mut v) => match &mut v {
                    Self::Null => out.push(JsonValue::Null),
                    Self::Bool(b) => out.push(JsonValue::Bool(*b)),
                    Self::Int(i) => out.push(JsonValue::Int(std::mem::take(i))),
                    Self::Float(f) => out.push(JsonValue::Float(*f)),
                    Self::Str(s) => out.push(JsonValue::Str(pystr::to_string_lossy(s))),
                    Self::List(items) => {
                        let items = std::mem::take(items);
                        work.push(Work::List(items.len()));
                        work.extend(items.into_iter().rev().map(Work::Visit));
                    }
                    Self::Dict(pairs) => {
                        let (keys, vals): (Vec<_>, Vec<_>) =
                            std::mem::take(pairs).into_iter().unzip();
                        work.push(Work::Dict(
                            keys.iter().map(|k| pystr::to_string_lossy(k)).collect(),
                        ));
                        work.extend(vals.into_iter().rev().map(Work::Visit));
                    }
                },
                Work::List(n) => {
                    let items = split_tail(&mut out, n);
                    out.push(JsonValue::Array(items));
                }
                Work::Dict(keys) => {
                    let vals = split_tail(&mut out, keys.len());
                    out.push(JsonValue::Object(keys.into_iter().zip(vals).collect()));
                }
            }
        }
        out.pop().unwrap_or(JsonValue::Null)
    }
}

/// Dropping a deep tree must not recurse (a 9998-deep document is legal input): children are moved to a heap
/// stack first.
impl Drop for Val {
    fn drop(&mut self) {
        let mut stack: Vec<Self> = Vec::new();
        let take = |v: &mut Self, stack: &mut Vec<Self>| match v {
            Self::List(items) => stack.extend(std::mem::take(items)),
            Self::Dict(pairs) => stack.extend(std::mem::take(pairs).into_iter().map(|(_, x)| x)),
            _ => {}
        };
        take(self, &mut stack);
        while let Some(mut v) = stack.pop() {
            take(&mut v, &mut stack);
        }
    }
}

/// The last `n` elements of `v`, in order (`n` is clamped to the length).
pub(crate) fn split_tail<T>(v: &mut Vec<T>, n: usize) -> Vec<T> {
    let at = v.len().saturating_sub(n);
    let mut tail = Vec::new();
    while v.len() > at {
        if let Some(x) = v.pop() {
            tail.push(x);
        }
    }
    tail.reverse();
    tail
}

/// An object's `(key, value)` pairs.
pub(super) type Pairs = Vec<(Box<[u8]>, Val)>;

/// `TypeError: '<' not supported between instances of 'X' and 'Y'`.
#[derive(Debug, Clone)]
pub(super) struct TypeErr(pub(super) &'static str, pub(super) &'static str);

impl TypeErr {
    pub(super) fn text(&self) -> String {
        format!(
            "'<' not supported between instances of '{}' and '{}'",
            self.0, self.1
        )
    }
}

// --------------------------------------------------------------------------------------------------------------
// numbers
// --------------------------------------------------------------------------------------------------------------

/// A Python number: `bool`/`int` as a decimal string, or a float.
enum Num<'a> {
    Int(&'a str),
    Float(f64),
}

fn as_num(v: &Val) -> Option<Num<'_>> {
    match v {
        Val::Bool(false) => Some(Num::Int("0")),
        Val::Bool(true) => Some(Num::Int("1")),
        Val::Int(i) => Some(Num::Int(i)),
        Val::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// Compares two decimal integer strings (`-` sign, no leading zeros) numerically.
fn cmp_int(a: &str, b: &str) -> Ordering {
    let (na, da) = a.strip_prefix('-').map_or((false, a), |d| (true, d));
    let (nb, db) = b.strip_prefix('-').map_or((false, b), |d| (true, d));
    let (na, nb) = (na && da != "0", nb && db != "0");
    let mag = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
    match (na, nb) {
        (false, false) => mag,
        (true, true) => mag.reverse(),
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
    }
}

/// Python's exact `int` ↔ `float` comparison; `None` when the float is NaN.
fn cmp_int_float(i: &str, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        return Some(if f > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let t = f.trunc();
    let frac = f - t;
    let t_str = if t == 0.0 {
        "0".to_owned()
    } else {
        format!("{t:.0}")
    };
    Some(cmp_int(i, &t_str).then(if frac > 0.0 {
        Ordering::Less
    } else if frac < 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }))
}

fn num_cmp(a: &Num<'_>, b: &Num<'_>) -> Option<Ordering> {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => Some(cmp_int(x, y)),
        (Num::Float(x), Num::Float(y)) => x.partial_cmp(y),
        (Num::Int(x), Num::Float(y)) => cmp_int_float(x, *y),
        (Num::Float(x), Num::Int(y)) => cmp_int_float(y, *x).map(Ordering::reverse),
    }
}

// --------------------------------------------------------------------------------------------------------------
// == and <
// --------------------------------------------------------------------------------------------------------------

/// `PyObject_RichCompareBool(a, b, Py_EQ)` (identity first: every JSON `NaN` is the same object), iteratively.
pub(super) fn py_eq(left: &Val, right: &Val) -> bool {
    let mut todo = vec![(left, right)];
    while let Some((lhs, rhs)) = todo.pop() {
        let equal = match (lhs, rhs) {
            (Val::Float(fl), Val::Float(fr)) if fl.is_nan() && fr.is_nan() => true,
            (Val::Null, Val::Null) => true,
            (Val::Str(sl), Val::Str(sr)) => sl == sr,
            (Val::List(items_l), Val::List(items_r)) => {
                let same_len = items_l.len() == items_r.len();
                if same_len {
                    todo.extend(items_l.iter().zip(items_r.iter()));
                }
                same_len
            }
            (Val::Dict(pairs_l), Val::Dict(pairs_r)) => {
                if pairs_l.len() == pairs_r.len() {
                    let index: HashMap<&[u8], &Val> =
                        pairs_r.iter().map(|(k, v)| (&**k, v)).collect();
                    let mut ok = true;
                    for (key, val) in pairs_l {
                        if let Some(other) = index.get(&**key) {
                            todo.push((val, other));
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    ok
                } else {
                    false
                }
            }
            _ => match (as_num(lhs), as_num(rhs)) {
                (Some(nl), Some(nr)) => num_cmp(&nl, &nr) == Some(Ordering::Equal),
                _ => false,
            },
        };
        if !equal {
            return false;
        }
    }
    true
}

/// `a < b` with Python semantics: numbers across `bool`/`int`/`float`, `str` by code point, `list`
/// lexicographically (first `!=` element decides); `None`, `dict` and mixed types raise `TypeError`.
pub(super) fn py_lt(left: &Val, right: &Val) -> Result<bool, TypeErr> {
    let (mut lhs, mut rhs) = (left, right);
    loop {
        match (lhs, rhs) {
            (Val::Str(sl), Val::Str(sr)) => return Ok(sl < sr),
            (Val::List(items_l), Val::List(items_r)) => {
                let Some((el, er)) = items_l
                    .iter()
                    .zip(items_r.iter())
                    .find(|(el, er)| !py_eq(el, er))
                else {
                    return Ok(items_l.len() < items_r.len());
                };
                lhs = el;
                rhs = er;
            }
            _ => {
                return match (as_num(lhs), as_num(rhs)) {
                    (Some(nl), Some(nr)) => Ok(num_cmp(&nl, &nr) == Some(Ordering::Less)),
                    _ => Err(TypeErr(lhs.type_name(), rhs.type_name())),
                };
            }
        }
    }
}

// --------------------------------------------------------------------------------------------------------------
// pairs_first
// --------------------------------------------------------------------------------------------------------------

/// `pairs_first(pairs)`: top keys first (stable by `top_keys.index(key)`), the rest `list.sort()`ed as
/// `(key, value)` tuples when `sort_keys`, then `dict(before + after)`. `minor` selects the emulated `list.sort`
/// (3.12 and 3.13 differ in run detection); `None` is the lenient variant that never raises nor panics (3.13's sort
/// with `<` taken as `False` wherever Python raises `TypeError`).
pub(super) fn pairs_first(
    pairs: Pairs,
    o: &DumpOpts,
    minor: Option<PyMinor>,
) -> Result<Pairs, TypeErr> {
    let top_index = |k: &[u8]| o.top_keys.iter().position(|t| t.as_bytes() == k);
    let (mut before, mut after): (Vec<_>, Vec<_>) =
        pairs.into_iter().partition(|(k, _)| top_index(k).is_some());
    before.sort_by_key(|(k, _)| top_index(k));
    if o.sort_keys {
        let lt = |x: &(Box<[u8]>, Val), y: &(Box<[u8]>, Val)| -> Result<bool, TypeErr> {
            if x.0 != y.0 {
                return Ok(x.0 < y.0);
            }
            if py_eq(&x.1, &y.1) {
                return Ok(false);
            }
            py_lt(&x.1, &y.1)
        };
        if let Some(v) = minor {
            pysort::sort(&mut after, v, lt)?;
        } else {
            // CPython's sort with a `<` that answers `False` where Python raises: like CPython, it tolerates the
            // inconsistent order this describes (mixed value types, NaN), where `slice::sort_by` panics on it.
            let lenient = |x: &(Box<[u8]>, Val), y: &(Box<[u8]>, Val)| {
                Ok::<bool, std::convert::Infallible>(lt(x, y).unwrap_or(false))
            };
            let Ok(()) = pysort::sort(&mut after, PyMinor::P313, lenient);
        }
    }
    let mut out: Vec<(Box<[u8]>, Val)> = Vec::new();
    let mut index: HashMap<Box<[u8]>, usize> = HashMap::new();
    for (k, v) in before.into_iter().chain(after) {
        if let Some(&i) = index.get(&k) {
            if let Some(slot) = out.get_mut(i) {
                slot.1 = v;
            }
        } else {
            index.insert(k.clone(), out.len());
            out.push((k, v));
        }
    }
    Ok(out)
}

/// The lenient [`pairs_first`] (never raises) applied to every object of a tree, innermost first (iterative
/// post-order; no subtree is copied).
pub(super) fn pairs_first_tree(v: Val, o: &DumpOpts) -> Val {
    enum Work {
        Visit(Val),
        List(usize),
        Dict(Vec<Box<[u8]>>),
    }
    let mut work = vec![Work::Visit(v)];
    let mut out: Vec<Val> = Vec::new();
    while let Some(w) = work.pop() {
        match w {
            Work::Visit(mut v) => match &mut v {
                Val::List(items) => {
                    let items = std::mem::take(items);
                    work.push(Work::List(items.len()));
                    work.extend(items.into_iter().rev().map(Work::Visit));
                }
                Val::Dict(pairs) => {
                    let (keys, vals): (Vec<_>, Vec<_>) = std::mem::take(pairs).into_iter().unzip();
                    work.push(Work::Dict(keys));
                    work.extend(vals.into_iter().rev().map(Work::Visit));
                }
                _ => out.push(v),
            },
            Work::List(n) => {
                let items = split_tail(&mut out, n);
                out.push(Val::List(items));
            }
            Work::Dict(keys) => {
                let vals = split_tail(&mut out, keys.len());
                let pairs: Vec<_> = keys.into_iter().zip(vals).collect();
                let pairs = pairs_first(pairs, o, None).unwrap_or_default();
                out.push(Val::Dict(pairs));
            }
        }
    }
    out.pop().unwrap_or(Val::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(s: &str) -> Val {
        Val::Int(s.into())
    }

    #[test]
    fn numbers_compare_exactly() {
        assert!(
            py_lt(
                &int("9007199254740993"),
                &Val::Float(9_007_199_254_740_992.0)
            )
            .is_ok_and(|b| !b)
        );
        assert!(
            py_lt(
                &Val::Float(9_007_199_254_740_992.0),
                &int("9007199254740993")
            )
            .unwrap()
        );
        assert!(py_eq(&int("1"), &Val::Float(1.0)));
        assert!(py_eq(&Val::Bool(true), &int("1")));
        assert!(py_lt(&int("-5"), &int("-4")).unwrap());
        assert!(py_lt(&int("-10"), &int("2")).unwrap());
        assert!(
            py_lt(
                &int("100000000000000000000000000000"),
                &Val::Float(f64::INFINITY)
            )
            .unwrap()
        );
        assert!(!py_lt(&Val::Float(f64::NAN), &int("1")).unwrap());
        assert!(py_eq(&Val::Float(f64::NAN), &Val::Float(f64::NAN)));
        assert!(py_lt(&Val::Float(-0.5), &int("0")).unwrap());
        assert!(py_eq(&int("0"), &Val::Float(-0.0)));
    }

    #[test]
    fn type_errors() {
        let e = py_lt(&int("1"), &Val::Dict(Vec::new())).unwrap_err();
        assert_eq!(
            e.text(),
            "'<' not supported between instances of 'int' and 'dict'"
        );
        assert!(py_lt(&Val::Null, &Val::Null).is_err());
        assert!(py_eq(&Val::Null, &Val::Null));
        let a = Val::List(vec![int("1"), Val::Str((*b"x").into())]);
        let b = Val::List(vec![int("1"), int("2")]);
        assert!(py_lt(&a, &b).is_err());
        assert!(py_lt(&Val::List(vec![int("1")]), &b).unwrap());
    }
}
