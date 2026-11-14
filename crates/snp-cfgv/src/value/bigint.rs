//! Arbitrary-precision integers (storage, conversion and comparison only).

use std::cmp::Ordering;
use std::fmt::{self, Write};
use std::str::FromStr;

/// An arbitrary-precision integer, as produced by PyYAML for integers that do
/// not fit in an `i64`.
///
/// cfgv never does arithmetic on integers; it only needs their type name,
/// their decimal text for `repr()`, equality and ordering. A `BigInt` is
/// stored as a sign and a binary magnitude, so that parsing hexadecimal,
/// octal and binary text takes linear time whatever its length.
///
/// # CPython's integer string conversion limit
///
/// CPython refuses to convert between `int` and decimal text when the text
/// has more than 4300 digits ([`BigInt::MAX_STR_DIGITS`], the default of
/// `sys.set_int_max_str_digits()`), because the conversion is quadratic.
/// cfgv applies the same limit:
///
/// * [`BigInt::from_str_radix`] and [`FromStr`] reject more than 4300 digits
///   in any radix that is not a power of two, with CPython's message
///   ([`ParseBigIntError::ExceedsMaxStrDigits`]). Radixes 2, 4, 8, 16 and 32
///   are not limited, as in CPython, so PyYAML's hexadecimal, octal and
///   binary literals of any length can be represented.
/// * [`BigInt::to_decimal_string`] (and [`Value::try_repr`]) fail with
///   [`MaxStrDigitsError`] when the decimal text would have more than 4300
///   digits, where CPython's `str()`/`repr()` raise `ValueError`.
/// * [`Display`](fmt::Display), which cannot fail, writes such a value in
///   hexadecimal instead (`0x...`, a Python literal of the same value).
///
/// [`Value::from`](crate::Value) normalises a `BigInt` that fits in an `i64`
/// to [`Value::Int`](crate::Value::Int); values constructed directly as
/// `Value::BigInt` still compare and hash equal to the corresponding
/// `Value::Int`.
///
/// [`Value::try_repr`]: crate::Value::try_repr
///
/// ```
/// use cfgv::BigInt;
///
/// let big: BigInt = "-000123456789012345678901234567890".parse().unwrap();
/// assert_eq!(big.to_string(), "-123456789012345678901234567890");
/// assert_eq!(BigInt::from_str_radix("ff", 16).unwrap().to_i64(), Some(255));
///
/// // Hexadecimal input of any size; decimal output is limited like CPython.
/// let huge = BigInt::from_str_radix(&"f".repeat(4000), 16).unwrap();
/// assert_eq!(huge.bit_length(), 16_000);
/// assert!(huge.to_decimal_string().is_err());
/// assert!(huge.to_string().starts_with("0xfff"));
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BigInt {
    /// Whether the integer is negative (never set for zero).
    negative: bool,
    /// Little-endian base-2^32 limbs without high zero limbs; empty for
    /// zero.
    magnitude: Box<[u32]>,
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
    /// The input has more digits than CPython converts in a radix that is
    /// not a power of two ([`BigInt::MAX_STR_DIGITS`]); `Display` is
    /// CPython's `ValueError` message.
    ExceedsMaxStrDigits {
        /// The number of digits of the input.
        digits: usize,
    },
}

impl fmt::Display for ParseBigIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("cannot parse integer from empty string"),
            Self::InvalidDigit => f.write_str("invalid digit found in string"),
            Self::InvalidRadix => f.write_str("radix must be in 2..=36"),
            Self::ExceedsMaxStrDigits { digits } => write!(
                f,
                "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion: \
                 value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ),
        }
    }
}

impl std::error::Error for ParseBigIntError {}

/// CPython's `ValueError` for converting an `int` whose decimal text has
/// more than [`BigInt::MAX_STR_DIGITS`] digits to a string.
///
/// Returned by [`BigInt::to_decimal_string`], [`Value::try_repr`] and
/// [`Value::try_to_string`]. It converts into an
/// [uncaught](crate::ValidationError::uncaught) `ValueError`, which is how
/// cfgv reports a message that Python could not have formatted.
///
/// [`Value::try_repr`]: crate::Value::try_repr
/// [`Value::try_to_string`]: crate::Value::try_to_string
///
/// ```
/// use cfgv::MaxStrDigitsError;
///
/// assert_eq!(
///     MaxStrDigitsError.to_string(),
///     "Exceeds the limit (4300 digits) for integer string conversion; \
///      use sys.set_int_max_str_digits() to increase the limit",
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct MaxStrDigitsError;

