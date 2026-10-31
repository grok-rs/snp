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
