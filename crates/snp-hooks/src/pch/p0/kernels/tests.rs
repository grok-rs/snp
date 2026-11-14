//! The ✓-verified examples of 09 §4.A/§4.D/§4.E for the byte-level cores.

use super::*;

fn tw(input: &[u8], md: bool, chars: Option<&[u8]>) -> Vec<u8> {
    trailing_whitespace(input, md, chars).unwrap_or_else(|| input.to_vec())
}

#[test]
fn trailing_whitespace_default_args() {
    assert_eq!(tw(b"a \n", false, None), b"a\n");
    assert_eq!(tw(b"b\t\r\n", false, None), b"b\r\n");
    assert_eq!(tw(b"c \x0b\x0c\n", false, None), b"c\n");
    assert!(trailing_whitespace(b"e \x1c\n", false, None).is_none());
    assert!(trailing_whitespace(b"f \xc2\xa0\n", false, None).is_none());
    assert_eq!(tw(b"g\r", false, None), b"g");
    assert_eq!(tw(b"d\r\r\n", false, None), b"d\r\n");
    assert!(trailing_whitespace(b"", false, None).is_none());
    assert_eq!(tw(b"x  \n", false, None), b"x\n");
    // CR-only content is one line: only its end is stripped.
    assert_eq!(tw(b"a \rb \r", false, None), b"a \rb");
}

#[test]
fn trailing_whitespace_markdown() {
    assert!(trailing_whitespace(b"x  \n", true, None).is_none());
    assert_eq!(tw(b"y   \n", true, None), b"y  \n");
    assert_eq!(tw(b"   \n", true, None), b"\n");
    assert_eq!(tw(b"z\t  \r\n", true, None), b"z  \r\n");
}

#[test]
fn trailing_whitespace_chars_is_a_byte_set() {
    assert_eq!(
        tw("ax\u{e9}\n".as_bytes(), false, Some("x\u{e9}".as_bytes())),
        b"a\n"
    );
    assert!(trailing_whitespace(b"a \n", false, Some(b"")).is_none());
    assert_eq!(tw(b"a \t\n", false, Some(b"\t")), b"a \n");
    assert_eq!(tw(b"a \t\n", false, Some(b" \t")), b"a\n");
}

#[test]
fn trailing_whitespace_copies_unchanged_prefix() {
    assert_eq!(tw(b"ok\nok\nbad \nok", false, None), b"ok\nok\nbad\nok");
}

#[test]
fn end_of_file_truth_table() {
    let f = |b: &[u8]| end_of_file_bytes(b);
    assert_eq!(f(b""), None);
    assert_eq!(f(b"a\n"), None);
    assert_eq!(f(b"a\r\n"), None);
    assert_eq!(f(b"a\r"), None);
    assert_eq!(f(b"a"), Some(b"a\n".to_vec()));
    assert_eq!(f(b"a\r\nb"), Some(b"a\r\nb\n".to_vec()));
    assert_eq!(f(b"a\n\n\n"), Some(b"a\n".to_vec()));
    assert_eq!(f(b"a\n\r\n"), Some(b"a\n".to_vec()));
    assert_eq!(f(b"a\r\n\r\n"), Some(b"a\r\n".to_vec()));
    assert_eq!(f(b"a\r\n\n"), Some(b"a\r\n".to_vec()));
    assert_eq!(f(b"a\r\n\r"), Some(b"a\r\n".to_vec()));
    assert_eq!(f(b"a\r\r"), Some(b"a\r".to_vec()));
    assert_eq!(f(b"\n"), Some(Vec::new()));
    assert_eq!(f(b"\r\n"), Some(Vec::new()));
    assert_eq!(f(b"\r"), Some(Vec::new()));
    assert_eq!(f(b"\n\n\r\n"), Some(Vec::new()));
    assert_eq!(f(b"a\n \n"), None);
}

#[test]
fn end_of_file_tail_only() {
    // The caller passes a tail holding a non-EOL byte; the result equals the whole-file computation.
    let mut big = vec![b'x'; 10_000];
    big.extend_from_slice(b"\n\n\n");
    let tail = &big[big.len() - 10..];
    assert_eq!(
        end_of_file(tail, u64::try_from(big.len()).unwrap()),
        EofAction::Truncate(10_001)
    );
}