impl fmt::Display for MaxStrDigitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Exceeds the limit ({MAX_STR_DIGITS} digits) for integer string conversion; \
             use sys.set_int_max_str_digits() to increase the limit"
        )
    }
}

impl std::error::Error for MaxStrDigitsError {}

/// CPython's default `sys.get_int_max_str_digits()`.
const MAX_STR_DIGITS: usize = 4300;

/// Integers below `2**FITS_BITS` have at most 4300 decimal digits
/// (`2**14284 < 10**4300`).
const FITS_BITS: u64 = 14_284;

/// Integers of at least `2**(EXCEEDS_BITS - 1)` have more than 4300 decimal
/// digits (`2**14286 > 10**4300`).
const EXCEEDS_BITS: u64 = 14_287;

/// `10**9`, the base of the decimal chunks used for conversions.
const DECIMAL_CHUNK: u32 = 1_000_000_000;

/// The low 32 bits of `x`.
#[allow(clippy::cast_possible_truncation, reason = "truncation is the point")]
const fn low32(x: u64) -> u32 {
    x as u32
}

/// The low 32 bits of `x`.
#[allow(clippy::cast_possible_truncation, reason = "truncation is the point")]
const fn low32_u128(x: u128) -> u32 {
    x as u32
}

/// The value of an ASCII digit that has already been validated.
fn digit_value(b: u8) -> u32 {
    char::from(b).to_digit(36).unwrap_or(0)
}

/// Removes high zero limbs.
fn trim(limbs: &mut Vec<u32>) {
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
}

/// `limbs = limbs * mul + add`.
fn mul_add_small(limbs: &mut Vec<u32>, mul: u32, add: u32) {
    let mut carry = u64::from(add);
    for limb in limbs.iter_mut() {
        // At most (2**32 - 1)**2 + 2**32 - 1 < 2**64.
        let acc = u64::from(*limb) * u64::from(mul) + carry;
        *limb = low32(acc);
        carry = acc >> 32;
    }
    if carry > 0 {
        limbs.push(low32(carry));
    }
}

/// `limbs = limbs / div`, returning the remainder.
fn div_rem_small(limbs: &mut Vec<u32>, div: u32) -> u32 {
    let div = u64::from(div.max(1));
    let mut rem = 0u64;
    for limb in limbs.iter_mut().rev() {
        let acc = (rem << 32) | u64::from(*limb);
        *limb = low32(acc / div);
        rem = acc % div;
    }
    trim(limbs);
    low32(rem)
}

/// Packs digits of a power-of-two radix (`bits` bits each) into limbs, in
/// linear time.
fn from_binary_digits(digits: &[u8], bits: u32) -> Vec<u32> {
    let capacity = digits.len().saturating_mul(bits as usize) / 32 + 1;
    let mut limbs = Vec::with_capacity(capacity);
    let mut acc = 0u64;
    let mut acc_bits = 0u32;
    for &b in digits.iter().rev() {
        acc |= u64::from(digit_value(b)) << acc_bits;
        acc_bits += bits;
        if acc_bits >= 32 {
            limbs.push(low32(acc));
            acc >>= 32;
            acc_bits -= 32;
        }
    }
    if acc_bits > 0 {
        limbs.push(low32(acc));
    }
    trim(&mut limbs);
    limbs
}

