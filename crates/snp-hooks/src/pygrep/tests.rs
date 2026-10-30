//! pygrep's line splitting and the compiled-pattern bookkeeping (04a §8.4, §12.4).

use super::*;

#[test]
fn lines_split_after_newline() {
    let v: Vec<&[u8]> = lines(b"a\nb\r\n\nc").collect();
    assert_eq!(v, vec![&b"a\n"[..], b"b\r\n", b"\n", b"c"]);
    assert_eq!(lines(b"").count(), 0);
    assert_eq!(lines(b"x\n").count(), 1);
}

#[test]
fn prepare_reports_once() {
    let c = prepare(&[Arc::from("-i")], "fo+", 80).unwrap();
    let dbg = format!("{c:?}");
    assert!(
        dbg.contains("fo+") && dbg.contains("ignore_case: true"),
        "{dbg}"
    );
    let (code, out) = prepare(&[], "(", 80).unwrap_err();
    assert_eq!(code, 1);
    assert_eq!(
        out,
        b"re.PatternError: missing ), unterminated subpattern at position 0\n"
    );
    let (code, out) = prepare(&[], "a{4294967295}", 80).unwrap_err();
    assert_eq!(code, 1);
    assert_eq!(out, b"OverflowError: the repetition number is too large\n");
    let (code, out) = prepare(&[Arc::from("--bogus")], "x", 80).unwrap_err();
    assert_eq!(code, 2);
    assert!(out.ends_with(b"pygrep.py: error: unrecognized arguments: --bogus\n"));
    let (code, _) = prepare(&[Arc::from("-h")], "x", 80).unwrap_err();
    assert_eq!(code, 0);
}
