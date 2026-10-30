//! The Python objects `SafeConstructor` builds, as far as `check-yaml` can observe them: `dict` membership
//! (`hash` + `==`, with the identity shortcut and the shared `nan_value`), `TypeError: unhashable type`, and
//! `str()`/`repr()` for duplicate-key messages (containers print as constructed so far).
//!
//! Owner: WP-17.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use super::PyType;
use super::int::BigInt;
use crate::pystr;

pub(super) type ObjId = u32;

/// `datetime.date` / the date part of a `datetime.datetime`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Ymd {
    pub(super) y: i64,
    pub(super) m: i64,
    pub(super) d: i64,
}

/// `datetime.datetime` (`tz`: the `timezone` offset in seconds and its name).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DateTime {
    pub(super) date: Ymd,
    pub(super) hour: i64,
    pub(super) minute: i64,
    pub(super) second: i64,
    pub(super) micro: i64,
    pub(super) tz: Option<(i64, String)>,
}

/// `datetime.timedelta` normalised to (days, seconds, microseconds) like CPython.
pub(super) fn timedelta_repr(total_seconds: i64) -> String {
    let days = total_seconds.checked_div_euclid(86_400).unwrap_or(0);
    let secs = total_seconds.checked_rem_euclid(86_400).unwrap_or(0);
    let mut parts = Vec::new();
    if days != 0 {
        parts.push(format!("days={days}"));
    }
    if secs != 0 {
        parts.push(format!("seconds={secs}"));
    }
    if parts.is_empty() {
        "datetime.timedelta(0)".to_owned()
    } else {
        format!("datetime.timedelta({})", parts.join(", "))
    }
}

/// Days since 0001-01-01 (proleptic Gregorian), for comparing aware datetimes.
pub(super) fn ordinal(d: Ymd) -> i64 {
    let y = d.y.saturating_sub(1);
    let mut days = y
        .saturating_mul(365)
        .saturating_add(y / 4)
        .saturating_sub(y / 100)
        .saturating_add(y / 400);
    for m in 1..d.m {
        days = days.saturating_add(days_in_month(d.y, m));
    }
    days.saturating_add(d.d)
}

pub(super) const fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub(super) const fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

impl DateTime {
    /// Microseconds since the epoch of `ordinal` in UTC (aware) or local wall time (naive).
    fn instant(&self) -> i128 {
        let days = i128::from(ordinal(self.date));
        let secs = days
            .saturating_mul(86_400)
            .saturating_add(i128::from(self.hour).saturating_mul(3600))
            .saturating_add(i128::from(self.minute).saturating_mul(60))
            .saturating_add(i128::from(self.second))
            .saturating_sub(i128::from(self.tz.as_ref().map_or(0, |t| t.0)));
        secs.saturating_mul(1_000_000)
            .saturating_add(i128::from(self.micro))
    }
}

/// An insertion-ordered `dict` (or the keys of a `set`).
#[derive(Debug, Clone, Default)]
pub(super) struct Dict {
    pub(super) entries: Vec<(ObjId, ObjId)>,
    index: HashMap<u64, Vec<usize>>,
}

/// A constructed object.
#[derive(Debug, Clone)]
pub(super) enum Obj {
    None,
    Bool(bool),
    Int(BigInt),
    /// A float; the flag marks `SafeConstructor.nan_value`, one shared object (so `.nan` keys collide).
    Float(f64, bool),
    Str(Vec<u8>),
    Bytes(Vec<u8>),
    Date(Ymd),
    DateTime(DateTime),
    List(Vec<ObjId>),
    Tuple(Vec<ObjId>),
    Dict(Dict),
    ODict(Dict),
    Set(Dict),
}

impl Obj {
    pub(super) const fn py_type(&self) -> PyType {
        match self {
            Self::None => PyType::NoneType,
            Self::Bool(_) => PyType::Bool,
            Self::Int(_) => PyType::Int,
            Self::Float(..) => PyType::Float,
            Self::Str(_) => PyType::Str,
            Self::Bytes(_) => PyType::Bytes,
            Self::Date(_) => PyType::Date,
            Self::DateTime(_) => PyType::DateTime,
            Self::List(_) => PyType::List,
            Self::Tuple(_) => PyType::Tuple,
            Self::Dict(_) => PyType::Dict,
            Self::ODict(_) => PyType::ODict,
            Self::Set(_) => PyType::Set,
        }
    }

    /// `isinstance(obj, Hashable)`.
    pub(super) const fn is_hashable_type(&self) -> bool {
        !matches!(
            self,
            Self::List(_) | Self::Dict(_) | Self::ODict(_) | Self::Set(_)
        )
    }
}

/// The object arena.
#[derive(Debug, Default)]
pub(super) struct Objs {
    pub(super) v: Vec<Obj>,
}

