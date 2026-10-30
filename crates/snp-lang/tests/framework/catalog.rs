//! Catalog rows owned by WP-16 (10 §5.4 T10-XRG-20…22, §5.18 T10-UTL-02; ADR-062 names).

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::env::Env;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_lang::common;
use snp_lang::{CmdRunner, LangError, check_support};

/// An executable file at `p` (parents created).
fn exe(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn env(pairs: &[(&str, &OsStr)]) -> Env {
    Env::from_pairs(pairs.iter().map(|(k, v)| (*k, v.to_os_string())))
}

/// `lang_base_test.py:33-62`: `exe_exists('ruby')` — not found; a system path; a shims directory; inside the home
/// directory; `commonpath` raising `ValueError` (relative result); home `/`.
#[cfg(unix)]
#[test]
fn t10_xrg_20_exe_exists() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let home = r.join("home/me");
    std::fs::create_dir_all(&home).unwrap();
    let ruby = OsStr::new("ruby");

    // Not found.
    let e = env(&[
        ("PATH", r.join("empty").as_os_str()),
        ("HOME", home.as_os_str()),
    ]);
    assert!(!common::exe_exists(ruby, &e, r));

    // A system-wide executable outside $HOME.
    exe(&r.join("usr/bin/ruby"));
    let e = env(&[
        ("PATH", r.join("usr/bin").as_os_str()),
        ("HOME", home.as_os_str()),
    ]);
    assert!(common::exe_exists(ruby, &e, r));

    // rbenv/pyenv/asdf-style shims are not "system" (`[/\\]shims[/\\]`).
    exe(&r.join("foo/shims/ruby"));
    let e = env(&[
        ("PATH", r.join("foo/shims").as_os_str()),
        ("HOME", home.as_os_str()),
    ]);
    assert!(!common::exe_exists(ruby, &e, r));

    // Inside the home directory (nvm, ~/sdk, ~/.rbenv).
    exe(&home.join("somedir/ruby"));
    let e = env(&[
        ("PATH", home.join("somedir").as_os_str()),
        ("HOME", home.as_os_str()),
    ]);
    assert!(!common::exe_exists(ruby, &e, r));

    // A relative PATH entry gives a relative result: `commonpath` raises ValueError, which counts as "not inside".
    exe(&r.join("rel/ruby"));
    let e = env(&[("PATH", OsStr::new("rel")), ("HOME", home.as_os_str())]);
    assert!(common::exe_exists(ruby, &e, r));

    // home == '/': everything counts as system.
    let e = env(&[
        ("PATH", home.join("somedir").as_os_str()),
        ("HOME", OsStr::new("/")),
    ]);
    assert!(common::exe_exists(ruby, &e, r));
}

/// The Windows flavour of T10-XRG-20 (`USERPROFILE`, `PATHEXT`).
#[cfg(windows)]
#[test]
fn t10_xrg_20_exe_exists() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let home = r.join("home");
    std::fs::create_dir_all(&home).unwrap();
    exe(&r.join("sys\\ruby.exe"));
    exe(&r.join("foo\\shims\\ruby.exe"));
    exe(&home.join("dir\\ruby.exe"));
    let ruby = OsStr::new("ruby");
    let mk = |path: &Path| {
        env(&[
            ("PATH", path.as_os_str()),
            ("PATHEXT", OsStr::new(".EXE")),
            ("USERPROFILE", home.as_os_str()),
        ])
    };
    assert!(common::exe_exists(ruby, &mk(&r.join("sys")), r));
    assert!(!common::exe_exists(ruby, &mk(&r.join("foo\\shims")), r));
    assert!(!common::exe_exists(ruby, &mk(&home.join("dir")), r));
    assert!(!common::exe_exists(ruby, &mk(&r.join("none")), r));
}

/// `lang_base_test.py:73-82`: a setup command that writes `\x81\xfe` to stderr and exits 1 raises
/// `CalledProcessError`; the envelope renders the raw bytes (format A) without a Unicode crash.
#[test]
fn t10_xrg_21_setup_cmd_non_utf8_stderr() {
    #[derive(Debug)]
    struct Fails;
    impl CmdRunner for Fails {
        fn run(
            &self,
            _argv: &[OsString],
            _env: &Env,
            _cwd: &Path,
        ) -> Result<snp_proc::spawn::Captured, LangError> {
            Ok(snp_proc::spawn::Captured {
                status: snp_proc::spawn::PyStatus(1),
                stdout: Vec::new(),
                stderr: b"\x81\xfe".to_vec(),
                stopped_by_tty: false,
            })
        }
    }
    let argv = vec![
        OsString::from("i-do-not-exist-anywhere"),
        OsString::from("x"),
    ];
    let e = common::setup_cmd(
        &Fails,
        &argv,
        &Env::from_pairs([("PATH", "")]),
        Path::new("/"),
    )
    .unwrap_err();
    let LangError::CalledProcess {
        cmd,
        rc,
        stdout,
        stderr,
    } = &e
    else {
        panic!("{e:?}");
    };
    assert_eq!(cmd, &argv, "unresolvable argv[0]: the argv as given");
    assert_eq!(*rc, 1);
    assert_eq!(stdout.len(), 0);
    assert_eq!(stderr, b"\x81\xfe");
    let f = Failure::from(e);
    assert_eq!(f.exit_code(), 3);
    let Failure::Exc { exc, .. } = &f else {
        panic!("{f:?}")
    };
    assert_eq!(exc.ty, "CalledProcessError");
    assert_eq!(
        exc.text,
        b"command: ('i-do-not-exist-anywhere', 'x')\nreturn code: 1\nstdout: (none)\nstderr:\n    \x81\xfe"
    );
}