/// The K6 tail fold, driven as `end_of_file_fixer::tail_action` drives it but with any block size, agrees with the
/// whole-file kernel on every short file over `{a, CR, LF}` and reads at most `run + size` bytes (K6 T1/T2).
#[test]
fn tail_fold_any_block_size() {
    let scan = |c: &[u8], size: u64| {
        let len = u64::try_from(c.len()).unwrap();
        let (mut st, mut from, mut low) = (TailFold::EMPTY, len, len);
        while let Some((start, n)) = block_before(from, size) {
            let s = usize::try_from(start).unwrap();
            assert_eq!(n, usize::try_from(from - start).unwrap());
            let (next, done) = fold_block(st, &c[s..s + n]);
            (st, from, low) = (next, start, start);
            if done {
                break;
            }
        }
        (end_of_file_run(st.head(), st.run, len), low)
    };
    let mut c = Vec::new();
    for len in 0..=7_u32 {
        for code in 0..3_u32.checked_pow(len).unwrap() {
            c.clear();
            let mut k = code;
            for _ in 0..len {
                c.push(b"a\r\n"[usize::try_from(k % 3).unwrap()]);
                k /= 3;
            }
            let want = end_of_file(&c, u64::from(len));
            let run = c.iter().rev().take_while(|&&b| b != b'a').count();
            for size in 1..=9_u64 {
                let (got, low) = scan(&c, size);
                assert_eq!(got, want, "{c:?} size {size}");
                let read = c.len() - usize::try_from(low).unwrap();
                assert!(
                    read <= run + usize::try_from(size).unwrap(),
                    "{c:?} size {size}: read {read}"
                );
            }
        }
    }
    assert_eq!(block_before(0, 4096), None);
    assert_eq!(block_before(5, 0), None);
    assert_eq!(block_before(5, 4096), Some((0, 5)));
    assert_eq!(block_before(10_000, 4096), Some((10_000 - 4096, 4096)));
}

#[test]
fn mixed_line_ending_verified() {
    assert_eq!(
        mixed_line_ending(b"a\rb\rc\r\nd\n", LineFix::Auto),
        (true, Some(b"a\rb\rc\rd\r".to_vec()))
    );
    assert_eq!(
        mixed_line_ending(b"x\r\r\ny\n", LineFix::Auto),
        (true, Some(b"x\n\ny\n".to_vec()))
    );
    assert_eq!(
        mixed_line_ending(b"a\rb\nc\r\n", LineFix::Auto),
        (true, Some(b"a\nb\nc\n".to_vec()))
    );
    assert_eq!(
        mixed_line_ending(b"a\r\nb\r\nc", LineFix::Cr),
        (true, Some(b"a\rb\rc\r".to_vec()))
    );
    assert_eq!(
        mixed_line_ending(b"a\r\nb\nc", LineFix::Auto),
        (true, Some(b"a\nb\nc\n".to_vec()))
    );
    assert_eq!(mixed_line_ending(b"a\nb", LineFix::Lf), (false, None));
    assert_eq!(
        mixed_line_ending(b"a\r\nb\r\n", LineFix::Lf),
        (true, Some(b"a\nb\n".to_vec()))
    );
    assert_eq!(mixed_line_ending(b"a\r\nb\n", LineFix::No), (true, None));
    assert_eq!(mixed_line_ending(b"a\nb\n", LineFix::No), (false, None));
    assert_eq!(mixed_line_ending(b"", LineFix::Auto), (false, None));
    // `\x0b`, `\x0c`, `\x1c`, `\x85` are not line breaks for bytes.splitlines.
    assert_eq!(
        mixed_line_ending(b"a\x0bb\x0cc\n", LineFix::Auto),
        (false, None)
    );
}