/// Converts digits of any radix, a chunk of digits at a time. Quadratic, so
/// only used for inputs within CPython's digit limit.
fn from_digits(digits: &[u8], radix: u32) -> Vec<u32> {
    // The largest power of `radix` that fits in a `u32`, and its exponent.
    let mut chunk_pow = radix;
    let mut chunk_len = 1;
    while let Some(next) = chunk_pow.checked_mul(radix) {
        chunk_pow = next;
        chunk_len += 1;
    }
    let mut limbs = Vec::new();
    for chunk in digits.rchunks(chunk_len).rev() {
        // Both stay below `radix**chunk_len`, which fits in a `u32`.
        let value = chunk.iter().fold(0u32, |acc, &b| {
            acc.wrapping_mul(radix).wrapping_add(digit_value(b))
        });
        let mul = if chunk.len() == chunk_len {
            chunk_pow
        } else {
            chunk.iter().fold(1u32, |acc, _| acc.wrapping_mul(radix))
        };
        mul_add_small(&mut limbs, mul, value);
    }
    trim(&mut limbs);
    limbs
}

fn split_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else {
        (false, s.strip_prefix('+').unwrap_or(s))
    }
}

impl BigInt {
    /// CPython's default limit on the number of decimal digits converted
    /// between `int` and `str` (`sys.get_int_max_str_digits()`).
    pub const MAX_STR_DIGITS: usize = MAX_STR_DIGITS;

    /// Builds the canonical form.
    fn from_parts(negative: bool, mut magnitude: Vec<u32>) -> Self {
        trim(&mut magnitude);
        Self {
            negative: negative && !magnitude.is_empty(),
            magnitude: magnitude.into_boxed_slice(),
        }
    }

    /// Whether the integer is negative.
    #[must_use]
    pub const fn is_negative(&self) -> bool {
        self.negative
    }

    /// The number of bits of the magnitude (Python's `int.bit_length()`).
    #[must_use]
    pub fn bit_length(&self) -> u64 {
        match self.magnitude.split_last() {
            None => 0,
            Some((top, rest)) => (rest.len() as u64)
                .saturating_mul(32)
                .saturating_add(u64::from(32 - top.leading_zeros())),
        }
    }

    /// The magnitude, if it fits in a `u128`.
    fn magnitude_u128(&self) -> Option<u128> {
        if self.magnitude.len() > 4 {
            return None;
        }
        Some(
            self.magnitude
                .iter()
                .rev()
                .fold(0u128, |acc, &limb| (acc << 32) | u128::from(limb)),
        )
    }

    /// The integer as an `i64`, if it fits.
    #[must_use]
    pub fn to_i64(&self) -> Option<i64> {
        self.to_i128().and_then(|i| i64::try_from(i).ok())
    }

    /// The integer as a `u64`, if it fits.
    #[must_use]
    pub fn to_u64(&self) -> Option<u64> {
        self.to_u128().and_then(|u| u64::try_from(u).ok())
    }

    /// The integer as an `i128`, if it fits.
    #[must_use]
    pub fn to_i128(&self) -> Option<i128> {
        let magnitude = self.magnitude_u128()?;
        if !self.negative {
            return i128::try_from(magnitude).ok();
        }
        if magnitude == i128::MIN.unsigned_abs() {
            return Some(i128::MIN);
        }
        i128::try_from(magnitude).ok().map(|m| -m)
    }

    /// The integer as a `u128`, if it fits.
    #[must_use]
    pub fn to_u128(&self) -> Option<u128> {
        if self.negative {
            None
        } else {
            self.magnitude_u128()
        }
    }

