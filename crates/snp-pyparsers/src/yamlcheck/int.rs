//! Python `int` (arbitrary precision, sign-magnitude) for the constructor: `int(s, base)` and `float(s)` with
//! CPython's acceptance (whitespace, Unicode decimal digits, underscores between digits, base prefixes) and
//! `ValueError` texts, including the 4300-digit conversion limit in both directions.
//!
//! Owner: WP-17.

use std::cmp::Ordering;
use std::fmt::Write as _;

use snp_pycompat::chars::{decimal_value, is_space};

use super::obj::repr_pystr;
use crate::pystr;

/// `sys.get_int_max_str_digits()`.
const MAX_STR_DIGITS: usize = 4300;

/// An arbitrary-precision integer; `mag` is little-endian base 2³², without trailing zero limbs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(super) struct BigInt {
    pub(super) neg: bool,
    pub(super) mag: Vec<u32>,
}

impl BigInt {
    pub(super) fn from_u64(v: u64) -> Self {
        let mut b = Self {
            neg: false,
            mag: vec![
                u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0),
                u32::try_from(v >> 32).unwrap_or(0),
            ],
        };
        b.trim();
        b
    }

    pub(super) fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    fn trim(&mut self) {
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        if self.mag.is_empty() {
            self.neg = false;
        }
    }

    pub(super) fn negated(mut self) -> Self {
        if !self.is_zero() {
            self.neg.toggle();
        }
        self
    }

    /// `self * m + a` on the magnitude.
    fn mul_add_small(&mut self, m: u32, a: u32) {
        let mut carry = u64::from(a);
        for limb in &mut self.mag {
            let v = u64::from(*limb)
                .wrapping_mul(u64::from(m))
                .wrapping_add(carry);
            *limb = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0);
            carry = v >> 32;
        }
        if carry != 0 {
            self.mag.push(u32::try_from(carry).unwrap_or(0));
        }
        self.trim();
    }

    fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.iter().rev().cmp(b.iter().rev()))
    }

    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut carry = 0u64;
        for i in 0..a.len().max(b.len()) {
            let v = u64::from(a.get(i).copied().unwrap_or(0))
                .wrapping_add(u64::from(b.get(i).copied().unwrap_or(0)))
                .wrapping_add(carry);
            out.push(u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0));
            carry = v >> 32;
        }
        if carry != 0 {
            out.push(u32::try_from(carry).unwrap_or(0));
        }
        out
    }

    /// `a - b` for `|a| >= |b|`.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut borrow = 0i64;
        for (i, &x) in a.iter().enumerate() {
            let mut v = i64::from(x)
                .wrapping_sub(i64::from(b.get(i).copied().unwrap_or(0)))
                .wrapping_sub(borrow);
            if v < 0 {
                v = v.wrapping_add(1 << 32);
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(u32::try_from(v).unwrap_or(0));
        }
        out
    }

    pub(super) fn add(&self, o: &Self) -> Self {
        let mut r = if self.neg == o.neg {
            Self {
                neg: self.neg,
                mag: Self::add_mag(&self.mag, &o.mag),
            }
        } else if Self::cmp_mag(&self.mag, &o.mag) != Ordering::Less {
            Self {
                neg: self.neg,
                mag: Self::sub_mag(&self.mag, &o.mag),
            }
        } else {
            Self {
                neg: o.neg,
                mag: Self::sub_mag(&o.mag, &self.mag),
            }
        };
        r.trim();
        r
    }

    pub(super) fn mul(&self, o: &Self) -> Self {
        if self.is_zero() || o.is_zero() {
            return Self::default();
        }
        let mut out = vec![0u32; self.mag.len().saturating_add(o.mag.len())];
        for (i, &a) in self.mag.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in o.mag.iter().enumerate() {
                let k = i.saturating_add(j);
                let cur = u64::from(out.get(k).copied().unwrap_or(0));
                let v = u64::from(a)
                    .wrapping_mul(u64::from(b))
                    .wrapping_add(cur)
                    .wrapping_add(carry);
                if let Some(slot) = out.get_mut(k) {
                    *slot = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0);
                }
                carry = v >> 32;
            }
            let mut k = i.saturating_add(o.mag.len());
            while carry != 0 {
                let cur = u64::from(out.get(k).copied().unwrap_or(0));
                let v = cur.wrapping_add(carry);
                if let Some(slot) = out.get_mut(k) {
                    *slot = u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0);
                } else {
                    out.push(u32::try_from(v & 0xFFFF_FFFF).unwrap_or(0));
                }
                carry = v >> 32;
                k = k.saturating_add(1);
            }
        }
        let mut r = Self {
            neg: self.neg != o.neg,
            mag: out,
        };
        r.trim();
        r
    }

    /// The number of bits of the magnitude.
    fn bits(&self) -> u64 {
        match self.mag.last() {
            None => 0,
            Some(&top) => u64::try_from(self.mag.len().saturating_sub(1))
                .unwrap_or(u64::MAX)
                .saturating_mul(32)
                .saturating_add(u64::from(32u32.saturating_sub(top.leading_zeros()))),
        }
    }

    /// `str(int)`: `Err` is CPython's `ValueError` text past the 4300-digit limit.
    pub(super) fn to_decimal(&self) -> Result<String, String> {
        const LIMIT_MSG: &str = "Exceeds the limit (4300 digits) for integer string conversion; use \
                                 sys.set_int_max_str_digits() to increase the limit";
        // More than 14300 bits always has more than 4300 digits.
        if self.bits() > 14_300 {
            return Err(LIMIT_MSG.to_owned());
        }
        let mut mag = self.mag.clone();
        let mut chunks: Vec<u32> = Vec::new();
        while !mag.is_empty() {
            let mut rem = 0u64;
            for limb in mag.iter_mut().rev() {
                let cur = (rem << 32) | u64::from(*limb);
                *limb = u32::try_from(cur / 1_000_000_000).unwrap_or(0);
                rem = cur % 1_000_000_000;
            }
            chunks.push(u32::try_from(rem).unwrap_or(0));
            while mag.last() == Some(&0) {
                mag.pop();
            }
        }
        let mut s = String::new();
        if self.neg {
            s.push('-');
        }
        match chunks.split_last() {
            None => s.push('0'),
            Some((top, rest)) => {
                s.push_str(&top.to_string());
                for c in rest.iter().rev() {
                    write!(s, "{c:09}").unwrap_or_default();
                }
            }
        }
        if s.trim_start_matches('-').len() > MAX_STR_DIGITS {
            return Err(LIMIT_MSG.to_owned());
        }
        Ok(s)
    }

    /// The exact integer value of an integral finite float.
    pub(super) fn from_f64(x: f64) -> Option<Self> {
        if !x.is_finite() || x.fract() != 0.0 {
            return None;
        }
        let bits = x.abs().to_bits();
        let exp = i64::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
        let frac = bits & ((1u64 << 52) - 1);
        if exp == 0 {
            return Some(Self::default()); // zero (subnormals are never integral unless zero)
        }
        let mant = frac | (1u64 << 52);
        let shift = exp.saturating_sub(1075);
        let mut b = if shift < 0 {
            let s = u32::try_from(shift.saturating_neg()).unwrap_or(0);
            Self::from_u64(mant >> s.min(63))
        } else {
            let mut b = Self::from_u64(mant);
            for _ in 0..shift {
                b.mul_add_small(2, 0);
            }
            b
        };
        b.neg = x < 0.0 && !b.is_zero();
        Some(b)
    }

    /// Python's `int == float` (exact).
    pub(super) fn eq_f64(&self, x: f64) -> bool {
        Self::from_f64(x).is_some_and(|b| b == *self)
    }
}

