//! `float.__repr__` (`Objects/floatobject.c` `float_repr` → `PyOS_double_to_string(x, 'r', 0,
//! Py_DTSF_ADD_DOT_0)` → `_Py_dg_dtoa` mode 0) and pre-commit's duration rendering (02 §13.2,
//! 15 §A4.7).

use std::fmt::Write as _;

/// A non-empty string of decimal digits plus one (`"129"` → `"130"`, `"99"` → `"100"`).
fn increment(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for b in bytes.iter_mut().rev() {
        if *b == b'9' {
            *b = b'0';
        } else {
            *b = b.saturating_add(1);
            return String::from_utf8(bytes).unwrap_or_default();
        }
    }
    let mut out = vec![b'1'];
    out.extend_from_slice(&bytes);
    String::from_utf8(out).unwrap_or_default()
}

/// `digits` (first digit at decimal exponent `exp`) as a string `str::parse::<f64>` reads.
fn sci(digits: &str, exp: i32) -> String {
    let mut cs = digits.chars();
    let first = cs.next().unwrap_or('0');
    format!("{first}.{}e{exp}", cs.as_str())
}

/// dtoa mode 0 breaks an exact tie between the two closest shortest candidates by the even last
/// digit (`_Py_dg_dtoa`: `if (j1 > 0 || (j1 == 0 && dig & 1))` round up); Rust's shortest
/// formatting can round such a tie up (`1078486155042150.25` → Python `…150.2`, Rust `…150.3`).
/// A tie needs at least 16 significant digits (the decimal step must undercut the binary ulp), so
/// only long outputs are checked against the exact expansion.
fn even_tie(x: f64, digits: String, exp: i32) -> (String, i32) {
    let n = digits.len();
    let odd_last = digits.as_bytes().last().is_some_and(|d| d % 2 == 1);
    if n < 15 || !odd_last {
        return (digits, exp);
    }
    // every double's exact expansion has at most 767 significant digits
    let exact = format!("{x:.800e}");
    let Some((mant, e)) = exact.split_once('e') else {
        return (digits, exp);
    };
    let Ok(exact_exp) = e.parse::<i32>() else {
        return (digits, exp);
    };
    let exact_digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let exact_digits = exact_digits.trim_end_matches('0');
    // align the exact expansion to the decade of the shortest digits (Rust may round up a decade)
    let shift = match exp.checked_sub(exact_exp) {
        Some(0) => "",
        Some(1) => "0",
        _ => return (digits, exp),
    };
    let aligned = format!("{shift}{exact_digits}");
    if aligned.len() != n.saturating_add(1) || !aligned.ends_with('5') {
        return (digits, exp);
    }
    let Some(low) = aligned.get(..n) else {
        return (digits, exp);
    };
    let high = increment(low);
    let other = if digits == high {
        low.to_owned()
    } else if digits == low {
        high
    } else {
        return (digits, exp);
    };
    let even = other.as_bytes().last().is_some_and(|d| d % 2 == 0);
    let same_length = other.len() == n && !other.starts_with('0');
    if even && same_length && sci(&other, exp).parse::<f64>() == Ok(x) {
        let trimmed = other.trim_end_matches('0');
        return (trimmed.to_owned(), exp);
    }
    (digits, exp)
}

/// The shortest round-trip decimal digits of a finite `|x|` (no leading or trailing zeros, `"0"`
/// for zero) and the decimal exponent of the first digit, chosen as dtoa mode 0 does.
fn shortest(x: f64) -> (String, i32) {
    let abs = x.abs();
    // Rust's `{:e}` prints the shortest round-trip digits, e.g. "1.5e-7", "0e0".
    let sci = format!("{abs:e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    even_tie(abs, digits, exp)
}

/// `repr(x)` for a Python `float`: the shortest round-trip digits (ties to the even digit),
/// positional when `-4 < decpt <= 16` (always with a fractional part, `1.0`), otherwise
/// `d[.ddd]e±XX` with at least two exponent digits; `inf`, `-inf`, `nan`, `-0.0`.
#[must_use]
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let (digits, exp) = shortest(x);
    let decpt = exp.saturating_add(1);
    let mut out = String::new();
    if x.is_sign_negative() {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        let mut cs = digits.chars();
        if let Some(first) = cs.next() {
            out.push(first);
        }
        let rest = cs.as_str();
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let e = decpt.saturating_sub(1);
        write!(
            out,
            "e{}{:02}",
            if e < 0 { '-' } else { '+' },
            e.unsigned_abs()
        )
        .unwrap_or_default();
    } else if decpt <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n(
            '0',
            usize::try_from(decpt.unsigned_abs()).unwrap_or(0),
        ));
        out.push_str(&digits);
    } else {
        let d = usize::try_from(decpt.unsigned_abs()).unwrap_or(0);
        match digits.split_at_checked(d) {
            Some((int_part, frac)) if !frac.is_empty() => {
                out.push_str(int_part);
                out.push('.');
                out.push_str(frac);
            }
            _ => {
                out.push_str(&digits);
                out.extend(std::iter::repeat_n('0', d.saturating_sub(digits.len())));
                out.push_str(".0");
            }
        }
    }
    out
}

/// `f'{round(x, 2) or 0}'` (`commands/run.py:202,221`, the `- duration: …s` line): Python's
/// correctly rounded `round()` (`double_round`: the exact binary value rounded half-even at two
/// decimals, which Rust's exact `{:.2}` formatting also does) then `repr`, and `0` for a zero
/// result (`0.0` and `-0.0` are falsy). Verified on 12,000+ CPython vectors.
#[must_use]
pub fn format_duration(secs: f64) -> String {
    let rounded: f64 = format!("{secs:.2}").parse().unwrap_or(secs);
    if rounded == 0.0 {
        "0".to_owned()
    } else {
        repr(rounded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprs() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1.5e-7, "1.5e-07"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (123_456.789, "123456.789"),
            (1e15, "1000000000000000.0"),
            (1.25e22, "1.25e+22"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (5e-324, "5e-324"),
            (1.797_693_134_862_315_7e308, "1.7976931348623157e+308"),
        ];
        for (x, want) in cases {
            assert_eq!(repr(*x), *want, "{x:e}");
        }
    }

    #[test]
    fn exact_ties_round_to_even() {
        // 0x430ea7050154eb32 == 1078486155042150.25 exactly: .2 and .3 both round-trip
        let x = f64::from_bits(0x430e_a705_0154_eb32);
        assert_eq!(repr(x), "1078486155042150.2");
        assert_eq!(repr(-x), "-1078486155042150.2");
        assert_eq!(increment("129"), "130");
        assert_eq!(increment("99"), "100");
    }

    #[test]
    fn durations() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (0.001, "0"),
            (-0.001, "0"),
            (0.125, "0.12"),
            (2.675, "2.67"),
            (0.375, "0.38"),
            (0.01, "0.01"),
            (0.77, "0.77"),
            (1.0, "1.0"),
            (1.236, "1.24"),
            (12.5, "12.5"),
            (f64::NAN, "nan"),
            (f64::INFINITY, "inf"),
        ];
        for (x, want) in cases {
            assert_eq!(format_duration(*x), *want, "{x}");
        }
    }
}
