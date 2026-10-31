//! `PyOut` against the flush points measured on CPython 3.13.14 with stdout and stderr on one pipe (Linux).

use super::*;

/// Index of the stderr marker `E` in the merged output of `ops` followed by `sys.stderr.write('E\n')`.
fn marker(ops: &[(&str, usize)]) -> usize {
    let mut o = PyOut::default();
    for &(kind, n) in ops {
        if kind == "text" {
            o.write_text(&vec![b'x'; n]);
        } else {
            o.write_bytes(&vec![b'y'; n]);
        }
    }
    o.eprint_bytes(b"E\n");
    let out = o.finish();
    out.iter().position(|&b| b == b'E').unwrap()
}

#[cfg(not(windows))]
#[test]
fn text_layer_flushes_at_8192() {
    assert_eq!(marker(&[("text", 100)]), 0);
    assert_eq!(marker(&[("text", 8191)]), 0);
    assert_eq!(marker(&[("text", 8192)]), 8192);
    assert_eq!(marker(&[("text", 20000)]), 20000);
    assert_eq!(marker(&[("text", 5000), ("text", 4000)]), 9000);
    assert_eq!(marker(&[("text", 5000), ("text", 3193)]), 8193);
    assert_eq!(marker(&[("text", 100), ("text", 8100)]), 8200);
}

#[cfg(not(windows))]
#[test]
fn buffer_layer_matches_cpython_313() {
    assert_eq!(marker(&[("buf", 4095)]), 0);
    assert_eq!(marker(&[("buf", 4096)]), 4096);
    assert_eq!(marker(&[("buf", 100), ("buf", 4096)]), 4196);
    assert_eq!(marker(&[("buf", 100), ("buf", 3996)]), 0);
    assert_eq!(marker(&[("buf", 100), ("buf", 3997)]), 100);
    assert_eq!(marker(&[("buf", 4095), ("buf", 1)]), 0);
    assert_eq!(marker(&[("buf", 4095), ("buf", 2)]), 4095);
    assert_eq!(marker(&[("buf", 3000), ("buf", 1096), ("buf", 1)]), 4096);
    assert_eq!(marker(&[("buf", 3000), ("buf", 6000)]), 9000);
    assert_eq!(marker(&[("buf", 3000), ("text", 8192)]), 11192);
}

#[test]
fn buffer_writes_overtake_pending_text() {
    // print('a'); sys.stdout.buffer.write(b'B\n'); print('c', file=sys.stderr) → `c`, `B`, `a`.
    let mut o = PyOut::default();
    o.print("a");
    o.write_bytes(b"B\n");
    o.eprint("c");
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    assert_eq!(o.finish(), format!("c{nl}B\na{nl}").into_bytes());
}

#[test]
fn traceback_flushes_stdout_first() {
    let mut o = PyOut::default();
    o.print("Fixing a");
    o.traceback(&PyExc::new(
        "FileNotFoundError",
        "[Errno 2] No such file or directory: 'zz'",
    ));
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    assert_eq!(
        o.finish(),
        format!("Fixing a{nl}FileNotFoundError: [Errno 2] No such file or directory: 'zz'{nl}")
            .into_bytes()
    );
}

#[test]
fn crash_lines() {
    assert_eq!(
        crash_line(&PyExc::new(
            "ExecutableNotFoundError",
            "Executable `x` not found"
        )),
        b"pre_commit.parse_shebang.ExecutableNotFoundError: Executable `x` not found\n"
    );
    assert_eq!(
        crash_line(&PyExc::new("KeyboardInterrupt", "")),
        b"KeyboardInterrupt\n"
    );
    assert_eq!(
        crash_line(&PyExc::new("re.error", "x at position 0")),
        b"re.error: x at position 0\n"
    );
}
