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
    // An argparse error of `args + [entry]` is the partitions' to report (their files are part of the parse).
    let c = prepare(&[Arc::from("--bogus")], "x", 80).unwrap();
    assert!(c.pre.is_none());
    let o = run_partition(
        &c,
        &[OsStr::new("--bogus"), OsStr::new("x"), OsStr::new("f")],
        Path::new("."),
        &RegexCache::new(),
        &CancelToken::new(),
    );
    assert_eq!(o.code, 2);
    assert!(o.output.ends_with(if cfg!(windows) {
        b"pygrep.py: error: unrecognized arguments: --bogus\r\n"
    } else {
        b"pygrep.py: error: unrecognized arguments: --bogus\n"
    }));
    let (code, _) = prepare(&[Arc::from("-h")], "x", 80).unwrap_err();
    assert_eq!(code, 0);
}

/// An entry shaped like a flag is not the pattern of `args + [entry]`: deferred to the partitions' full parse.
#[test]
fn flag_shaped_entry_is_deferred() {
    for entry in ["-i", "--negate", "-foo"] {
        let c = prepare(&[], entry, 80).unwrap();
        assert!(c.pre.is_none(), "{entry}");
    }
    assert!(
        prepare(&[Arc::from("--negate")], "x", 80)
            .unwrap()
            .pre
            .is_some()
    );
}

/// D-SEC-10: a backtracking-limit hit is the register's line (pattern and path reprs), exit 1.
#[test]
fn backtrack_limit_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut line = vec![b'a'; 40];
    line.extend_from_slice(b"b\n");
    std::fs::write(dir.path().join("f'1"), &line).unwrap();
    let pattern = "(?:(?=a)a+)+(?<!a)b";
    let c = prepare(&[], pattern, 80).unwrap();
    let o = run_partition(
        &c,
        &[OsStr::new(pattern), OsStr::new("f'1")],
        dir.path(),
        &RegexCache::new(),
        &CancelToken::new(),
    );
    assert_eq!(o.code, 1);
    assert!(o.crashed);
    assert_eq!(
        String::from_utf8_lossy(&o.output),
        "snp: regular expression '(?:(?=a)a+)+(?<!a)b' exceeded the backtracking limit on \"f'1\"\n"
    );
}

/// `FutureWarning`s of the pattern start the partition's output.
#[test]
fn future_warnings_precede_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"x]\n").unwrap();
    let c = prepare(&[], "[[x]]", 80).unwrap();
    let o = run_partition(
        &c,
        &[OsStr::new("[[x]]"), OsStr::new("a")],
        dir.path(),
        &RegexCache::new(),
        &CancelToken::new(),
    );
    assert_eq!(
        String::from_utf8_lossy(&o.output),
        "[WARNING] regex '[[x]]': Possible nested set at position 1 (Python FutureWarning)\na:1:x]\n"
    );
}