#[test]
fn byte_order_marker() {
    assert_eq!(fix_byte_order_marker(b"hello"), Ok(None));
    assert_eq!(
        fix_byte_order_marker(b"\xef\xbb\xbfhello\r\n"),
        Ok(Some(&b"hello\r\n"[..]))
    );
    assert_eq!(
        fix_byte_order_marker(b"\xef\xbb\xbf\xef\xbb\xbfx"),
        Ok(Some(&b"\xef\xbb\xbfx"[..]))
    );
    assert_eq!(fix_byte_order_marker(b"\xef\xbb\xbf"), Ok(Some(&b""[..])));
    assert_eq!(
        fix_byte_order_marker(b"\xef\xbb\xbfab\xe9x").unwrap_err(),
        "'utf-8' codec can't decode byte 0xe9 in position 2: invalid continuation byte"
    );
}

#[test]
fn merge_conflict_scan() {
    let p = merge_conflict_patterns(3, false);
    let content =
        b"<<<<<<< HEAD\nx\n=======\r\n======= \n>>>>>>> b\n=======\n<<<<<<<\n||||||| base\n=======";
    let hits: Vec<(usize, &[u8])> = merge_conflicts(content, &p);
    assert_eq!(
        hits,
        vec![
            (1, &b"<<<<<<< "[..]),
            (3, b"=======\r\n"),
            (4, b"======= "),
            (5, b">>>>>>> "),
            (6, b"=======\n"),
        ]
    );
    // Epoch 1/2: no CRLF pattern.
    assert_eq!(
        merge_conflicts(b"=======\r\n", &merge_conflict_patterns(1, false)).len(),
        0
    );
    // builtin --diff3.
    assert_eq!(
        merge_conflicts(b"||||||| base\n", &merge_conflict_patterns(3, true)),
        vec![(1, &b"||||||| "[..])]
    );
}

#[test]
fn private_keys_by_epoch() {
    assert!(has_private_key(b"x\n-----BEGIN RSA PRIVATE KEY-----\n", 1));
    assert!(!has_private_key(b"BEGIN ENCRYPTED PRIVATE KEY", 1));
    assert!(has_private_key(b"BEGIN ENCRYPTED PRIVATE KEY", 2));
    assert!(has_private_key(b"#\n# BEGIN OpenVPN Static key V1", 2));
    assert!(!has_private_key(b"nothing", 2));
}

/// The three kernels as they were before the wave-3 H1 rewrite (ADR-081 bench gates), kept verbatim to tie the
/// current code to them.
mod former {
    use memchr::memchr_iter;

    use super::super::{LineFix, PRIVATE_KEY_PATTERNS};

    const fn is_ascii_ws(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    }

