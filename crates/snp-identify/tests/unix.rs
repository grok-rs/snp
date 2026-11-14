//! Unix-specific behaviour: permissions, symlinks, sockets, FIFOs and
//! non-UTF-8 names.
#![cfg(unix)]

use std::fs;
use std::io;
use std::path::Path;

use file_identify::{
    ExecPolicy, IdentifyError, Meta, file_is_text, parse_shebang_from_file,
    parse_shebang_from_file_with, tags_from_metadata, tags_from_path, tags_from_path_with,
};

const NONE: Vec<String> = Vec::new();

/// `tags_from_path`, checked against `tags_from_metadata` on the same
/// `lstat` result.
fn tags_of(path: &Path) -> String {
    let tags = tags_from_path(path).expect("identify");
    let md = fs::symlink_metadata(path).expect("lstat");
    let reused = tags_from_metadata(path, &md, ExecPolicy::PythonCompat).expect("identify");
    assert_eq!(reused, tags, "{}", path.display());
    tags.to_string()
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
    // Group/other x but not owner x: os.access(X_OK) is False for the owner,
    // and git stages the file as 100644.
    chmod(&path, 0o655);
    let md = fs::symlink_metadata(&path).expect("lstat");
    assert!(!Meta::from_metadata(&md, ExecPolicy::ModeBits).is_executable());
    let bits = tags_from_path_with(&path, ExecPolicy::ModeBits).expect("identify");
    assert_eq!(bits.to_string(), "file, non-executable, text");
    if !privileged(dir.path()) {
        let compat = tags_from_path_with(&path, ExecPolicy::PythonCompat).expect("identify");
        assert_eq!(compat.to_string(), "file, non-executable, text");
        assert_eq!(tags_from_path(&path).expect("identify"), compat);
    }
    // The shebang is read under the same policy as the tags.
    let shebang = |policy| parse_shebang_from_file_with(&path, policy).expect("parse");
    assert_eq!(shebang(ExecPolicy::ModeBits), NONE);
    if !privileged(dir.path()) {
        assert_eq!(shebang(ExecPolicy::PythonCompat), NONE);
        assert_eq!(parse_shebang_from_file(&path).expect("parse"), NONE);
    }
    // Owner x only: executable for both policies.
    chmod(&path, 0o700);
    for policy in [ExecPolicy::ModeBits, ExecPolicy::PythonCompat] {
        let tags = tags_from_path_with(&path, policy).expect("identify");
        assert_eq!(tags.to_string(), "executable, file, sh, shell, text");
        assert_eq!(shebang(policy), ["/bin/sh"]);
    }
    // Missing paths are reported whatever the policy.
    let missing = dir.path().join("missing");
    let err = parse_shebang_from_file_with(&missing, ExecPolicy::ModeBits).expect_err("missing");
    assert!(matches!(err, IdentifyError::PathNotFound { .. }), "{err:?}");
}

/// `ExecPolicy::ModeBits` on `lstat` metadata must agree with the mode git
/// records for the same file (`Meta::from_git_mode`), so that the `lstat`
/// path and the git index fast path classify a file the same way.
#[test]
fn mode_bits_agree_with_git() {
    use std::process::Command;
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(["-c", "core.fileMode=true", "-c", "core.autocrlf=false"])
            .args(args)
            .current_dir(d)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
    };
    match git(&["init", "-q", "."]) {
        Ok(out) if out.status.success() => {}
        _ => return, // git is not available
    }
    let modes = [
        0o644, 0o664, 0o600, 0o655, 0o645, 0o611, 0o601, 0o610, 0o700, 0o711, 0o744, 0o755, 0o777,
    ];
    for mode in modes {
        let path = d.join(format!("f{mode:o}"));
        fs::write(&path, "x\n").expect("write");
        chmod(&path, mode);
    }
    let add = git(&["add", "."]).expect("git add");
    assert!(add.status.success(), "{add:?}");
    let out = git(&["ls-files", "-s", "-z"]).expect("git ls-files");
    assert!(out.status.success(), "{out:?}");
    let mut seen = 0;
    for entry in out.stdout.split(|&b| b == 0).filter(|e| !e.is_empty()) {
        // "<mode> <sha> <stage>\t<path>"
        let entry = std::str::from_utf8(entry).expect("utf-8");
        let (info, name) = entry.split_once('\t').expect("tab");
        let git_mode =
            u32::from_str_radix(info.split(' ').next().expect("mode"), 8).expect("octal");
        let md = fs::symlink_metadata(d.join(name)).expect("lstat");
        assert_eq!(
            Meta::from_git_mode(git_mode),
            Some(Meta::from_metadata(&md, ExecPolicy::ModeBits)),
            "{name}: git mode {git_mode:o}"
        );
        seen += 1;
    }
    assert_eq!(seen, modes.len());
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

/// The metadata is trusted: a file removed after the caller's `lstat` is
/// reported like identify's re-check (`does not exist.`) when it has to be
/// opened, and not noticed when the name decides everything.
#[test]
fn metadata_of_a_removed_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("gone");
    fs::write(&path, "x").expect("write");
    let md = fs::symlink_metadata(&path).expect("lstat");
    fs::remove_file(&path).expect("remove");
    let err = tags_from_metadata(&path, &md, ExecPolicy::PythonCompat).expect_err("removed");
    assert!(matches!(err, IdentifyError::PathNotFound { .. }), "{err:?}");
    let named = dir.path().join("gone.py");
    let tags = tags_from_metadata(&named, &md, ExecPolicy::PythonCompat).expect("no I/O");
    assert_eq!(tags.to_string(), "file, non-executable, python, text");
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
