//! `float.__repr__` and pre-commit's duration rendering (02 §13.2, 15 §A4.7).

use std::fmt::Write as _;

/// `repr(x)` for a Python `float`: the shortest round-trip digits, positional when
/// `-4 < decpt <= 16` (always with a fractional part, `1.0`), otherwise `d[.ddd]e±XX` with at least
/// two exponent digits; `inf`, `-inf`, `nan`, `-0.0`.
#[must_use]
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // Rust's `{:e}` prints the shortest round-trip digits, e.g. "1.5e-7", "0e0".
    let sci = format!("{:e}", x.abs());
    let (mant, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
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

/// `f'{round(x, 2) or 0}'` (`run.py` duration line): Python's correctly rounded `round()` (the
/// shortest decimal at two places, ties to even on the exact binary value) then `repr`, and `0`
/// for a zero result (`0.0` and `-0.0` are falsy).
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
        ];
        for (x, want) in cases {
            assert_eq!(format_duration(*x), *want, "{x}");
        }
    }
}
