//! `GitExe::resolve` (version from `git --version` or the cache) and `version_gate` (D-DIAG-01, ADR-063).

use std::io::Write;
use std::sync::{Arc, Mutex};

use snp_core::brand::Brand;
use snp_core::cancel::CancelToken;
use snp_core::env::{Env, EnvPatch};
use snp_core::output::{Printer, PrinterOpts};
use snp_git::{GitCaps, GitExe, GitVersion};

use crate::common::{Sandbox, exc};

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn printer(brand: Brand, color: bool) -> (Printer, Buf, Buf) {
    let (out, err) = (Buf::default(), Buf::default());
    let p = Printer::new(
        Box::new(out.clone()),
        Box::new(err.clone()),
        PrinterOpts {
            brand,
            color,
            stdout_is_tty: false,
            windows_text_mode: false,
            cancel: CancelToken::new(),
        },
    );
    (p, out, err)
}

fn exe_at(v: Option<GitVersion>) -> GitExe {
    GitExe {
        argv0: vec!["/usr/bin/git".into()],
        caps: GitCaps {
            version: v,
            ls_files_dedup: true,
            path_format: true,
            config_env: true,
            init_object_format: true,
            config_hooks: false,
            pathspec_from_file: true,
            diff_no_relative: true,
        },
    }
}

#[test]
fn resolve_reads_the_real_version_and_caps() {
    let sb = Sandbox::new();
    let exe = GitExe::resolve(&sb.env, &sb.root, None).unwrap();
    assert_eq!(exe.argv0.len(), 1);
    assert!(std::path::Path::new(&exe.argv0[0]).is_absolute());
    let raw = sb.git_text(&sb.root, &["--version"]);
    let v = exe.caps.version.expect("git --version parses");
    assert!(raw.starts_with(&format!("git version {v}")), "{raw} vs {v}");
    assert_eq!(
        exe.caps.ls_files_dedup,
        v >= GitVersion {
            major: 2,
            minor: 31,
            patch: 0
        }
    );
}

#[test]
fn missing_git_is_pre_commits_get_root_fatal_error() {
    let sb = Sandbox::new();
    let env = sb
        .env
        .patched(&EnvPatch::default().set("PATH", sb.root.join("empty-bin")));
    let e = GitExe::resolve(&env, &sb.root, None).unwrap_err();
    assert_eq!(
        exc(&e),
        (
            "FatalError".to_owned(),
            "git failed. Is it installed, and are you in a Git repository directory?".to_owned()
        )
    );
    assert_eq!(e.exit_code(), 1);
}

#[test]
fn version_cache_trusts_only_passing_entries() {
    let sb = Sandbox::new();
    let cache = sb.path("git-version.cache");
    let real = GitExe::resolve(&sb.env, &sb.root, Some(&cache)).unwrap();
    let v = real.caps.version.unwrap();
    let text = std::fs::read_to_string(&cache).unwrap();
    assert!(text.starts_with("snp-git-version-cache v1\n"), "{text}");
    // a hit costs no spawn: a doctored passing version is believed …
    std::fs::write(&cache, text.replacen(&v.to_string(), "2.99.0", 1)).unwrap();
    let hit = GitExe::resolve(&sb.env, &sb.root, Some(&cache)).unwrap();
    assert_eq!(
        hit.caps.version,
        Some(GitVersion {
            major: 2,
            minor: 99,
            patch: 0
        })
    );
    // … a below-floor entry never is (an upgraded git behind a stale entry must not stay "old")
    std::fs::write(&cache, text.replacen(&v.to_string(), "2.30.0", 1)).unwrap();
    let miss = GitExe::resolve(&sb.env, &sb.root, Some(&cache)).unwrap();
    assert_eq!(miss.caps.version, Some(v));
    // GIT_EXEC_PATH is part of the key
    std::fs::write(&cache, text.replacen(&v.to_string(), "2.99.0", 1)).unwrap();
    let env = sb
        .env
        .patched(&EnvPatch::default().set("GIT_EXEC_PATH", "/elsewhere"));
    let other = GitExe::resolve(&env, &sb.root, Some(&cache)).unwrap();
    assert_ne!(
        other.caps.version,
        Some(GitVersion {
            major: 2,
            minor: 99,
            patch: 0
        })
    );
}

/// The only test of this binary that lets the process-wide D-DIAG-01 latch fire.
#[test]
fn version_gate_never_refuses_and_warns_once() {
    let old = Some(GitVersion {
        major: 2,
        minor: 30,
        patch: 2,
    });
    let quiet = Env::from_pairs([("SNP_ALLOW_OLD_GIT", "1")]);
    let (p, out, err) = printer(Brand::Snp, false);
    exe_at(old).version_gate(&p, &quiet).unwrap();
    exe_at(Some(GitVersion {
        major: 2,
        minor: 31,
        patch: 0,
    }))
    .version_gate(&p, &Env::from_pairs::<&str, &str>([]))
    .unwrap();
    exe_at(None)
        .version_gate(&p, &Env::from_pairs::<&str, &str>([]))
        .unwrap();
    assert!(err.0.lock().unwrap().is_empty());
    let loud = Env::from_pairs([("SNP_ALLOW_OLD_GIT", "")]);
    exe_at(old).version_gate(&p, &loud).unwrap();
    exe_at(Some(GitVersion {
        major: 2,
        minor: 18,
        patch: 5,
    }))
    .version_gate(&p, &loud)
    .unwrap();
    p.finish();
    assert_eq!(
        String::from_utf8(err.0.lock().unwrap().clone()).unwrap(),
        "[WARNING] git 2.30.2 is older than 2.31.0, the oldest git version snp is tested with. Set \
         SNP_ALLOW_OLD_GIT=1 to silence this warning.\n"
    );
    assert!(
        out.0.lock().unwrap().is_empty(),
        "stdout stays byte-identical to pre-commit"
    );
}
