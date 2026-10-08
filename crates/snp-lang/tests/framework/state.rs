//! `InstallState` JSON (ADR-012, X13-ENV-01: malformed ⇒ not installed).

use std::path::PathBuf;
use std::sync::Arc;

use snp_lang::{Fingerprint, InstallState};

fn state() -> InstallState {
    InstallState {
        v: InstallState::VERSION,
        language: Arc::from("python"),
        language_version: Arc::from("python3.12"),
        deps: Arc::from([Arc::from("six"), Arc::from("attrs==23.1")]),
        installer: Arc::from("uv"),
        fingerprint: Some(Fingerprint {
            exe: PathBuf::from("/usr/bin/python3.12"),
            len: 6_000_000,
            mtime_ns: 1_700_000_000_123_456_789,
            ino_or_index: 42,
            detail: Arc::from("3.12.3.final.0"),
        }),
        snp_version: Arc::from("2.0.0"),
    }
}

#[test]
fn round_trip() {
    let s = state();
    let json = s.to_json();
    let back = InstallState::from_json(&json).unwrap();
    assert_eq!(back.to_json(), json);
    assert_eq!(&*back.language_version, "python3.12");
    assert_eq!(back.deps.len(), 2);
    assert_eq!(back.fingerprint, s.fingerprint);
    // Compact, deterministic JSON with the fields in declaration order.
    let text = String::from_utf8(json).unwrap();
    assert!(text.starts_with(r#"{"v":1,"language":"python","language_version":"python3.12","deps":["six","attrs==23.1"]"#), "{text}");
    // Pre-epoch mtimes survive.
    let mut neg = state();
    neg.fingerprint.as_mut().unwrap().mtime_ns = -5;
    assert_eq!(
        InstallState::from_json(&neg.to_json())
            .unwrap()
            .fingerprint
            .unwrap()
            .mtime_ns,
        -5
    );
}

#[test]
fn malformed_means_not_installed() {
    let good = String::from_utf8(state().to_json()).unwrap();
    for bad in [
        String::new(),
        "{".to_owned(),
        "null".to_owned(),
        "[]".to_owned(),
        // pre-commit's own state file.
        r#"{"additional_dependencies": []}"#.to_owned(),
        good.replace(r#""v":1"#, r#""v":2"#),
        good.replace(r#""v":1"#, r#""v":"1""#),
        good.replace(r#","installer":"uv""#, ""),
        good.replace(r#""len":6000000"#, r#""len":-1"#),
    ] {
        assert!(InstallState::from_json(bad.as_bytes()).is_none(), "{bad}");
    }
    assert!(InstallState::from_json(b"\xff\xfe").is_none());
    let none = InstallState {
        fingerprint: None,
        ..state()
    };
    assert!(
        InstallState::from_json(&none.to_json())
            .unwrap()
            .fingerprint
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_interpreter_paths_round_trip() {
    use std::os::unix::ffi::OsStringExt;
    let mut s = state();
    let exe = PathBuf::from(std::ffi::OsString::from_vec(
        b"/opt/py\xff/bin/python".to_vec(),
    ));
    s.fingerprint.as_mut().unwrap().exe = exe.clone();
    let json = s.to_json();
    assert_ne!(json.len(), 0);
    assert_eq!(
        InstallState::from_json(&json)
            .unwrap()
            .fingerprint
            .unwrap()
            .exe,
        exe
    );
}
