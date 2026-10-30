//! A minimal signed arbitrary-precision integer: exactly what PyYAML's constructors need beyond `cfgv::BigInt`
//! (which stores and compares but does no arithmetic) — parsing digits, `x * 60 + d` for sexagesimal ints, and the
//! correctly rounded `int → float` conversion of `float * int` in sexagesimal floats.
//!
//! Owner: WP-14.

use std::fmt::Write as _;

use cfgv::{BigInt, Value};

/// 2³².
const B32: u64 = 0x1_0000_0000;
/// 2⁵².
const B52: u64 = 0x10_0000_0000_0000;
/// 2⁵³.
const B53: u64 = 0x20_0000_0000_0000;
/// The sign bit of an `f64`.
const SIGN: u64 = 0x8000_0000_0000_0000;

/// A signed integer as a sign and little-endian base-2³² limbs without high zero limbs (zero is `[]`, positive).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Big {
    neg: bool,
    mag: Vec<u32>,
}

/// The low 32 bits.
fn lo32(t: u64) -> u32 {
    u32::try_from(t & 0xFFFF_FFFF).unwrap_or(u32::MAX)
}

/// The high 32 bits.
fn hi32(t: u64) -> u32 {
    u32::try_from(t.wrapping_shr(32)).unwrap_or(u32::MAX)
}

impl Big {
    /// Zero.
    pub(crate) const fn zero() -> Self {
        Self {
            neg: false,
            mag: Vec::new(),
        }
    }

    /// The value of ASCII `digits` (already validated for `radix`) with the given sign.
    ///
    /// Radixes 2, 4, 8, 16 and 32 pack the bits into limbs from the least significant digit (linear: CPython has no
    /// digit limit for them, so a 16 MiB `0x…` scalar must not be quadratic); other radixes multiply in by chunks
    /// of as many digits as fit in a `u32` (CPython caps base 10 at 4300 digits).
    pub(crate) fn from_ascii_digits(neg: bool, digits: &[u8], radix: u32) -> Self {
        let mut b = if radix.is_power_of_two() && radix > 1 {
            Self::from_pow2_digits(digits, radix)
        } else {
            Self::from_digits_chunked(digits, radix)
        };
        b.trim();
        b.neg = neg && !b.mag.is_empty();
        b
    }

    /// `digits` in a power-of-two `radix`: each digit's bits go straight into place.
    fn from_pow2_digits(digits: &[u8], radix: u32) -> Self {
        let bits = radix.trailing_zeros();
        let total = u64::try_from(digits.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(bits));
        let limbs = usize::try_from(total.div_ceil(32)).unwrap_or(0);
        let mut mag = vec![0u32; limbs];
        let mut pos: u64 = 0;
        for d in digits.iter().rev() {
            let v = u64::from(char::from(*d).to_digit(radix).unwrap_or(0));
            let limb = usize::try_from(pos / 32).unwrap_or(usize::MAX);
            let shift = u32::try_from(pos % 32).unwrap_or(0);
            // `v < 2^bits` and `shift + bits <= 31 + 5`: the digit spans at most two limbs.
            let wide = v.wrapping_shl(shift);
            if let Some(l) = mag.get_mut(limb) {
                *l |= lo32(wide);
            }
            if let Some(l) = mag.get_mut(limb.saturating_add(1)) {
                *l |= hi32(wide);
            }
            pos = pos.saturating_add(u64::from(bits));
        }
        Self { neg: false, mag }
    }