    /// Parses an integer written in `radix` (`2..=36`), with an optional
    /// leading `+` or `-` and ASCII digits/letters (case-insensitive), like
    /// Python's `int(s, radix)` without prefixes, underscores or whitespace.
    ///
    /// Radixes that are powers of two are converted in linear time and
    /// accept any number of digits. Other radixes accept at most
    /// [`BigInt::MAX_STR_DIGITS`] digits, like CPython.
    ///
    /// # Errors
    ///
    /// Returns [`ParseBigIntError`] for an invalid radix, an empty digit
    /// string, an invalid digit, or too many digits in a radix that is not a
    /// power of two (checked after the digits, as CPython does).
    ///
    /// ```
    /// use cfgv::{BigInt, ParseBigIntError};
    ///
    /// assert!(BigInt::from_str_radix(&"7".repeat(10_000), 8).is_ok());
    /// let err = "1".repeat(4301).parse::<BigInt>().unwrap_err();
    /// assert_eq!(err, ParseBigIntError::ExceedsMaxStrDigits { digits: 4301 });
    /// assert_eq!(
    ///     err.to_string(),
    ///     "Exceeds the limit (4300 digits) for integer string conversion: \
    ///      value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit",
    /// );
    /// ```
    pub fn from_str_radix(s: &str, radix: u32) -> Result<Self, ParseBigIntError> {
        if !(2..=36).contains(&radix) {
            return Err(ParseBigIntError::InvalidRadix);
        }
        let (negative, digits) = split_sign(s);
        if digits.is_empty() {
            return Err(ParseBigIntError::Empty);
        }
        // Non-ASCII bytes are never digits, so once every byte is validated
        // the byte count is the digit count.
        let digits = digits.as_bytes();
        if !digits.iter().all(|&b| char::from(b).is_digit(radix)) {
            return Err(ParseBigIntError::InvalidDigit);
        }
        let magnitude = if radix.is_power_of_two() {
            from_binary_digits(digits, radix.trailing_zeros())
        } else if digits.len() > MAX_STR_DIGITS {
            return Err(ParseBigIntError::ExceedsMaxStrDigits {
                digits: digits.len(),
            });
        } else {
            from_digits(digits, radix)
        };
        Ok(Self::from_parts(negative, magnitude))
    }

    /// The decimal digits of the magnitude, if there are at most
    /// [`BigInt::MAX_STR_DIGITS`] of them.
    fn decimal_magnitude(&self) -> Result<String, MaxStrDigitsError> {
        if self.bit_length() >= EXCEEDS_BITS {
            return Err(MaxStrDigitsError);
        }
        let mut limbs = self.magnitude.to_vec();
        let mut chunks = Vec::with_capacity(limbs.len() * 32 / 29 + 1);
        while !limbs.is_empty() {
            chunks.push(div_rem_small(&mut limbs, DECIMAL_CHUNK));
        }
        let mut text = String::with_capacity(chunks.len() * 9);
        match chunks.split_last() {
            None => text.push('0'),
            Some((top, rest)) => {
                let _ = write!(text, "{top}");
                for chunk in rest.iter().rev() {
                    let _ = write!(text, "{chunk:09}");
                }
            }
        }
        if text.len() > MAX_STR_DIGITS {
            Err(MaxStrDigitsError)
        } else {
            Ok(text)
        }
    }

    /// Whether the decimal text has more than [`BigInt::MAX_STR_DIGITS`]
    /// digits, i.e. whether CPython's `str()` and `repr()` raise
    /// `ValueError`.
    #[must_use]
    pub fn exceeds_max_str_digits(&self) -> bool {
        let bits = self.bit_length();
        if bits <= FITS_BITS {
            false
        } else {
            bits >= EXCEEDS_BITS || self.decimal_magnitude().is_err()
        }
    }

    /// The decimal text, like CPython's `str(int)`.
    ///
    /// # Errors
    ///
    /// Returns [`MaxStrDigitsError`] when the text would have more than
    /// [`BigInt::MAX_STR_DIGITS`] digits (the sign is not counted), where
    /// CPython raises `ValueError`.
    pub fn to_decimal_string(&self) -> Result<String, MaxStrDigitsError> {
        let digits = self.decimal_magnitude()?;
        Ok(if self.negative {
            format!("-{digits}")
        } else {
            digits
        })
    }

    /// The hexadecimal digits of the magnitude.
    fn hex_magnitude(&self, upper: bool) -> String {
        let mut text = String::with_capacity(self.magnitude.len().saturating_mul(8));
        match self.magnitude.split_last() {
            None => text.push('0'),
            Some((top, rest)) => {
                let _ = if upper {
                    write!(text, "{top:X}")
                } else {
                    write!(text, "{top:x}")
                };
                for limb in rest.iter().rev() {
                    let _ = if upper {
                        write!(text, "{limb:08X}")
                    } else {
                        write!(text, "{limb:08x}")
                    };
                }
            }
        }
        text
    }