/// The text `int()` sees after `_PyUnicode_TransformDecimalAndSpaceToASCII`: Unicode decimal digits become
/// ASCII digits, Unicode whitespace becomes a space, any other non-ASCII character becomes `?`.
fn to_ascii(s: &[u8]) -> String {
    pystr::code_points(s)
        .map(|cp| match char::from_u32(cp) {
            Some(c) if c.is_ascii() => c,
            Some(c) if is_space(c) => ' ',
            Some(c) => match decimal_value(c) {
                Some(d) => char::from_digit(d, 10).unwrap_or('?'),
                None => '?',
            },
            None => '?',
        })
        .collect()
}

/// `int(s, base)` for base 2, 8, 10 or 16.
#[expect(
    clippy::many_single_char_names,
    reason = "the names of the Python and C code this ports"
)]
pub(super) fn py_int(s: &[u8], base: u32) -> Result<BigInt, String> {
    let invalid = || {
        let r: String = repr_pystr(s).chars().take(200).collect();
        format!("invalid literal for int() with base {base}: {r}")
    };
    let a = to_ascii(s);
    let t = a.trim_matches(' ');
    let (neg, mut body) = match t.as_bytes().first() {
        Some(b'-') => (true, t.get(1..).unwrap_or_default()),
        Some(b'+') => (false, t.get(1..).unwrap_or_default()),
        _ => (false, t),
    };
    let mut prefixed = false;
    let prefix = match base {
        16 => Some(["0x", "0X"]),
        8 => Some(["0o", "0O"]),
        2 => Some(["0b", "0B"]),
        _ => None,
    };
    if let Some(ps) = prefix {
        for p in ps {
            if let Some(rest) = body.strip_prefix(p) {
                body = rest;
                prefixed = true;
                break;
            }
        }
    }
    let digit_of = |c: u8| -> Option<u32> { char::from(c).to_digit(base) };
    if base == 10 {
        let digits = body
            .bytes()
            .take_while(|&c| c.is_ascii_digit() || c == b'_')
            .filter(|&c| c != b'_')
            .count();
        if digits > MAX_STR_DIGITS {
            return Err(format!(
                "Exceeds the limit (4300 digits) for integer string conversion: value has {digits} digits; use \
                 sys.set_int_max_str_digits() to increase the limit"
            ));
        }
    }
    let bytes = body.as_bytes();
    if bytes.is_empty() {
        return Err(invalid());
    }
    let mut v = BigInt::default();
    let mut prev_underscore = !prefixed; // a leading underscore is only allowed right after a base prefix
    let mut any = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'_' {
            if prev_underscore || i.saturating_add(1) == bytes.len() {
                return Err(invalid());
            }
            prev_underscore = true;
            continue;
        }
        let Some(d) = digit_of(c) else {
            return Err(invalid());
        };
        v.mul_add_small(base, d);
        prev_underscore = false;
        any = true;
    }
    if !any {
        return Err(invalid());
    }
    Ok(if neg { v.negated() } else { v })
}