    /// `digits` in any `radix`, `k` digits per multiplication where `radix^k` fits in a `u32`.
    fn from_digits_chunked(digits: &[u8], radix: u32) -> Self {
        let mut per = 1usize;
        let mut scale = radix;
        while let Some(next) = scale.checked_mul(radix) {
            scale = next;
            per = per.saturating_add(1);
        }
        let mut b = Self::zero();
        #[expect(
            clippy::disallowed_methods,
            reason = "per >= 1: it starts at 1 and only grows"
        )]
        let chunks = digits.chunks(per);
        for chunk in chunks {
            let mut m = 1u32;
            let mut a = 0u32;
            for d in chunk {
                let v = char::from(*d).to_digit(radix).unwrap_or(0);
                // At most `per` digits: `m <= scale` and `a < m`, both fit.
                m = m.wrapping_mul(radix);
                a = a.wrapping_mul(radix).wrapping_add(v);
            }
            b.mul_add_small(m, a);
        }
        b
    }

    /// The number of base-2³² limbs of the magnitude.
    pub(crate) fn limbs(&self) -> usize {
        self.mag.len()
    }

    /// `|self| = |self| * m + a` (the sign is kept).
    pub(crate) fn mul_add_small(&mut self, m: u32, a: u32) {
        let mut carry = u64::from(a);
        for limb in &mut self.mag {
            // limb * m + carry <= (2^32 - 1)^2 + 2^32 - 1 < 2^64: no wrap.
            let t = u64::from(*limb)
                .wrapping_mul(u64::from(m))
                .wrapping_add(carry);
            *limb = lo32(t);
            carry = u64::from(hi32(t));
        }
        if carry != 0 {
            self.mag.push(lo32(carry));
        }
        self.trim();
    }

    fn trim(&mut self) {
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        if self.mag.is_empty() {
            self.neg = false;
        }
    }

    /// Compares magnitudes.
    fn cmp_mag(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.iter().rev().cmp(b.iter().rev()))
    }

    /// `a + b` on magnitudes.
    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let (long, short) = if a.len() >= b.len() { (a, b) } else { (b, a) };
        let mut out = Vec::new();
        let mut carry = 0u64;
        for (i, x) in long.iter().enumerate() {
            let y = short.get(i).copied().unwrap_or(0);
            // x + y + carry < 2^34: no wrap.
            let t = u64::from(*x).wrapping_add(u64::from(y)).wrapping_add(carry);
            out.push(lo32(t));
            carry = u64::from(hi32(t));
        }
        if carry != 0 {
            out.push(lo32(carry));
        }
        out
    }

    /// `a - b` on magnitudes, `a >= b`.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::new();
        let mut borrow = 0u64;
        for (i, x) in a.iter().enumerate() {
            let y = u64::from(b.get(i).copied().unwrap_or(0)).wrapping_add(borrow);
            let x = u64::from(*x);
            if x >= y {
                out.push(lo32(x.wrapping_sub(y)));
                borrow = 0;
            } else {
                out.push(lo32(x.wrapping_add(B32).wrapping_sub(y)));
                borrow = 1;
            }
        }
        out
    }

    /// `self += other`.
    pub(crate) fn add(&mut self, other: &Self) {
        if self.neg == other.neg {
            self.mag = Self::add_mag(&self.mag, &other.mag);
        } else if Self::cmp_mag(&self.mag, &other.mag).is_ge() {
            self.mag = Self::sub_mag(&self.mag, &other.mag);
        } else {
            self.mag = Self::sub_mag(&other.mag, &self.mag);
            self.neg = other.neg;
        }
        self.trim();
    }

    /// `-self`.
    pub(crate) fn negate(&mut self) {
        if !self.mag.is_empty() {
            self.neg = !self.neg;
        }
    }

    /// The Python `int` as a `cfgv::Value` (`Int` when it fits in an `i64`, else `BigInt`).
    pub(crate) fn to_value(&self) -> Value {
        if self.mag.len() <= 2 {
            let lo = u64::from(self.mag.first().copied().unwrap_or(0));
            let hi = u64::from(self.mag.get(1).copied().unwrap_or(0));
            let m = hi.wrapping_shl(32) | lo;
            if self.neg {
                if let Some(v) = 0i64.checked_sub_unsigned(m) {
                    return Value::Int(v);
                }
            } else if let Ok(v) = i64::try_from(m) {
                return Value::Int(v);
            }
        }
        let mut hex = String::new();
        if self.neg {
            hex.push('-');
        }
        let mut limbs = self.mag.iter().rev();
        if let Some(top) = limbs.next() {
            write!(hex, "{top:x}").unwrap_or_default();
        }
        for l in limbs {
            write!(hex, "{l:08x}").unwrap_or_default();
        }
        // Radix 16 is a power of two: cfgv accepts any length.
        BigInt::from_str_radix(&hex, 16).map_or(Value::Int(0), Value::from)
    }

    /// The number of significant bits of the magnitude.
    fn bit_length(&self) -> u64 {
        match self.mag.split_last() {
            None => 0,
            Some((top, rest)) => u64::try_from(rest.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(32)
                .saturating_add(u64::from(32u32.saturating_sub(top.leading_zeros()))),
        }
    }

    /// Bit `i` of the magnitude.
    fn bit(&self, i: u64) -> bool {
        let limb = usize::try_from(i.wrapping_div(32)).unwrap_or(usize::MAX);
        let shift = u32::try_from(i.wrapping_rem(32)).unwrap_or(0);
        self.mag
            .get(limb)
            .is_some_and(|l| l.wrapping_shr(shift) & 1 == 1)
    }

    /// Whether any bit below `i` is set.
    fn any_below(&self, i: u64) -> bool {
        let full = usize::try_from(i.wrapping_div(32)).unwrap_or(usize::MAX);
        let partial = u32::try_from(i.wrapping_rem(32)).unwrap_or(0);
        if self.mag.iter().take(full).any(|l| *l != 0) {
            return true;
        }
        partial != 0
            && self
                .mag
                .get(full)
                .is_some_and(|l| l & (u32::MAX.wrapping_shr(32u32.saturating_sub(partial))) != 0)
    }

    /// `float(self)` (CPython `PyLong_AsDouble`: round half to even), `None` on `OverflowError`.
    pub(crate) fn to_f64(&self) -> Option<f64> {
        let n = self.bit_length();
        if n == 0 {
            return Some(0.0);
        }
        // The top 53 bits as the significand, `shift` bits dropped below them.
        let shift = n.saturating_sub(53);
        let mut m: u64 = 0;
        let mut i = n;
        while i > shift {
            i = i.saturating_sub(1);
            m = m.wrapping_shl(1) | u64::from(self.bit(i));
        }
        let mut exp = shift;
        if shift > 0 {
            let round = self.bit(shift.saturating_sub(1));
            let sticky = self.any_below(shift.saturating_sub(1));
            if round && (sticky || m & 1 == 1) {
                m = m.wrapping_add(1);
                if m == B53 {
                    m = B52;
                    exp = exp.saturating_add(1);
                }
            }
        }
        // value = m * 2^exp, with m < 2^53 and (for n >= 53) m >= 2^52.
        let bits_m = 64u64.saturating_sub(u64::from(m.leading_zeros()));
        let top_exp = exp.saturating_add(bits_m).saturating_sub(1);
        let biased = top_exp.saturating_add(1023);
        if biased >= 2047 {
            return None;
        }
        let norm = m.wrapping_shl(u32::try_from(53u64.saturating_sub(bits_m)).unwrap_or(0));
        let bits = biased.wrapping_shl(52)
            | (norm & (B52.wrapping_sub(1)))
            | if self.neg { SIGN } else { 0 };
        Some(f64::from_bits(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Big {
        let (neg, d) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
        Big::from_ascii_digits(neg, d.as_bytes(), 10)
    }

    #[test]
    fn values() {
        assert_eq!(parse("0").to_value(), Value::Int(0));
        assert_eq!(parse("-12").to_value(), Value::Int(-12));
        assert_eq!(
            parse("9223372036854775807").to_value(),
            Value::Int(i64::MAX)
        );
        assert_eq!(
            parse("-9223372036854775808").to_value(),
            Value::Int(i64::MIN)
        );
        assert_eq!(
            parse("123456789012345678901234567890")
                .to_value()
                .to_string(),
            "123456789012345678901234567890"
        );
        assert_eq!(
            parse("-9223372036854775809").to_value().to_string(),
            "-9223372036854775809"
        );
    }

    /// The packed (power-of-two) and chunked parsers equal digit-by-digit `x * radix + d`.
    #[test]
    fn fast_parsers_equal_the_digit_loop() {
        let mut seed = 0x2545_f491_u32;
        for radix in [2u32, 4, 8, 10, 16, 32, 36] {
            for len in [0usize, 1, 2, 7, 8, 9, 10, 31, 32, 33, 64, 65, 100, 257] {
                let digits: Vec<u8> = (0..len)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        let d = seed % radix;
                        char::from_digit(d, radix).map_or(b'0', |c| u8::try_from(c).unwrap_or(b'0'))
                    })
                    .collect();
                let mut slow = Big::zero();
                for d in &digits {
                    slow.mul_add_small(radix, char::from(*d).to_digit(radix).unwrap_or(0));
                }
                assert_eq!(
                    Big::from_ascii_digits(false, &digits, radix),
                    slow,
                    "{radix} {len}"
                );
                let leading: Vec<u8> = [b'0'; 40].iter().chain(&digits).copied().collect();
                assert_eq!(
                    Big::from_ascii_digits(false, &leading, radix),
                    slow,
                    "{radix} 0…{len}"
                );
            }
        }
        assert_eq!(Big::from_ascii_digits(true, b"000", 16), Big::zero());
    }

    #[test]
    fn arithmetic() {
        let mut a = parse("100");
        a.add(&parse("-250"));
        assert_eq!(a.to_value(), Value::Int(-150));
        a.add(&parse("150"));
        assert_eq!(a, Big::zero());
        let mut b = parse("4294967295");
        b.mul_add_small(60, 59);
        assert_eq!(b.to_value(), Value::Int(4_294_967_295 * 60 + 59));
    }

    #[test]
    fn to_f64_rounds_half_even() {
        assert_eq!(
            parse("9007199254740993").to_f64(),
            Some(9_007_199_254_740_992.0)
        );
        assert_eq!(
            parse("9007199254740995").to_f64(),
            Some(9_007_199_254_740_996.0)
        );
        assert_eq!(parse("-3").to_f64(), Some(-3.0));
        let mut p = parse("1");
        for _ in 0..173 {
            p.mul_add_small(60, 0);
        }
        assert!(p.to_f64().is_some_and(f64::is_finite));
        p.mul_add_small(60, 0);
        assert_eq!(p.to_f64(), None);
    }
}