    /// The integer equal to `f`, if `f` is finite and integral (exact for
    /// every magnitude).
    pub(crate) fn from_integral_f64(f: f64) -> Option<Self> {
        if !f.is_finite() || f.fract() != 0.0 {
            return None;
        }
        let bits = f.to_bits();
        let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
        if exponent == 0 {
            // Zero: subnormal floats are never integral.
            return Some(Self::from(0));
        }
        let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
        let shift = exponent - 1075;
        let magnitude = if let Ok(shift) = u32::try_from(shift) {
            let wide = u128::from(mantissa) << (shift % 32);
            let mut limbs = vec![0u32; (shift / 32) as usize];
            limbs.extend((0..3).map(|i| low32_u128(wide >> (32 * i))));
            limbs
        } else {
            // `f` is integral, so the bits shifted out are zero, and
            // `|f| >= 1` bounds the shift by 52.
            let value = mantissa >> shift.unsigned_abs().min(63);
            vec![low32(value), low32(value >> 32)]
        };
        Some(Self::from_parts(f < 0.0, magnitude))
    }
}

impl FromStr for BigInt {
    type Err = ParseBigIntError;

    /// Parses decimal text with an optional sign (`[+-]?[0-9]+`), at most
    /// [`BigInt::MAX_STR_DIGITS`] digits.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_str_radix(s, 10)
    }
}

impl Ord for BigInt {
    fn cmp(&self, other: &Self) -> Ordering {
        let by_magnitude = || {
            self.magnitude
                .len()
                .cmp(&other.magnitude.len())
                .then_with(|| {
                    self.magnitude
                        .iter()
                        .rev()
                        .cmp(other.magnitude.iter().rev())
                })
        };
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => by_magnitude(),
            (true, true) => by_magnitude().reverse(),
        }
    }
}

impl PartialOrd for BigInt {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for BigInt {
    /// The decimal text (Python's `str()`), or, when that would exceed
    /// CPython's 4300-digit limit, the hexadecimal Python literal of the
    /// same value (`0x...`, `-0x...`), where CPython raises `ValueError`.
    /// Honours width, fill and the `+` flag.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(digits) = self.decimal_magnitude() {
            f.pad_integral(!self.negative, "", &digits)
        } else {
            let hex = self.hex_magnitude(false);
            f.pad_integral(!self.negative, "", &format!("0x{hex}"))
        }
    }
}

impl fmt::Debug for BigInt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::LowerHex for BigInt {
    /// Hexadecimal digits with a `-` sign for negative values; the `#` flag
    /// adds the `0x` prefix (`format!("{n:#x}")` is Python's `hex(n)`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.negative, "0x", &self.hex_magnitude(false))
    }
}

impl fmt::UpperHex for BigInt {
    /// Like [`LowerHex`](fmt::LowerHex), with upper-case digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad_integral(!self.negative, "0x", &self.hex_magnitude(true))
    }
}

impl From<u128> for BigInt {
    fn from(n: u128) -> Self {
        let magnitude = (0..4).map(|i| low32_u128(n >> (32 * i))).collect();
        Self::from_parts(false, magnitude)
    }
}

impl From<i128> for BigInt {
    fn from(n: i128) -> Self {
        let magnitude = Self::from(n.unsigned_abs()).magnitude.into_vec();
        Self::from_parts(n < 0, magnitude)
    }
}

macro_rules! bigint_from_int {
    ($wide:ty => $($t:ty),*) => {$(
        impl From<$t> for BigInt {
            fn from(n: $t) -> Self {
                Self::from(<$wide>::from(n))
            }
        }
    )*};
}

bigint_from_int!(i128 => i8, i16, i32, i64);
bigint_from_int!(u128 => u8, u16, u32, u64);

impl From<isize> for BigInt {
    fn from(n: isize) -> Self {
        // Lossless: `isize` is at most 64 bits wide on supported targets.
        Self::from(n as i64)
    }
}

