//! `base64.decodebytes` (CPython 3.12 `binascii.a2b_base64`, non-strict) for `!!binary`, and
//! `base64.encodebytes` for dumping `bytes`.
//!
//! Owner: WP-14.

use crate::messages::{INCORRECT_PADDING, base64_one_extra};

/// The standard alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `table_a2b_base64`: the 6-bit value of an alphabet byte.
fn sextet(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b.wrapping_sub(b'A')),
        b'a'..=b'z' => Some(b.wrapping_sub(b'a').wrapping_add(26)),
        b'0'..=b'9' => Some(b.wrapping_sub(b'0').wrapping_add(52)),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// `binascii.a2b_base64(data, strict_mode=False)`: characters outside the alphabet are skipped, a pad sequence
/// that completes a quad ends the data, and a dangling quad is `Incorrect padding` (or, with exactly one data
/// character, the "1 more than a multiple of 4" message). The error is the `binascii.Error` text.
pub(crate) fn decode(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut quad_pos = 0u8;
    let mut left: u8 = 0;
    let mut pads = 0u8;
    for &ch in data {
        if ch == b'=' {
            if quad_pos >= 2 {
                pads = pads.saturating_add(1);
                if quad_pos.saturating_add(pads) >= 4 {
                    return Ok(out);
                }
            }
            continue;
        }
        let Some(v) = sextet(ch) else {
            continue;
        };
        pads = 0;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = v;
            }
            1 => {
                quad_pos = 2;
                out.push(left.wrapping_shl(2) | v.wrapping_shr(4));
                left = v & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push(left.wrapping_shl(4) | v.wrapping_shr(2));
                left = v & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push(left.wrapping_shl(6) | v);
                left = 0;
            }
        }
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(base64_one_extra(
            out.len()
                .wrapping_div(3)
                .saturating_mul(4)
                .saturating_add(1),
        )),
        _ => Err(INCORRECT_PADDING.to_owned()),
    }
}

/// `base64.encodebytes(data).decode('ascii')`: 57-byte chunks, each encoded to 76 characters and a `\n`.
pub(crate) fn encodebytes(data: &[u8]) -> String {
    let mut out = String::new();
    let mut rest = data;
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at_checked(57).unwrap_or((rest, &[]));
        encode_into(chunk, &mut out);
        out.push('\n');
        rest = tail;
    }
    out
}

/// Standard base64 with `=` padding.
fn encode_into(data: &[u8], out: &mut String) {
    let sym = |i: u8| char::from(ALPHABET.get(usize::from(i & 0x3f)).copied().unwrap_or(b'A'));
    let mut rest = data;
    loop {
        match rest {
            [a, b, c, tail @ ..] => {
                out.push(sym(a.wrapping_shr(2)));
                out.push(sym(a.wrapping_shl(4) | b.wrapping_shr(4)));
                out.push(sym(b.wrapping_shl(2) | c.wrapping_shr(6)));
                out.push(sym(*c));
                rest = tail;
            }
            [a, b] => {
                out.push(sym(a.wrapping_shr(2)));
                out.push(sym(a.wrapping_shl(4) | b.wrapping_shr(4)));
                out.push(sym(b.wrapping_shl(2)));
                out.push('=');
                return;
            }
            [a] => {
                out.push(sym(a.wrapping_shr(2)));
                out.push(sym(a.wrapping_shl(4)));
                out.push_str("==");
                return;
            }
            [] => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_matches_cpython() {
        assert_eq!(decode(b"aGk="), Ok(b"hi".to_vec()));
        assert_eq!(decode(b"aGk=x"), Ok(b"hi".to_vec()));
        assert_eq!(decode(b"a G\nk ="), Ok(b"hi".to_vec()));
        assert_eq!(decode(b"aGk"), Err("Incorrect padding".into()));
        assert_eq!(
            decode(b"a"),
            Err(
                "Invalid base64-encoded string: number of data characters (1) cannot be 1 more \
                 than a multiple of 4"
                    .into()
            )
        );
        assert_eq!(decode(b""), Ok(Vec::new()));
        assert_eq!(decode(b"===="), Ok(Vec::new()));
    }

    #[test]
    fn encodebytes_wraps_at_76() {
        assert_eq!(encodebytes(b""), "");
        assert_eq!(encodebytes(b"hi"), "aGk=\n");
        let long = encodebytes(&[b'x'; 100]);
        let lines: Vec<&str> = long.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines.first().map(|l| l.len()), Some(76));
        assert!(long.ends_with("eA==\n"));
    }
}