/// A numeric value for cross-type comparison.
enum Num<'a> {
    Int(std::borrow::Cow<'a, BigInt>),
    Float(f64, bool),
}

impl Objs {
    pub(super) fn push(&mut self, o: Obj) -> ObjId {
        self.v.push(o);
        ObjId::try_from(self.v.len().saturating_sub(1)).unwrap_or(ObjId::MAX)
    }

    pub(super) fn get(&self, id: ObjId) -> &Obj {
        const NONE: Obj = Obj::None;
        self.v
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .unwrap_or(&NONE)
    }

    pub(super) fn get_mut(&mut self, id: ObjId) -> Option<&mut Obj> {
        self.v.get_mut(usize::try_from(id).unwrap_or(usize::MAX))
    }

    fn num(&self, id: ObjId) -> Option<Num<'_>> {
        match self.get(id) {
            Obj::Bool(b) => Some(Num::Int(std::borrow::Cow::Owned(BigInt::from_u64(
                u64::from(*b),
            )))),
            Obj::Int(i) => Some(Num::Int(std::borrow::Cow::Borrowed(i))),
            Obj::Float(f, shared) => Some(Num::Float(*f, *shared)),
            _ => None,
        }
    }

    /// `hash(obj)` up to consistency with `==` (equal objects hash equal); `Err` is the unhashable type name.
    pub(super) fn hash(&self, id: ObjId) -> Result<u64, &'static str> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash_into(id, &mut h)?;
        Ok(h.finish())
    }

    fn hash_into(&self, id: ObjId, h: &mut impl Hasher) -> Result<(), &'static str> {
        match self.get(id) {
            Obj::None => 0u8.hash(h),
            Obj::Bool(_) | Obj::Int(_) | Obj::Float(..) => {
                1u8.hash(h);
                match self.num(id) {
                    Some(Num::Int(i)) => i.hash(h),
                    Some(Num::Float(f, _)) => match BigInt::from_f64(f) {
                        Some(i) => i.hash(h),
                        None if f.is_nan() => 2u8.hash(h),
                        None => f.to_bits().hash(h),
                    },
                    None => {}
                }
            }
            Obj::Str(s) => {
                3u8.hash(h);
                s.hash(h);
            }
            Obj::Bytes(b) => {
                4u8.hash(h);
                b.hash(h);
            }
            Obj::Date(d) => {
                5u8.hash(h);
                d.hash(h);
            }
            Obj::DateTime(dt) => {
                6u8.hash(h);
                dt.instant().hash(h);
            }
            Obj::Tuple(items) => {
                7u8.hash(h);
                for &i in items {
                    self.hash_into(i, h)?;
                }
            }
            Obj::List(_) => return Err("list"),
            Obj::Dict(_) => return Err("dict"),
            Obj::ODict(_) => return Err("ordereddict"),
            Obj::Set(_) => return Err("set"),
        }
        Ok(())
    }

    /// `a == b` with the identity shortcut of `PyObject_RichCompareBool` (for hashable objects).
    #[expect(
        clippy::many_single_char_names,
        reason = "the names of the Python and C code this ports"
    )]
    pub(super) fn eq(&self, a: ObjId, b: ObjId) -> bool {
        if a == b {
            return true;
        }
        match (self.num(a), self.num(b)) {
            (Some(x), Some(y)) => {
                return match (x, y) {
                    (Num::Int(p), Num::Int(q)) => p == q,
                    (Num::Int(p), Num::Float(f, _)) | (Num::Float(f, _), Num::Int(p)) => {
                        p.eq_f64(f)
                    }
                    (Num::Float(f, s1), Num::Float(g, s2)) => {
                        if f.is_nan() || g.is_nan() {
                            s1 && s2 && f.is_nan() && g.is_nan()
                        } else {
                            f == g
                        }
                    }
                };
            }
            (None, None) => {}
            _ => return false,
        }
        match (self.get(a), self.get(b)) {
            (Obj::None, Obj::None) => true,
            (Obj::Str(x), Obj::Str(y)) | (Obj::Bytes(x), Obj::Bytes(y)) => x == y,
            (Obj::Date(x), Obj::Date(y)) => x == y,
            (Obj::DateTime(x), Obj::DateTime(y)) => match (&x.tz, &y.tz) {
                (None, None) | (Some(_), Some(_)) => x.instant() == y.instant(),
                _ => false,
            },
            (Obj::Tuple(x), Obj::Tuple(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(&p, &q)| self.eq(p, q))
            }
            _ => false,
        }
    }
}