    fn rstrip<'a>(line: &'a [u8], chars: Option<&[u8]>) -> &'a [u8] {
        let keep = |b: &u8| match chars {
            None => !is_ascii_ws(*b),
            Some(set) => !set.contains(b),
        };
        let end = line
            .iter()
            .rposition(keep)
            .map_or(0, |i| i.saturating_add(1));
        line.get(..end).unwrap_or_default()
    }

    pub(super) fn process_line<'a>(
        line: &'a [u8],
        md: bool,
        chars: Option<&[u8]>,
        out: &mut Vec<u8>,
    ) {
        let (body, eol): (&'a [u8], &[u8]) = if let Some(b) = line.strip_suffix(b"\r\n") {
            (b, b"\r\n")
        } else if let Some(b) = line.strip_suffix(b"\n") {
            (b, b"\n")
        } else {
            (line, b"")
        };
        let is_space = !body.is_empty() && body.iter().all(|&b| is_ascii_ws(b));
        if md && !is_space && body.ends_with(b"  ") {
            let head = body.get(..body.len().saturating_sub(2)).unwrap_or_default();
            out.extend_from_slice(rstrip(head, chars));
            out.extend_from_slice(b"  ");
        } else {
            out.extend_from_slice(rstrip(body, chars));
        }
        out.extend_from_slice(eol);
    }

    pub(super) fn trailing_whitespace(
        content: &[u8],
        md: bool,
        chars: Option<&[u8]>,
    ) -> Option<Vec<u8>> {
        let mut out: Option<Vec<u8>> = None;
        let mut start = 0_usize;
        let mut scratch = Vec::new();
        let mut each = |line: &[u8], line_start: usize, out: &mut Option<Vec<u8>>| {
            scratch.clear();
            process_line(line, md, chars, &mut scratch);
            match out {
                Some(o) => o.extend_from_slice(&scratch),
                None if scratch.as_slice() != line => {
                    let mut o = Vec::new();
                    o.extend_from_slice(content.get(..line_start).unwrap_or_default());
                    o.extend_from_slice(&scratch);
                    *out = Some(o);
                }
                None => {}
            }
        };
        for nl in memchr_iter(b'\n', content) {
            let end = nl.saturating_add(1);
            let line = content.get(start..end).unwrap_or_default();
            each(line, start, &mut out);
            start = end;
        }
        if start < content.len() {
            let line = content.get(start..).unwrap_or_default();
            each(line, start, &mut out);
        }
        out
    }

    fn splitlines_keepends(content: &[u8]) -> impl Iterator<Item = &[u8]> {
        let mut rest = content;
        std::iter::from_fn(move || {
            if rest.is_empty() {
                return None;
            }
            let cut = match memchr::memchr2(b'\r', b'\n', rest) {
                None => rest.len(),
                Some(i) => {
                    let crlf = rest.get(i) == Some(&b'\r')
                        && rest.get(i.saturating_add(1)) == Some(&b'\n');
                    i.saturating_add(if crlf { 2 } else { 1 })
                }
            };
            let (line, tail) = rest.split_at_checked(cut).unwrap_or((rest, &[]));
            rest = tail;
            Some(line)
        })
    }

    pub(super) fn mixed_line_ending(content: &[u8], fix: LineFix) -> (bool, Option<Vec<u8>>) {
        let (mut cr, mut crlf, mut lf) = (0_usize, 0_usize, 0_usize);
        for line in splitlines_keepends(content) {
            if line.ends_with(b"\r") {
                cr = cr.saturating_add(1);
            } else if line.ends_with(b"\r\n") {
                crlf = crlf.saturating_add(1);
            } else if line.ends_with(b"\n") {
                lf = lf.saturating_add(1);
            }
        }
        let kinds = [cr, crlf, lf].iter().filter(|&&n| n > 0).count();
        let mixed = kinds > 1;
        let ending: &[u8] = match fix {
            LineFix::No => return (mixed, None),
            LineFix::Auto if !mixed => return (false, None),
            LineFix::Auto => {
                let mut best: (&[u8], usize) = (b"\n", 0);
                for (e, n) in [(&b"\r"[..], cr), (b"\r\n", crlf), (b"\n", lf)] {
                    if n >= best.1 {
                        best = (e, n);
                    }
                }
                best.0
            }
            LineFix::Cr | LineFix::Crlf | LineFix::Lf => {
                let (target, others): (&[u8], usize) = match fix {
                    LineFix::Cr => (b"\r", crlf.saturating_add(lf)),
                    LineFix::Crlf => (b"\r\n", cr.saturating_add(lf)),
                    _ => (b"\n", cr.saturating_add(crlf)),
                };
                if others == 0 {
                    return (false, None);
                }
                target
            }
        };
        let mut out = Vec::new();
        for line in splitlines_keepends(content) {
            let end = line
                .iter()
                .rposition(|&b| b != b'\r' && b != b'\n')
                .map_or(0, |i| i.saturating_add(1));
            out.extend_from_slice(line.get(..end).unwrap_or_default());
            out.extend_from_slice(ending);
        }
        (true, Some(out))
    }

    pub(super) fn has_private_key(content: &[u8], epoch: u16) -> bool {
        let n = if epoch >= 2 { 10 } else { 8 };
        PRIVATE_KEY_PATTERNS
            .iter()
            .take(n)
            .any(|p| memchr::memmem::find(content, p).is_some())
    }
}

/// Every byte string over `alphabet` of length at most `max`, shortest first.
fn words(alphabet: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut all = vec![Vec::new()];
    let mut last = vec![Vec::new()];
    for _ in 0..max {
        let mut next = Vec::new();
        for w in &last {
            for &b in alphabet {
                let mut v = w.clone();
                v.push(b);
                next.push(v);
            }
        }
        all.extend(next.iter().cloned());
        last = next;
    }
    all
}

const EPOCHS: [u16; 5] = [0, 1, 2, 3, u16::MAX];

