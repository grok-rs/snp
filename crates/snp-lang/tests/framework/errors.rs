//! `LangError` → the failure envelope: Python type, text and exit code (1 Fatal, 130 Interrupted, else 3).

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use snp_core::failure::Failure;
use snp_lang::LangError;
use snp_net::NetError;

/// (type, text, exit code) of the envelope.
fn envelope(e: LangError) -> (String, Vec<u8>, i32) {
    let f = Failure::from(e);
    let code = f.exit_code();
    match f {
        Failure::Exc { exc, .. } => (exc.ty.into_owned(), exc.text, code),
        Failure::Interrupted(_) => ("<interrupted>".to_owned(), Vec::new(), code),
        other => panic!("{other:?}"),
    }
}

#[test]
fn python_types_texts_and_exit_codes() {
    assert_eq!(
        envelope(LangError::Fatal(
            "Expected .cabal files or additional_dependencies".into()
        )),
        (
            "FatalError".into(),
            b"Expected .cabal files or additional_dependencies".to_vec(),
            1
        )
    );
    assert_eq!(
        envelope(LangError::Assertion(
            "This language is not installable".into()
        )),
        (
            "AssertionError".into(),
            b"This language is not installable".to_vec(),
            3
        )
    );
    assert_eq!(
        envelope(LangError::Value("entry must start with `Rscript`.".into())),
        (
            "ValueError".into(),
            b"entry must start with `Rscript`.".to_vec(),
            3
        )
    );
    assert_eq!(
        envelope(LangError::CalledProcess {
            cmd: vec![OsString::from("luarocks"), OsString::from("config"), OsString::from("--lua-ver")],
            rc: 1,
            stdout: b"Executable `luarocks` not found".to_vec(),
            stderr: Vec::new(),
        }),
        (
            "CalledProcessError".into(),
            b"command: ('luarocks', 'config', '--lua-ver')\nreturn code: 1\nstdout:\n    Executable `luarocks` not found\nstderr: (none)"
                .to_vec(),
            3
        )
    );
    let (ty, text, code) = envelope(LangError::Io {
        op: "rmtree",
        path: PathBuf::from("/x/y"),
        err: io::Error::from_raw_os_error(2),
    });
    assert_eq!((ty.as_str(), code), ("FileNotFoundError", 3));
    assert!(String::from_utf8(text).unwrap().ends_with(": '/x/y'"));
    assert_eq!(envelope(LangError::Interrupted).2, 130);
    assert_eq!(envelope(LangError::Net(NetError::Cancelled)).2, 130);
    assert_eq!(envelope(LangError::from(NetError::Cancelled)).2, 130);
}

#[test]
fn download_failures() {
    let (ty, text, code) = envelope(LangError::Net(NetError::Checksum {
        url: "https://x/a.tar.gz".into(),
        expected: "aa".into(),
        actual: "bb".into(),
    }));
    assert_eq!((ty.as_str(), code), ("NetError", 3));
    assert_eq!(
        text,
        b"checksum mismatch for https://x/a.tar.gz: expected aa, got bb"
    );
    let (ty, _, _) = envelope(LangError::Net(NetError::Http {
        url: "https://x".into(),
        status: 500,
    }));
    assert_eq!(ty, "HTTPError");
    let (ty, _, _) = envelope(LangError::Net(NetError::Transport {
        url: "https://x".into(),
        msg: "dns".into(),
    }));
    assert_eq!(ty, "URLError");
    let (ty, text, _) = envelope(LangError::Net(NetError::Offline));
    assert_eq!(
        (ty.as_str(), text),
        (
            "NetError",
            b"network access disabled (SNP_OFFLINE)".to_vec()
        )
    );
    let (ty, _, code) = envelope(LangError::Net(NetError::Io(io::Error::from(
        io::ErrorKind::PermissionDenied,
    ))));
    assert_eq!((ty.as_str(), code), ("PermissionError", 3));
}

#[test]
fn display_is_the_exception_text() {
    assert_eq!(LangError::Assertion("x".into()).to_string(), "x");
    assert_eq!(LangError::Interrupted.py_type(), "KeyboardInterrupt");
    let e = LangError::CalledProcess {
        cmd: vec![OsString::from("a")],
        rc: 2,
        stdout: Vec::new(),
        stderr: b"err\n".to_vec(),
    };
    assert_eq!(
        e.to_string(),
        "command: ('a',)\nreturn code: 2\nstdout: (none)\nstderr:\n    err"
    );
    assert!(std::error::Error::source(&LangError::Net(NetError::Offline)).is_some());
}
