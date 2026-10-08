//! `int(s)` (base 10) for `clientlib.parse_version`.
use crate::repr::repr_str;
use crate::unicode::{decimal_value, is_space};

/// `ValueError` from `int()`; Display is CPython's text, whose `%.200R`
/// truncates the repr to 200 characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntParseError {
    pub literal: String,
}

impl core::fmt::Display for IntParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let r: String = repr_str(&self.literal).chars().take(200).collect();
        write!(f, "invalid literal for int() with base 10: {r}")
    }
}
impl std::error::Error for IntParseError {}

/// `int(s)` for base 10; `u64`/`i64` range only (pre-commit compares tuples of
/// small ints). Unicode `Nd` digits and single underscores between digits are accepted.
pub fn parse_int(s: &str) -> Result<i128, IntParseError> {
    let err = || IntParseError { literal: s.to_owned() };
    let t = s.trim_matches(is_space);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let mut val: i128 = 0;
    let mut prev_digit = false;
    let mut any = false;
    for c in digits.chars() {
        if c == '_' {
            if !prev_digit {
                return Err(err());
            }
            prev_digit = false;
            continue;
        }
        let d = decimal_value(c).ok_or_else(err)?;
        val = val.checked_mul(10).and_then(|v| v.checked_add(i128::from(d))).ok_or_else(err)?;
        prev_digit = true;
        any = true;
    }
    if !any || !prev_digit {
        return Err(err());
    }
    Ok(if neg { -val } else { val })
}

/// `clientlib.parse_version(s)`: `tuple(int(p) for p in s.split('.'))`.
pub fn parse_version(s: &str) -> Result<Vec<i128>, IntParseError> {
    s.split('.').map(parse_int).collect()
}