/// The text the H1 fast path scans for first: every pattern holds it exactly once, except `PuTTY-User-Key-File-2`
/// (index 5) and `BEGIN OpenVPN Static key V1` (index 9), which hold it not at all.
#[test]
fn private_key_patterns_hold_the_anchor_once_or_are_the_two_exceptions() {
    for (i, p) in PRIVATE_KEY_PATTERNS.iter().enumerate() {
        let hits = memchr::memmem::find_iter(p, b"PRIVATE KEY").count();
        if i == 5 || i == 9 {
            assert_eq!(hits, 0, "pattern {i}");
        } else {
            assert_eq!(hits, 1, "pattern {i}");
        }
    }
}

/// detect-private-key agrees with the former 8/10-pattern scan on every pair of fragments (each pattern whole,
/// truncated at the end or the start, the bare anchor and its halves, the `SSH2 ENCRYPTED` decoy, noise) at offsets
/// 0 and 1, for epochs 0, 1, 2, 3 and `u16::MAX`; plus repeated anchors. Both branches (anchor found or not) see
/// both answers.
#[test]
fn private_key_matches_former_code() {
    let mut frags: Vec<Vec<u8>> = Vec::new();
    for p in PRIVATE_KEY_PATTERNS {
        frags.push(p.to_vec());
        frags.push(p[..p.len() - 1].to_vec());
        frags.push(p[1..].to_vec());
    }
    for f in [
        &b"PRIVATE KEY"[..],
        b"PRIVATE KE",
        b"RIVATE KEY",
        b"SSH2 ENCRYPTED PRIVATE KEY",
        b"BEGIN SSH2 ENCRYPTED",
        b"BEGIN ",
        b"x",
        b"\n",
        b"",
    ] {
        frags.push(f.to_vec());
    }
    let mut seen = [[0_usize; 2]; 2];
    let mut check = |c: &[u8]| {
        for epoch in EPOCHS {
            let got = has_private_key(c, epoch);
            assert_eq!(
                got,
                former::has_private_key(c, epoch),
                "{:?} epoch {epoch}",
                String::from_utf8_lossy(c)
            );
            let anchored = memchr::memmem::find(c, b"PRIVATE KEY").is_some();
            seen[usize::from(anchored)][usize::from(got)] += 1;
        }
    };
    for a in &frags {
        for b in &frags {
            for lead in [&b""[..], b"x"] {
                check(&[lead, a, b].concat());
            }
        }
    }
    for p in PRIVATE_KEY_PATTERNS {
        for k in 1..4 {
            let anchors = vec![&b"PRIVATE KEY"[..]; k].concat();
            check(&[anchors.as_slice(), p].concat());
            check(&[p, anchors.as_slice()].concat());
        }
    }
    check(b"PRIVATE KEYPRIVATE KEY BEGIN RSA PRIVATE KEY");
    assert!(
        seen.iter().flatten().all(|&n| n > 0),
        "branch coverage {seen:?}"
    );
}

const TW_ALPHABET: &[u8] = b"ax \t\r\n\x0b";
const TW_CHARS: [Option<&[u8]>; 5] = [None, Some(b""), Some(b" "), Some(b"x"), Some(b"a\r")];

/// trailing-whitespace agrees with the former line-copying code on every string of at most 5 bytes over
/// `{a, x, ' ', TAB, CR, LF, VT}`, markdown on and off, `--chars` unset, empty, `" "`, `"x"` and `"a\r"`; a fix is
/// strictly shorter than the content (it only deletes bytes, which the output reservation relies on). This holds
/// the case the rejected `line_unchanged` predicate got wrong: `ax  \n` with markdown and `--chars x`.
#[test]
fn trailing_whitespace_matches_former_code_exhaustively() {
    let mut changed = [0_usize; 2];
    for c in words(TW_ALPHABET, 5) {
        for md in [false, true] {
            for chars in TW_CHARS {
                let got = trailing_whitespace(&c, md, chars);
                assert_eq!(
                    got,
                    former::trailing_whitespace(&c, md, chars),
                    "{c:?} md {md} chars {chars:?}"
                );
                if let Some(new) = &got {
                    assert!(new.len() < c.len(), "{c:?} grew");
                }
                changed[usize::from(got.is_some())] += 1;
            }
        }
    }
    assert!(changed.iter().all(|&n| n > 0));
    assert_eq!(
        trailing_whitespace(b"ax  \n", true, Some(b"x")).as_deref(),
        Some(&b"a  \n"[..])
    );
    assert_eq!(
        trailing_whitespace(b"ok\nax  \nok", true, Some(b"x")).as_deref(),
        Some(&b"ok\na  \nok"[..])
    );
}

