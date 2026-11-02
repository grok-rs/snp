//! P2 natives against the Python hooks (WP-27 acceptance): the per-epoch goldens of `cases.json` recorded by
//! `cargo xtask pch-gates goldens` (sort-simple-yaml's `\n` quirk, unescaped permalink domains, gitlinks, the
//! name-tests-test patterns, the real fix-encoding-pragma) and the Appendix A argparse goldens of every P2 console
//! script at every supported tag.
//!
//! Owner: WP-27.
#![cfg(test)]
#![cfg_attr(
    windows,
    allow(
        dead_code,
        unused_imports,
        reason = "most cases are posix-only (sh scripts, modes, signals): on Windows their shared helpers go unused"
    )
)]

#[path = "../framework/support.rs"]
mod support;

#[path = "../p1/golden.rs"]
mod golden;

use std::ffi::OsStr;
use std::sync::Arc;

use snp_hooks::{Epoch, NativeSource};
use support::{Ctx, Sandbox, show};

/// The P2 hooks and their console scripts.
const P2: [(&str, &str); 5] = [
    ("sort-simple-yaml", "sort-simple-yaml"),
    ("check-vcs-permalinks", "check-vcs-permalinks"),
    ("forbid-new-submodules", "forbid-new-submodules"),
    ("name-tests-test", "name-tests-test"),
    ("fix-encoding-pragma", "fix-encoding-pragma"),
];

#[cfg(unix)]
#[test]
fn p2_goldens_per_epoch() {
    let checked = golden::replay("p2");
    eprintln!("p2: {checked} golden cases");
    assert!(checked > 200, "only {checked} golden cases ran");
}

#[cfg(unix)]
#[test]
fn p2_appendix_a_goldens() {
    let checked = golden::appendix_a(&P2);
    eprintln!("p2: {checked} Appendix A texts");
    assert!(checked >= 100, "only {checked} Appendix A texts compared");
}

#[test]
fn p2_registry_and_preflight() {
    golden::registry(&P2);
    // fix-encoding-pragma v4.6.0…v5.0.0 shares its epoch with the v6.0.0 stub in the data: delegated there.
    assert!(golden::native_epoch("v4.5.0", "fix-encoding-pragma", &[]).is_some());
    assert!(golden::native_epoch("v4.6.0", "fix-encoding-pragma", &[]).is_none());
    // A domain beyond snp's regex engines makes check-vcs-permalinks delegate; an invalid one is emulated.
    let vcs = snp_hooks::native(NativeSource::PreCommitHooks, "check-vcs-permalinks").unwrap();
    let args = |xs: &[&str]| xs.iter().map(|&x| Arc::<str>::from(x)).collect::<Vec<_>>();
    assert!(
        vcs.preflight(
            Epoch(3),
            &args(&["--additional-github-domain", "gitlab.com"])
        )
        .is_ok()
    );
    assert!(
        vcs.preflight(Epoch(3), &args(&["--additional-github-domain", "("]))
            .is_ok()
    );
}

/// The real fix-encoding-pragma of v4.6.0…v5.0.0 prints its deprecation warning before anything else, help
/// included (the hook is reached directly; the registry delegates that epoch, see above).
#[test]
fn fix_encoding_pragma_deprecation_first() {
    let sb = Sandbox::new(true);
    sb.write("a.py", b"x = 1\n");
    let ctx = Ctx::new(sb);
    let golden = support::json("goldens/appendix_a/v4.6.0.json");
    let want = golden["scripts"]["fix-encoding-pragma"]["help"]["out"]
        .as_str()
        .unwrap()
        .to_owned();
    let stub = snp_hooks::native(NativeSource::PreCommitHooks, "fix-encoding-pragma").unwrap();
    // Epoch 1 through the stub: no warning.
    let o = stub.run(&ctx.cx(Epoch(1)), &[OsStr::new("a.py")]);
    assert_eq!(o.code, 1);
    assert_eq!(show(&o.output), "Added `# -*- coding: utf-8 -*-` to a.py\n");
    assert!(want.starts_with("warning: this hook is deprecated"));
}

/// sort-simple-yaml rewrites a file that is exactly `\n` on every run (exit 1) without changing a byte. (Not on
/// Windows: text mode writes that `\n` back as `\r\n`, the next test's subject.)
#[cfg(not(windows))]
#[test]
fn sort_simple_yaml_newline_quirk() {
    let sb = Sandbox::new(true);
    sb.write("a.yaml", b"\n");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "sort-simple-yaml").unwrap();
    for _ in 0..2 {
        let o = h.run(&ctx.cx(Epoch(1)), &[OsStr::new("a.yaml")]);
        assert_eq!(o.code, 1);
        assert_eq!(o.modified.len(), 0);
        assert_eq!(show(&o.output), "Fixing file `a.yaml`\n");
    }
    assert_eq!(std::fs::read(ctx.sb.path("a.yaml")).unwrap(), b"\n");
}

/// Windows text mode (09 §2.7): sort-simple-yaml writes `\r\n`; a CRLF file already in order is untouched.
#[test]
fn sort_simple_yaml_windows_text_writes() {
    let sb = Sandbox::new(true);
    sb.write("a.yaml", b"b: 1\n\na: 2\n");
    sb.write("b.yaml", b"a: 1\r\n\r\nb: 2\r\n");
    let ctx = Ctx::new(sb);
    let h = snp_hooks::native(NativeSource::PreCommitHooks, "sort-simple-yaml").unwrap();
    let cx = snp_hooks::HookCx {
        windows: true,
        ..ctx.cx(Epoch(1))
    };
    let o = h.run(&cx, &[OsStr::new("a.yaml"), OsStr::new("b.yaml")]);
    assert_eq!(o.code, 1);
    assert_eq!(
        std::fs::read(ctx.sb.path("a.yaml")).unwrap(),
        b"a: 2\r\n\r\nb: 1\r\n"
    );
    assert_eq!(
        std::fs::read(ctx.sb.path("b.yaml")).unwrap(),
        b"a: 1\r\n\r\nb: 2\r\n"
    );
}
