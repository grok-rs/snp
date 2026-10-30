//! `_cmd_filenames` and the exit-status folding (03 Addendum §H.4, §H.7.3).

use super::*;

fn os(xs: &[&str]) -> Vec<OsString> {
    xs.iter().map(OsString::from).collect()
}

#[test]
fn cmd_filenames_splits_on_the_last_separator() {
    assert!(matches!(
        cmd_filenames(&os(&["hello", "world"])),
        Err(Stop::SystemExit(m)) if m == "hazmat entry must end with `--`"
    ));
    let cmd = os(&["hello", "world", "--"]);
    let (head, files) = cmd_filenames(&cmd).ok().unwrap();
    assert_eq!(head, os(&["hello", "world"]).as_slice());
    assert_eq!(files.len(), 0);
    let cmd = os(&["hello", "--", "arg", "--", "f1", "f2"]);
    let (head, files) = cmd_filenames(&cmd).ok().unwrap();
    assert_eq!(head, os(&["hello", "--", "arg"]).as_slice());
    assert_eq!(files, os(&["f1", "f2"]).as_slice());
    // A file named `--` becomes the split point (parity).
    let cmd = os(&["echo", "--", "a", "--"]);
    let (head, files) = cmd_filenames(&cmd).ok().unwrap();
    assert_eq!(head, os(&["echo", "--", "a"]).as_slice());
    assert_eq!(files.len(), 0);
}

#[cfg(unix)]
#[test]
fn status_folding() {
    assert_eq!(exit_status(0), 0);
    assert_eq!(exit_status(-9), 247);
    assert_eq!(exit_status(-15), 241);
    assert_eq!(exit_status(-9 | 8), 255);
    assert_eq!(exit_status(1 | 2), 3);
    assert_eq!(exit_status(256), 0);
}

#[test]
fn empty_command_is_index_error() {
    let env = Env::from_pairs(Vec::<(OsString, OsString)>::new());
    match normalize(&[], &env, Path::new(".")) {
        Err(Stop::Exc(e)) => {
            assert_eq!(e.ty, "IndexError");
            assert_eq!(e.text, b"tuple index out of range");
        }
        other => panic!("{other:?}"),
    }
}