/// Unchanged long lines before and after a change, and a multibyte `--chars` set, against the former code.
#[test]
fn trailing_whitespace_matches_former_code_on_long_content() {
    let mut c = vec![&b"line one\r\n"[..]; 50].concat();
    c.extend_from_slice(b"md hard break  \n   \n\t\x0b\x0c\n");
    c.extend_from_slice("tail \u{e9}\u{e9}".as_bytes());
    for md in [false, true] {
        for chars in [
            None,
            Some(&b" \t"[..]),
            Some("\u{e9}".as_bytes()),
            Some(b"\r"),
        ] {
            assert_eq!(
                trailing_whitespace(&c, md, chars),
                former::trailing_whitespace(&c, md, chars),
                "md {md} chars {chars:?}"
            );
        }
    }
}

const FIXES: [LineFix; 5] = [
    LineFix::Auto,
    LineFix::No,
    LineFix::Cr,
    LineFix::Crlf,
    LineFix::Lf,
];

/// mixed-line-ending agrees with the former code for every `--fix` on every string of at most 8 bytes over
/// `{a, CR, LF}` and of at most 6 over `{a, LF, VT, ' '}` (CR-free); both branches (CR present or not) both keep and
/// rewrite.
#[test]
fn mixed_line_ending_matches_former_code_exhaustively() {
    let mut seen = [[0_usize; 2]; 2];
    let inputs = words(b"a\r\n", 8).into_iter().chain(words(b"a\n\x0b ", 6));
    for c in inputs {
        for fix in FIXES {
            let got = mixed_line_ending(&c, fix);
            assert_eq!(got, former::mixed_line_ending(&c, fix), "{c:?} {fix:?}");
            let has_cr = c.contains(&b'\r');
            seen[usize::from(has_cr)][usize::from(got.1.is_some())] += 1;
        }
    }
    assert!(
        seen.iter().flatten().all(|&n| n > 0),
        "branch coverage {seen:?}"
    );
}

/// The named shapes of the H1 plan for every `--fix`: empty input, no newline, LF only, an unterminated last line
/// (each with and without a CR elsewhere).
#[test]
fn mixed_line_ending_named_shapes_match_former_code() {
    for c in [
        &b""[..],
        b"abc",
        b"a\nb\n",
        b"a\nb",
        b"\n",
        b"\n\n",
        b"a\r",
        b"a\rb",
        b"a\r\nb",
        b"a\nb\r\nc",
        b"a\r\r\n",
    ] {
        for fix in FIXES {
            assert_eq!(
                mixed_line_ending(c, fix),
                former::mixed_line_ending(c, fix),
                "{c:?} {fix:?}"
            );
        }
    }
    assert_eq!(
        mixed_line_ending(b"a\nb", LineFix::Cr),
        (true, Some(b"a\rb\r".to_vec()))
    );
    assert_eq!(
        mixed_line_ending(b"a\nb", LineFix::Crlf),
        (true, Some(b"a\r\nb\r\n".to_vec()))
    );
    assert_eq!(mixed_line_ending(b"abc", LineFix::Crlf), (false, None));
    assert_eq!(mixed_line_ending(b"abc", LineFix::Cr), (false, None));
}

#[test]
fn kilobytes() {
    assert_eq!(size_kb(0), 0);
    assert_eq!(size_kb(1024), 1);
    assert_eq!(size_kb(1025), 2);
    assert_eq!(size_kb(u64::MAX), u64::MAX / 1024 + 1);
}

#[test]
fn extensions() {
    assert_eq!(splitext_ext(b"foo."), b".");
    assert_eq!(splitext_ext(b".md"), b"");
    assert_eq!(splitext_ext(b"...md"), b"");
    assert_eq!(splitext_ext(b"a..md"), b".md");
    assert_eq!(splitext_ext(b"dir.d/file"), b"");
}
