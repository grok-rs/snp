//! Arbitrary-precision integers (storage and comparison only).

use std::cmp::Ordering;
use std::fmt::{self, Write};
use std::str::FromStr;

/// An arbitrary-precision integer, as produced by PyYAML for integers that do
/// not fit in an `i64`.
///
/// cfgv never does arithmetic on integers; it only needs their type name,
/// their exact decimal text for `repr()`, equality and ordering. A `BigInt`
/// is therefore stored as canonical decimal text: an optional `-` followed by
/// digits without leading zeros (`"0"` for zero).
///
/// [`Value::from`](crate::Value) normalises a `BigInt` that fits in an `i64`
/// to [`Value::Int`](crate::Value::Int); values constructed directly as
/// `Value::BigInt` still compare and hash equal to the corresponding
/// `Value::Int`.
///
/// ```
/// use cfgv::BigInt;
///
/// let big: BigInt = "-000123456789012345678901234567890".parse().unwrap();
/// assert_eq!(big.as_str(), "-123456789012345678901234567890");
/// assert_eq!(BigInt::from_str_radix("ff", 16).unwrap().to_i64(), Some(255));
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BigInt {
    text: Box<str>,
}

/// Error returned when parsing a [`BigInt`] fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParseBigIntError {
    /// The input contained no digits.
    Empty,
    /// The input contained a character that is not a digit of the radix.
    InvalidDigit,
    /// The radix is outside `2..=36`.
    InvalidRadix,
}

impl fmt::Display for ParseBigIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "cannot parse integer from empty string",
            Self::InvalidDigit => "invalid digit found in string",
            Self::InvalidRadix => "radix must be in 2..=36",
        })
    }
}

impl std::error::Error for ParseBigIntError {}

impl BigInt {
    /// The canonical decimal text of the integer, e.g. `"-12"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the integer is negative.
    #[must_use]
    pub fn is_negative(&self) -> bool {
        self.text.starts_with('-')
    }

    /// The integer as an `i64`, if it fits.
    #[must_use]
    pub fn to_i64(&self) -> Option<i64> {
        self.text.parse().ok()
    }

    /// The integer as an `i128`, if it fits.
    #[must_use]
    pub fn to_i128(&self) -> Option<i128> {
        self.text.parse().ok()
    }

    /// The magnitude digits, without sign.
    fn magnitude(&self) -> &str {
        self.text.strip_prefix('-').unwrap_or(&self.text)
    }

    /// Parses an integer written in `radix` (`2..=36`), with an optional
    /// leading `+` or `-` and ASCII digits/letters (case-insensitive), like
    /// Python's `int(s, radix)` without prefixes, underscores or whitespace.
    ///
    /// # Errors
    ///
    /// Returns [`ParseBigIntError`] for an invalid radix, an empty digit
    /// string or an invalid digit.
    pub fn from_str_radix(s: &str, radix: u32) -> Result<Self, ParseBigIntError> {
        if !(2..=36).contains(&radix) {
            return Err(ParseBigIntError::InvalidRadix);
        }
        let (negative, digits) = split_sign(s);
        if digits.is_empty() {
            return Err(ParseBigIntError::Empty);
        }
        if radix == 10 {
            if !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(ParseBigIntError::InvalidDigit);
            }
            return Ok(Self::from_parts(negative, digits));
        }
        // Little-endian base-10^9 limbs; schoolbook multiply-and-add.
        let mut limbs: Vec<u32> = vec![0];
        for c in digits.chars() {
            let digit = c.to_digit(radix).ok_or(ParseBigIntError::InvalidDigit)?;
            let mut carry = u64::from(digit);
            for limb in &mut limbs {
                let acc = u64::from(*limb) * u64::from(radix) + carry;
                *limb = u32::try_from(acc % LIMB).unwrap_or(0);
                carry = acc / LIMB;
            }
            while carry > 0 {
                limbs.push(u32::try_from(carry % LIMB).unwrap_or(0));
                carry /= LIMB;
            }
        }
        let mut text = String::new();
        let mut iter = limbs.iter().rev();
        if let Some(first) = iter.next() {
            let _ = write!(text, "{first}");
        }
        for limb in iter {
            let _ = write!(text, "{limb:09}");
        }
        Ok(Self::from_parts(negative, &text))
    }

    /// Builds the canonical form from a sign and ASCII decimal digits.
    fn from_parts(negative: bool, digits: &str) -> Self {
        let trimmed = digits.trim_start_matches('0');
        let text = if trimmed.is_empty() {
            "0".into()
        } else if negative {
            format!("-{trimmed}").into_boxed_str()
        } else {
            trimmed.into()
        };
        Self { text }
    }
}