impl Dict {
    /// The entry index of `key`, hashing it first (`Err`: unhashable).
    pub(super) fn find(&self, objs: &Objs, key: ObjId) -> Result<Option<usize>, &'static str> {
        let h = objs.hash(key)?;
        Ok(self.index.get(&h).and_then(|slots| {
            slots
                .iter()
                .copied()
                .find(|&i| self.entries.get(i).is_some_and(|e| objs.eq(e.0, key)))
        }))
    }

    /// `d[key] = value`: an existing key keeps its key object.
    pub(super) fn set(
        &mut self,
        objs: &Objs,
        key: ObjId,
        value: ObjId,
    ) -> Result<(), &'static str> {
        if let Some(i) = self.find(objs, key)? {
            if let Some(e) = self.entries.get_mut(i) {
                e.1 = value;
            }
        } else {
            let h = objs.hash(key)?;
            self.index.entry(h).or_default().push(self.entries.len());
            self.entries.push((key, value));
        }
        Ok(())
    }
}

/// `repr(str)` of a generalized-UTF-8 string (CPython's `unicode_repr`).
pub(super) fn repr_pystr(s: &[u8]) -> String {
    let cps: Vec<u32> = pystr::code_points(s).collect();
    let has_sq = cps.contains(&u32::from(b'\''));
    let has_dq = cps.contains(&u32::from(b'"'));
    let quote = if has_sq && !has_dq { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for cp in cps {
        match char::from_u32(cp) {
            Some(c) if c == quote || c == '\\' => {
                out.push('\\');
                out.push(c);
            }
            Some('\t') => out.push_str("\\t"),
            Some('\n') => out.push_str("\\n"),
            Some('\r') => out.push_str("\\r"),
            Some(c) if u32::from(c) < 0x20 || u32::from(c) == 0x7f => {
                write!(out, "\\x{:02x}", u32::from(c)).unwrap_or_default();
            }
            Some(c) if c.is_ascii() => out.push(c),
            Some(c) if snp_pycompat::chars::is_printable(c) => out.push(c),
            _ => {
                if cp <= 0xff {
                    write!(out, "\\x{cp:02x}").unwrap_or_default();
                } else if cp <= 0xffff {
                    write!(out, "\\u{cp:04x}").unwrap_or_default();
                } else {
                    write!(out, "\\U{cp:08x}").unwrap_or_default();
                }
            }
        }
    }
    out.push(quote);
    out
}

/// `repr(bytes)`.
fn repr_bytes(b: &[u8], out: &mut String) {
    let quote = if b.contains(&b'\'') && !b.contains(&b'"') {
        '"'
    } else {
        '\''
    };
    out.push('b');
    out.push(quote);
    for &c in b {
        match c {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            _ if char::from(c) == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(char::from(c)),
            _ => {
                write!(out, "\\x{c:02x}").unwrap_or_default();
            }
        }
    }
    out.push(quote);
}

fn tz_offset_str(secs: i64) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s != 0 {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    } else {
        format!("{sign}{h:02}:{m:02}")
    }
}

/// Text output in generalized UTF-8; `Err` is a crash (`ValueError` of the int-to-str limit).
pub(super) type Text = Result<Vec<u8>, String>;

