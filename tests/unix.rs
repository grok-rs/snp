//! Unix-specific behaviour: permissions, symlinks, sockets, FIFOs and
//! non-UTF-8 names.
#![cfg(unix)]

use std::fs;
use std::io;
use std::path::Path;

use file_identify::{
    ExecPolicy, IdentifyError, Meta, file_is_text, parse_shebang_from_file, tags_from_path,
    tags_from_path_with,
};

const NONE: Vec<String> = Vec::new();

fn tags_of(path: &Path) -> String {
    tags_from_path(path).expect("identify").to_string()
}

fn chmod(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn executables_and_shebangs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    for (name, content, expected) in [
        (
            "a",
            "#!/usr/bin/env python3\n",
            "executable, file, python, python3, text",
        ),
        (
            "b",
            "#!/bin/bash\necho\n",
            "bash, executable, file, shell, text",
        ),
        ("c", "#!   \n", "executable, file, text"),
        (
            "d",
            "#!\"/opt/my tools/ruby\" -w\n",
            "executable, file, ruby, text",
        ),
        (
            "e",
            "#! /usr/bin/env nix-shell\n#! nix-shell -i zsh -p jq\n",
            "executable, file, shell, text, zsh",
        ),
        // Name tags win: the shebang is not consulted.
        (
            "f.rb",
            "#!/usr/bin/env python3\n",
            "executable, file, ruby, text",
        ),
    ] {
        let path = d.join(name);
        fs::write(&path, content).expect("write");
        chmod(&path, 0o755);
        assert_eq!(tags_of(&path), expected, "{name}");
    }
    let path = d.join("not-exec");
    fs::write(&path, "#!/usr/bin/env python\n").expect("write");
    assert_eq!(tags_of(&path), "file, non-executable, text");
    assert_eq!(parse_shebang_from_file(&path).expect("parse"), NONE);
    chmod(&path, 0o755);
    assert_eq!(parse_shebang_from_file(&path).expect("parse"), ["python"]);
}

/// Whether this process bypasses permission checks (e.g. runs as root).
fn privileged(dir: &Path) -> bool {
    let probe = dir.join(".probe");
    fs::write(&probe, "x").expect("write");
    chmod(&probe, 0o000);
    let open = fs::File::open(&probe).is_ok();
    fs::remove_file(&probe).expect("remove");
    open
}

#[test]
fn exec_policies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tool");
    fs::write(&path, "#!/bin/sh\n").expect("write");
    // Group/other x but not owner x: os.access(X_OK) is False for the owner.
    chmod(&path, 0o655);
    let md = fs::symlink_metadata(&path).expect("lstat");
    assert!(Meta::from_metadata(&md, ExecPolicy::ModeBits).is_executable());
    let bits = tags_from_path_with(&path, ExecPolicy::ModeBits).expect("identify");
    assert_eq!(bits.to_string(), "executable, file, sh, shell, text");
    if !privileged(dir.path()) {
        let compat = tags_from_path_with(&path, ExecPolicy::PythonCompat).expect("identify");
        assert_eq!(compat.to_string(), "file, non-executable, text");
        assert_eq!(tags_from_path(&path).expect("identify"), compat);
    }
}

#[test]
fn symlinks_sockets_fifos() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    fs::write(d.join("target.py"), "x").expect("write");
    std::os::unix::fs::symlink("target.py", d.join("link.py")).expect("symlink");
    std::os::unix::fs::symlink("nowhere", d.join("dangling")).expect("symlink");
    assert_eq!(tags_of(&d.join("link.py")), "symlink");
    assert_eq!(tags_of(&d.join("dangling")), "symlink");
    assert!(file_is_text(d.join("link.py")).expect("follows symlink"));
    assert!(matches!(
        file_is_text(d.join("dangling")),
        Err(IdentifyError::IoError { .. })
    ));
    assert_eq!(
        parse_shebang_from_file(d.join("dangling")).expect("dangling"),
        NONE
    );

    // Unix socket (short path: sun_path is limited).
    let sock_dir = tempfile::Builder::new()
        .prefix("fi")
        .tempdir()
        .expect("tempdir");
    let sock = sock_dir.path().join("s");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
    assert_eq!(tags_of(&sock), "socket");

    // A FIFO without a writer would block identify forever; we never open it.
    let fifo = d.join("pipe");
    let status = std::process::Command::new("mkfifo").arg(&fifo).status();
    if status.is_ok_and(|s| s.success()) {
        assert_eq!(tags_of(&fifo), "binary, file, non-executable");
        chmod(&fifo, 0o755);
        assert_eq!(tags_of(&fifo), "binary, executable, file");
        assert!(!file_is_text(&fifo).expect("never opened"));
        assert_eq!(parse_shebang_from_file(&fifo).expect("never opened"), NONE);
        let fifo_py = d.join("pipe.py");
        let _ = std::process::Command::new("mkfifo").arg(&fifo_py).status();
        assert_eq!(tags_of(&fifo_py), "file, non-executable, python, text");
    }
}

#[test]
fn unreadable_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secret");
    fs::write(&path, "x").expect("write");
    chmod(&path, 0o000);
    if privileged(dir.path()) {
        return;
    }
    // Content needed: pre-commit crashes with PermissionError; we report it.
    match tags_from_path(&path) {
        Err(IdentifyError::IoError { source }) => {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
        }
        other => panic!("unexpected {other:?}"),
    }
    // Name decides everything: the file is never opened.
    let named = dir.path().join("secret.txt");
    fs::write(&named, "x").expect("write");
    chmod(&named, 0o000);
    assert_eq!(tags_of(&named), "file, non-executable, plain-text, text");
}

#[test]
fn non_utf8_file_names() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(std::ffi::OsStr::from_bytes(b"caf\xe9.py"));
    if fs::write(&path, "print()\n").is_err() {
        return; // file system rejects non-UTF-8 names
    }
    assert_eq!(tags_of(&path), "file, non-executable, python, text");
}