impl From<usize> for BigInt {
    fn from(n: usize) -> Self {
        // Lossless: `usize` is at most 64 bits wide on supported targets.
        Self::from(n as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<String, ParseBigIntError> {
        s.parse::<BigInt>().map(|b| b.to_string())
    }

    fn big(s: &str, radix: u32) -> BigInt {
        BigInt::from_str_radix(s, radix).unwrap_or_else(|_| BigInt::from(0))
    }

    #[test]
    fn canonical_parsing() {
        assert_eq!(parse("0").as_deref(), Ok("0"));
        assert_eq!(parse("-0").as_deref(), Ok("0"));
        assert_eq!(parse("+007").as_deref(), Ok("7"));
        assert_eq!(parse("-0012").as_deref(), Ok("-12"));
        assert_eq!(parse(""), Err(ParseBigIntError::Empty));
        assert_eq!(parse("-"), Err(ParseBigIntError::Empty));
        assert_eq!(parse("1_0"), Err(ParseBigIntError::InvalidDigit));
        assert_eq!(parse("١"), Err(ParseBigIntError::InvalidDigit));
        assert!(!big("-0", 10).is_negative());
        assert!(!big("-0", 16).is_negative());
    }

    #[test]
    fn radix() {
        let r = |s: &str, radix| BigInt::from_str_radix(s, radix).map(|b| b.to_string());
        assert_eq!(r("ff", 16).as_deref(), Ok("255"));
        assert_eq!(r("-FF", 16).as_deref(), Ok("-255"));
        assert_eq!(r("777", 8).as_deref(), Ok("511"));
        assert_eq!(r("101", 2).as_deref(), Ok("5"));
        assert_eq!(r("zz", 36).as_deref(), Ok("1295"));
        assert_eq!(r("v", 32).as_deref(), Ok("31"));
        assert_eq!(r("33", 4).as_deref(), Ok("15"));
        assert_eq!(r("0000ff", 16).as_deref(), Ok("255"));
        assert_eq!(
            r("ffffffffffffffffffffffffffffffff", 16).as_deref(),
            Ok("340282366920938463463374607431768211455")
        );
        assert_eq!(
            r("100000000000000000000000000000000", 16).as_deref(),
            Ok("340282366920938463463374607431768211456")
        );
        assert_eq!(r("1000000000", 10).as_deref(), Ok("1000000000"));
        assert_eq!(r("2", 2), Err(ParseBigIntError::InvalidDigit));
        assert_eq!(r("g", 16), Err(ParseBigIntError::InvalidDigit));
        assert_eq!(r("1", 1), Err(ParseBigIntError::InvalidRadix));
        assert_eq!(r("1", 37), Err(ParseBigIntError::InvalidRadix));
    }

    #[test]
    fn max_str_digits_on_parse() {
        let at_limit = "9".repeat(MAX_STR_DIGITS);
        assert_eq!(parse(&at_limit).as_deref(), Ok(at_limit.as_str()));
        assert_eq!(
            parse(&"0".repeat(5000)),
            Err(ParseBigIntError::ExceedsMaxStrDigits { digits: 5000 })
        );
        assert_eq!(
            parse(&format!("-{}", "1".repeat(4301))),
            Err(ParseBigIntError::ExceedsMaxStrDigits { digits: 4301 })
        );
        // Invalid digits are reported first, like CPython.
        assert_eq!(
            parse(&format!("{}x", "1".repeat(5000))),
            Err(ParseBigIntError::InvalidDigit)
        );
        assert_eq!(
            BigInt::from_str_radix(&"z".repeat(4301), 36),
            Err(ParseBigIntError::ExceedsMaxStrDigits { digits: 4301 })
        );
        // Power-of-two radixes are not limited.
        for radix in [2, 4, 8, 16, 32] {
            assert!(BigInt::from_str_radix(&"1".repeat(20_000), radix).is_ok());
        }
    }

    #[test]
    fn max_str_digits_on_output() {
        let largest = big(&"9".repeat(MAX_STR_DIGITS), 10);
        assert_eq!(largest.to_decimal_string().map(|s| s.len()), Ok(4300));
        assert!(!largest.exceeds_max_str_digits());
        // The sign is not counted.
        let negative = big(&format!("-{}", "9".repeat(4300)), 10);
        assert_eq!(negative.to_decimal_string().map(|s| s.len()), Ok(4301));
        // 10**4300 has 4301 digits.
        let mut limbs = vec![1];
        for _ in 0..4300 {
            mul_add_small(&mut limbs, 10, 0);
        }
        let too_big = BigInt::from_parts(false, limbs);
        assert!(matches!(too_big.bit_length(), 14_285..=14_286));
        assert_eq!(too_big.to_decimal_string(), Err(MaxStrDigitsError));
        assert!(too_big.exceeds_max_str_digits());
        assert!(too_big.to_string().starts_with("0x"));
        let minus = BigInt::from_parts(true, too_big.magnitude.to_vec());
        assert!(minus.to_string().starts_with("-0x"));
        assert!(minus.exceeds_max_str_digits());
    }

    #[test]
    fn hex_formatting() {
        let n = BigInt::from(-255);
        assert_eq!(format!("{n:x}"), "-ff");
        assert_eq!(format!("{n:#X}"), "-0xFF");
        assert_eq!(format!("{:#x}", BigInt::from(0)), "0x0");
        assert_eq!(format!("{:x}", BigInt::from(u64::MAX)), "ffffffffffffffff");
        assert_eq!(format!("{:>6}", BigInt::from(-12)), "   -12");
        assert_eq!(format!("{:+}", BigInt::from(12)), "+12");
        let hex = "1".to_owned() + &"0".repeat(40);
        assert_eq!(format!("{:x}", big(&hex, 16)), hex);
    }

    #[test]
    fn ordering() {
        let b = |s: &str| big(s, 10);
        assert!(b("-100") < b("-99"));
        assert!(b("-1") < b("0"));
        assert!(b("99") < b("100"));
        assert!(b("123") < b("124"));
        assert!(b("-100000000000000000000") < b("-99999999999999999999"));
        assert!(b("4294967296") > b("4294967295"));
        assert_eq!(b("5").cmp(&b("005")), Ordering::Equal);
    }

    #[test]
    fn conversions() {
        assert_eq!(BigInt::from(i64::MIN).to_i64(), Some(i64::MIN));
        assert_eq!(BigInt::from(u64::MAX).to_i64(), None);
        assert_eq!(BigInt::from(u64::MAX).to_u64(), Some(u64::MAX));
        assert_eq!(BigInt::from(u64::MAX).to_i128(), Some(i128::from(u64::MAX)));
        assert_eq!(BigInt::from(i128::MIN).to_i128(), Some(i128::MIN));
        assert_eq!(BigInt::from(i128::MIN).to_string(), i128::MIN.to_string());
        assert_eq!(BigInt::from(u128::MAX).to_u128(), Some(u128::MAX));
        assert_eq!(BigInt::from(u128::MAX).to_string(), u128::MAX.to_string());
        assert_eq!(BigInt::from(u128::MAX).to_i128(), None);
        assert_eq!(BigInt::from(-1).to_u64(), None);
        assert!(BigInt::from(-3).is_negative());
        assert_eq!(BigInt::from(0).bit_length(), 0);
        assert_eq!(BigInt::from(255u8).bit_length(), 8);
        assert_eq!(BigInt::from(-256).bit_length(), 9);
        assert_eq!(BigInt::from(usize::MAX).to_u128(), Some(usize::MAX as u128));
        assert_eq!(BigInt::from(isize::MIN).to_i128(), Some(isize::MIN as i128));
    }

    #[test]
    fn from_integral_f64() {
        let text = |f: f64| BigInt::from_integral_f64(f).map(|b| b.to_string());
        assert_eq!(text(0.0).as_deref(), Some("0"));
        assert_eq!(text(-0.0).as_deref(), Some("0"));
        assert_eq!(text(1.0).as_deref(), Some("1"));
        assert_eq!(text(-3.0).as_deref(), Some("-3"));
        assert_eq!(
            text(4_503_599_627_370_497.0).as_deref(),
            Some("4503599627370497")
        );
        assert_eq!(text(1e20).as_deref(), Some("100000000000000000000"));
        assert_eq!(text(2f64.powi(63)).as_deref(), Some("9223372036854775808"));
        assert_eq!(text(f64::MAX), Some(format!("{:.0}", f64::MAX)));
        assert_eq!(text(-f64::MAX), Some(format!("{:.0}", -f64::MAX)));
        assert_eq!(text(0.5), None);
        assert_eq!(text(f64::NAN), None);
        assert_eq!(text(f64::INFINITY), None);
    }
}