#[expect(
    clippy::multiple_inherent_impl,
    reason = "the text output of objects is kept apart from equality and hashing"
)]
impl Objs {
    /// `str(obj)`.
    pub(super) fn str_of(&self, id: ObjId) -> Text {
        match self.get(id) {
            Obj::Str(s) => Ok(s.clone()),
            Obj::Date(d) => Ok(format!("{:04}-{:02}-{:02}", d.y, d.m, d.d).into_bytes()),
            Obj::DateTime(dt) => {
                let mut s = format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    dt.date.y, dt.date.m, dt.date.d, dt.hour, dt.minute, dt.second
                );
                if dt.micro != 0 {
                    write!(s, ".{:06}", dt.micro).unwrap_or_default();
                }
                if let Some((off, _)) = &dt.tz {
                    s.push_str(&tz_offset_str(*off));
                }
                Ok(s.into_bytes())
            }
            _ => self.repr(id),
        }
    }

    /// `repr(obj)` (recursive containers print `[...]`, `{...}`, `set(...)` and `...`).
    pub(super) fn repr(&self, id: ObjId) -> Text {
        let mut out = Vec::new();
        let mut active = Vec::new();
        self.repr_into(id, &mut out, &mut active)?;
        Ok(out)
    }

    fn repr_into(
        &self,
        id: ObjId,
        out: &mut Vec<u8>,
        active: &mut Vec<ObjId>,
    ) -> Result<(), String> {
        let o = self.get(id);
        let container = matches!(o, Obj::List(_) | Obj::Dict(_) | Obj::ODict(_) | Obj::Set(_));
        if container && active.contains(&id) {
            out.extend_from_slice(match o {
                Obj::List(_) => b"[...]".as_slice(),
                Obj::Dict(_) => b"{...}",
                Obj::Set(_) => b"set(...)",
                _ => b"...",
            });
            return Ok(());
        }
        let push_str = |out: &mut Vec<u8>, s: &str| out.extend_from_slice(s.as_bytes());
        match o {
            Obj::None => push_str(out, "None"),
            Obj::Bool(true) => push_str(out, "True"),
            Obj::Bool(false) => push_str(out, "False"),
            Obj::Int(i) => push_str(out, &i.to_decimal()?),
            Obj::Float(f, _) => push_str(out, &snp_pycompat::float::repr(*f)),
            Obj::Str(s) => push_str(out, &repr_pystr(s)),
            Obj::Bytes(b) => {
                let mut s = String::new();
                repr_bytes(b, &mut s);
                push_str(out, &s);
            }
            Obj::Date(d) => push_str(out, &format!("datetime.date({}, {}, {})", d.y, d.m, d.d)),
            Obj::DateTime(dt) => {
                let mut s = format!(
                    "datetime.datetime({}, {}, {}, {}, {}",
                    dt.date.y, dt.date.m, dt.date.d, dt.hour, dt.minute
                );
                if dt.second != 0 || dt.micro != 0 {
                    write!(s, ", {}", dt.second).unwrap_or_default();
                }
                if dt.micro != 0 {
                    write!(s, ", {}", dt.micro).unwrap_or_default();
                }
                if let Some((off, name)) = &dt.tz {
                    write!(
                        s,
                        ", tzinfo=datetime.timezone({}, {})",
                        timedelta_repr(*off),
                        snp_pycompat::repr::repr_str(name)
                    )
                    .unwrap_or_default();
                }
                s.push(')');
                push_str(out, &s);
            }
            Obj::Tuple(items) => {
                out.push(b'(');
                for (k, &i) in items.iter().enumerate() {
                    if k > 0 {
                        push_str(out, ", ");
                    }
                    self.repr_into(i, out, active)?;
                }
                if items.len() == 1 {
                    out.push(b',');
                }
                out.push(b')');
            }
            Obj::List(items) => {
                active.push(id);
                out.push(b'[');
                for (k, &i) in items.iter().enumerate() {
                    if k > 0 {
                        push_str(out, ", ");
                    }
                    self.repr_into(i, out, active)?;
                }
                out.push(b']');
                active.pop();
            }
            Obj::Dict(d) | Obj::ODict(d) => {
                let odict = matches!(o, Obj::ODict(_));
                if odict && d.entries.is_empty() {
                    push_str(out, "ordereddict()");
                    return Ok(());
                }
                active.push(id);
                if odict {
                    push_str(out, "ordereddict(");
                }
                out.push(b'{');
                for (k, &(key, val)) in d.entries.iter().enumerate() {
                    if k > 0 {
                        push_str(out, ", ");
                    }
                    self.repr_into(key, out, active)?;
                    push_str(out, ": ");
                    self.repr_into(val, out, active)?;
                }
                out.push(b'}');
                if odict {
                    out.push(b')');
                }
                active.pop();
            }
            Obj::Set(d) => {
                if d.entries.is_empty() {
                    push_str(out, "set()");
                    return Ok(());
                }
                active.push(id);
                out.push(b'{');
                for (k, &(key, _)) in d.entries.iter().enumerate() {
                    if k > 0 {
                        push_str(out, ", ");
                    }
                    self.repr_into(key, out, active)?;
                }
                out.push(b'}');
                active.pop();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprs() {
        assert_eq!(repr_pystr(b"it's"), "\"it's\"");
        assert_eq!(repr_pystr("\u{e9}\u{85}".as_bytes()), "'\u{e9}\\x85'");
        let mut lone = Vec::new();
        pystr::push_cp(&mut lone, 0xd800);
        assert_eq!(repr_pystr(&lone), "'\\ud800'");
        assert_eq!(
            timedelta_repr(-5 * 3600),
            "datetime.timedelta(days=-1, seconds=68400)"
        );
        assert_eq!(timedelta_repr(0), "datetime.timedelta(0)");
        let mut o = Objs::default();
        let a = o.push(Obj::Str(b"a".to_vec()));
        let one = o.push(Obj::Int(BigInt::from_u64(1)));
        let t = o.push(Obj::Tuple(vec![a]));
        assert_eq!(o.repr(t).unwrap(), b"('a',)");
        let l = o.push(Obj::List(vec![one, a]));
        assert_eq!(o.str_of(l).unwrap(), b"[1, 'a']");
        let tr = o.push(Obj::Bool(true));
        let fl = o.push(Obj::Float(1.0, false));
        assert!(o.eq(tr, one));
        assert!(o.eq(one, fl));
        assert_eq!(o.hash(tr), o.hash(fl));
    }
}