/// `float(s)`: `Err` is the `ValueError` text.
#[expect(
    clippy::many_single_char_names,
    reason = "the names of the Python and C code this ports"
)]
pub(super) fn py_float(s: &[u8]) -> Result<f64, String> {
    let invalid = || format!("could not convert string to float: {}", repr_pystr(s));
    let a = to_ascii(s);
    let t = a.trim_matches(' ');
    let body = t.trim_prefix(['-', '+']);
    let lower = body.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity" | "nan") {
        return t.parse::<f64>().map_err(|_| invalid());
    }
    // digits [. digits] | . digits, then an optional exponent; underscores only between digits
    let b = body.as_bytes();
    let mut i = 0usize;
    let mut clean = String::new();
    let mut int_digits = 0usize;
    let mut frac_digits = 0usize;
    let take_digits = |i: &mut usize, clean: &mut String| -> Result<usize, String> {
        let mut n = 0usize;
        let mut last_us = false;
        while let Some(&c) = b.get(*i) {
            if c.is_ascii_digit() {
                clean.push(char::from(c));
                n = n.saturating_add(1);
                last_us = false;
            } else if c == b'_'
                && n > 0
                && !last_us
                && b.get(i.saturating_add(1)).is_some_and(u8::is_ascii_digit)
            {
                last_us = true;
            } else {
                break;
            }
            *i = i.saturating_add(1);
        }
        Ok(n)
    };
    int_digits = int_digits.saturating_add(take_digits(&mut i, &mut clean)?);
    if b.get(i) == Some(&b'.') {
        clean.push('.');
        i = i.saturating_add(1);
        frac_digits = frac_digits.saturating_add(take_digits(&mut i, &mut clean)?);
    }
    if int_digits == 0 && frac_digits == 0 {
        return Err(invalid());
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        clean.push('e');
        i = i.saturating_add(1);
        if let Some(&c @ (b'-' | b'+')) = b.get(i) {
            clean.push(char::from(c));
            i = i.saturating_add(1);
        }
        if take_digits(&mut i, &mut clean)? == 0 {
            return Err(invalid());
        }
    }
    if i != b.len() {
        return Err(invalid());
    }
    let v: f64 = clean.parse().map_err(|_| invalid())?;
    Ok(if t.starts_with('-') { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(s: &str, base: u32) -> Result<String, String> {
        py_int(s.as_bytes(), base).and_then(|v| v.to_decimal())
    }

    #[test]
    fn ints() {
        assert_eq!(dec("  -12 ", 10).as_deref(), Ok("-12"));
        assert_eq!(dec("1_000", 10).as_deref(), Ok("1000"));
        assert_eq!(dec("0x_1f", 16).as_deref(), Ok("31"));
        assert_eq!(dec("ff", 16).as_deref(), Ok("255"));
        assert_eq!(dec("١٢", 10).as_deref(), Ok("12"));
        assert_eq!(
            dec("", 10),
            Err("invalid literal for int() with base 10: ''".to_owned())
        );
        assert_eq!(
            dec("_1", 10),
            Err("invalid literal for int() with base 10: '_1'".to_owned())
        );
        assert_eq!(
            dec("12345678901234567890123", 10).as_deref(),
            Ok("12345678901234567890123")
        );
        assert!(
            dec(&"9".chars().cycle().take(4301).collect::<String>(), 10)
                .unwrap_err()
                .contains("value has 4301 digits")
        );
        let big = py_int(
            "f".chars()
                .cycle()
                .take(5000)
                .collect::<String>()
                .as_bytes(),
            16,
        )
        .unwrap();
        assert!(
            big.to_decimal()
                .unwrap_err()
                .starts_with("Exceeds the limit (4300 digits)")
        );
    }

    #[test]
    fn floats() {
        assert_eq!(py_float(b" 1.5 "), Ok(1.5));
        assert_eq!(py_float(b"-Infinity"), Ok(f64::NEG_INFINITY));
        assert_eq!(py_float(b"1e999"), Ok(f64::INFINITY));
        assert_eq!(py_float(b"1."), Ok(1.0));
        assert_eq!(py_float(b".5"), Ok(0.5));
        assert_eq!(
            py_float(b"abc"),
            Err("could not convert string to float: 'abc'".to_owned())
        );
        assert!(py_float(b"1e").is_err());
        assert!(py_float(b".").is_err());
    }

    #[test]
    fn float_int_equality() {
        let two53 = BigInt::from_u64(1 << 53);
        assert!(two53.eq_f64(9_007_199_254_740_992.0));
        assert!(!BigInt::from_u64((1 << 53) + 1).eq_f64(9_007_199_254_740_992.0));
        assert!(BigInt::from_u64(1).eq_f64(1.0));
        assert!(BigInt::default().eq_f64(-0.0));
        assert_eq!(
            BigInt::from_f64(1e20)
                .and_then(|b| b.to_decimal().ok())
                .as_deref(),
            Some("100000000000000000000")
        );
    }
}