/// The real runner on a real script (POSIX): the error carries the normalised argv (shebang interpreter first).
#[cfg(unix)]
#[test]
fn t10_xrg_21_setup_cmd_non_utf8_stderr_real_process() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let script = d.path().join("bad-tool");
    std::fs::write(&script, b"#!/bin/sh\nprintf '\\201\\376' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let runner = snp_lang::ProcRunner {
        cancel: snp_core::cancel::CancelToken::new(),
    };
    let e = Env::from_pairs([("PATH", format!("{}:/usr/bin:/bin", d.path().display()))]);
    let err = common::setup_cmd(&runner, &[OsString::from("bad-tool")], &e, d.path()).unwrap_err();
    let LangError::CalledProcess {
        cmd, rc, stderr, ..
    } = &err
    else {
        panic!("{err:?}");
    };
    assert_eq!(cmd, &[OsString::from("/bin/sh"), script.into_os_string()]);
    assert_eq!(*rc, 1);
    assert_eq!(stderr, b"\x81\xfe");
}

/// `lang_base_test.py:85-108`: the environment directory of a language is `<prefix>/<ENVIRONMENT_DIR>-<version>`
/// (snp keeps pre-commit's `ENVIRONMENT_DIR` names; the store sanitises the version, ADR-009), and the two
/// assertion texts are verbatim.
#[test]
fn t10_xrg_22_environment_dir_and_assertions() {
    let expected = [
        (LanguageId::Conda, Some("conda")),
        (LanguageId::Coursier, Some("coursier")),
        (LanguageId::Dart, Some("dartenv")),
        (LanguageId::Docker, Some("docker")),
        (LanguageId::DockerImage, None),
        (LanguageId::Dotnet, Some("dotnetenv")),
        (LanguageId::Fail, None),
        (LanguageId::Golang, Some("golangenv")),
        (LanguageId::Haskell, Some("hs_env")),
        (LanguageId::Julia, Some("juliaenv")),
        (LanguageId::Lua, Some("lua_env")),
        (LanguageId::Node, Some("node_env")),
        (LanguageId::Perl, Some("perl_env")),
        (LanguageId::Pygrep, None),
        (LanguageId::Python, Some("py_env")),
        (LanguageId::R, Some("renv")),
        (LanguageId::Ruby, Some("rbenv")),
        (LanguageId::Rust, Some("rustenv")),
        (LanguageId::Swift, Some("swift_env")),
        (LanguageId::Unsupported, None),
        (LanguageId::UnsupportedScript, None),
    ];
    for (id, base) in expected {
        assert_eq!(id.env_dir_base(), base, "{id:?}");
    }
    // The `{prefix}/{base}-{version}` join itself is `snp_store::Store::env_dir` (WP-31, a W0 stub here); its own
    // test covers that half of T10-XRG-22. This crate owns the per-language bases above.

    let e = check_support(LanguageId::Docker, "1.2.3", &[], Brand::PreCommit).unwrap_err();
    assert_eq!(
        e.to_string(),
        "for now, pre-commit requires system-installed docker -- you selected `language_version: 1.2.3`"
    );
    let deps: [Arc<str>; 1] = [Arc::from("hmmm")];
    let e = check_support(LanguageId::Docker, "default", &deps, Brand::PreCommit).unwrap_err();
    assert_eq!(
        e.to_string(),
        "for now, pre-commit does not support additional_dependencies for docker -- you selected \
         `additional_dependencies: ['hmmm']`"
    );
    assert!(matches!(e, LangError::Assertion(_)));
}

/// `util_test.py:344-376`: `clean_path_on_failure` removes the path on any failure — an error or an unwinding
/// panic (Python's `SystemExit`/`KeyboardInterrupt` are `BaseException`s too) — and keeps it on success.
#[test]
fn t10_utl_02_clean_path_on_failure() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let env_dir = root.join("py_env-default");
    let make = || {
        std::fs::create_dir_all(env_dir.join("bin")).unwrap();
        std::fs::write(env_dir.join("bin/python"), b"x").unwrap();
    };

    // Success keeps it.
    let r: Result<u8, String> = common::clean_path_on_failure(&env_dir, root, || {
        make();
        Ok(7)
    });
    assert_eq!(r, Ok(7));
    assert!(env_dir.join("bin/python").exists());

    // An error removes it.
    let r: Result<(), String> =
        common::clean_path_on_failure(&env_dir, root, || Err("boom".to_owned()));
    assert_eq!(r, Err("boom".to_owned()));
    assert!(!env_dir.exists());

    // A panic (SystemExit's analogue: an exit that unwinds through) removes it too.
    make();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let outcome: Result<(), ()> =
            common::clean_path_on_failure(&env_dir, root, || panic!("SystemExit"));
        assert!(outcome.is_ok(), "unreachable: the closure panics");
    }));
    assert!(caught.is_err());
    assert!(!env_dir.exists());

    // A failure before the path exists is fine.
    let r: Result<(), &str> =
        common::clean_path_on_failure(&root.join("never"), root, || Err("early"));
    assert_eq!(r, Err("early"));
}