const LIMB: u64 = 1_000_000_000;

fn split_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    }
}

impl FromStr for BigInt {
    type Err = ParseBigIntError;

    /// Parses decimal text with an optional sign (`[+-]?[0-9]+`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_radix(s, 10)
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.is_negative(), other.is_negative()) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (negative, _) => {
                let (a, b) = (self.magnitude(), other.magnitude());
                let by_magnitude = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
                if negative {
                    by_magnitude.reverse()
                } else {
                    by_magnitude
                }
            }
        }
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

macro_rules! bigint_from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for BigInt {
            fn from(n: $t) -> Self {
                Self { text: n.to_string().into_boxed_str() }
            }
        }
    )*};
}

bigint_from_int!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_parsing() {
        let parse = |s: &str| s.parse::<BigInt>().map(|b| b.to_string());
        assert_eq!(parse("0").as_deref(), Ok("0"));
        assert_eq!(parse("-0").as_deref(), Ok("0"));
        assert_eq!(parse("+007").as_deref(), Ok("7"));
        assert_eq!(parse("-0012").as_deref(), Ok("-12"));
        assert_eq!(parse(""), Err(ParseBigIntError::Empty));
        assert_eq!(parse("-"), Err(ParseBigIntError::Empty));
        assert_eq!(parse("1_0"), Err(ParseBigIntError::InvalidDigit));
        assert_eq!(parse("١"), Err(ParseBigIntError::InvalidDigit));
    }

    #[test]
    fn radix() {
        let r = |s: &str, radix| BigInt::from_str_radix(s, radix).map(|b| b.to_string());
        assert_eq!(r("ff", 16).as_deref(), Ok("255"));
        assert_eq!(r("-FF", 16).as_deref(), Ok("-255"));
        assert_eq!(r("777", 8).as_deref(), Ok("511"));
        assert_eq!(r("101", 2).as_deref(), Ok("5"));
        assert_eq!(
            r("ffffffffffffffffffffffffffffffff", 16).as_deref(),
            Ok("340282366920938463463374607431768211455")
        );
        assert_eq!(r("1000000000", 10).as_deref(), Ok("1000000000"));
        assert_eq!(r("2", 2), Err(ParseBigIntError::InvalidDigit));
        assert_eq!(r("1", 1), Err(ParseBigIntError::InvalidRadix));
        assert_eq!(r("1", 37), Err(ParseBigIntError::InvalidRadix));
    }

    #[test]
    fn ordering() {
        let b = |s: &str| s.parse::<BigInt>().unwrap_or_else(|_| BigInt::from(0));
        assert!(b("-100") < b("-99"));
        assert!(b("-1") < b("0"));
        assert!(b("99") < b("100"));
        assert!(b("123") < b("124"));
        assert_eq!(b("5").cmp(&b("005")), Ordering::Equal);
    }

    #[test]
    fn conversions() {
        assert_eq!(BigInt::from(i64::MIN).to_i64(), Some(i64::MIN));
        assert_eq!(BigInt::from(u64::MAX).to_i64(), None);
        assert_eq!(BigInt::from(u64::MAX).to_i128(), Some(i128::from(u64::MAX)));
        assert!(BigInt::from(-3).is_negative());
    }
}
