//! Fuzz target `pch_p0_mixed_line_ending`: the native `mixed-line-ending` core on arbitrary bytes.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! Every `--fix` mode runs and answers what a transliteration of upstream's `fix_filename` does (the kernel's
//! CR-free short-circuit against the former counting pass); a rewrite leaves a single ending kind, so fixing it again
//! reports nothing.
//!
//! Owner: WP-26.
#![no_main]

use libfuzzer_sys::fuzz_target;
use snp_hooks::pch::p0::kernels::{self, LineFix};

/// `bytes.splitlines(keepends=True)`: a line ends after `\n`, after `\r\n`, and after a `\r` not followed by `\n`.
fn splitlines(c: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < c.len() {
        let end = match c[i] {
            b'\n' => i + 1,
            b'\r' if c.get(i + 1) == Some(&b'\n') => i + 2,
            b'\r' => i + 1,
            _ => {
                i += 1;
                continue;
            }
        };
        lines.push(&c[start..end]);
        (start, i) = (end, end);
    }
    if start < c.len() {
        lines.push(&c[start..]);
    }
    lines
}

/// `mixed_line_ending.fix_filename` (`:24-62`) and `_fix` (`:16-19`): `(return value, the rewrite if any)`.
fn oracle(c: &[u8], fix: LineFix) -> (bool, Option<Vec<u8>>) {
    let lines = splitlines(c);
    let (mut cr, mut crlf, mut lf) = (0_usize, 0_usize, 0_usize);
    for line in &lines {
        if line.ends_with(b"\r") {
            cr += 1;
        } else if line.ends_with(b"\r\n") {
            crlf += 1;
        } else if line.ends_with(b"\n") {
            lf += 1;
        }
    }
    let mixed = [cr, crlf, lf].iter().filter(|&&n| n > 0).count() > 1;
    let explicit = |target: &'static [u8], others: usize| (others > 0).then_some(target);
    let ending = match fix {
        LineFix::No => return (mixed, None),
        LineFix::Auto if !mixed => return (false, None),
        LineFix::Auto => {
            let mut best: (&[u8], usize) = (b"\n", 0);
            for (e, n) in [(&b"\r"[..], cr), (b"\r\n", crlf), (b"\n", lf)] {
                if n >= best.1 {
                    best = (e, n);
                }
            }
            Some(best.0)
        }
        LineFix::Cr => explicit(b"\r", crlf + lf),
        LineFix::Crlf => explicit(b"\r\n", cr + lf),
        LineFix::Lf => explicit(b"\n", cr + crlf),
    };
    let Some(ending) = ending else {
        return (false, None);
    };
    let mut out = Vec::new();
    for line in lines {
        let mut body = line;
        while let Some(rest) = body
            .strip_suffix(b"\r")
            .or_else(|| body.strip_suffix(b"\n"))
        {
            body = rest;
        }
        out.extend_from_slice(body);
        out.extend_from_slice(ending);
    }
    (true, Some(out))
}

fuzz_target!(|data: &[u8]| {
    for fix in [
        LineFix::Auto,
        LineFix::No,
        LineFix::Cr,
        LineFix::Crlf,
        LineFix::Lf,
    ] {
        let got = kernels::mixed_line_ending(data, fix);
        assert_eq!(got, oracle(data, fix), "{fix:?}");
        if let (ret, Some(new)) = got {
            assert!(ret);
            assert_eq!(kernels::mixed_line_ending(&new, LineFix::No), (false, None));
        }
    }
});
