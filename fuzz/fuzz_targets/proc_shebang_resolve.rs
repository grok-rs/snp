//! Fuzz target `proc_shebang_resolve`: shebang parsing in executable resolution.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input becomes the contents of an executable script `run` in a private directory that also holds an
//! executable `interp`; `normalize_cmd(["./run", "arg"])` must never panic, and on success returns a non-empty argv
//! that ends with the script and its argument (one level of shebang interpretation, `parse_shebang.py:65-85`).
//!
//! Owner: WP-15.
#![no_main]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use snp_core::env::Env;
use snp_proc::resolve::normalize_cmd;

/// A per-process scratch directory with an executable `interp` on PATH.
fn workdir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("snp-fuzz-shebang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("interp"), b"").unwrap();
        make_executable(&dir.join("interp"));
        dir
    })
}

fn make_executable(p: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = p;
}

fuzz_target!(|data: &[u8]| {
    let dir = workdir();
    let script = dir.join("run");
    if std::fs::write(&script, data).is_err() {
        return;
    }
    make_executable(&script);
    let env = Env::from_pairs([("PATH", dir.as_os_str())]);
    let cmd = [OsString::from("./run"), OsString::from("arg")];
    if let Ok(argv) = normalize_cmd(&cmd, &env, dir) {
        assert!(argv.len() >= 2);
        assert_eq!(argv.last(), Some(&OsString::from("arg")));
        let script_pos = argv.len() - 2;
        assert_eq!(argv.get(script_pos), Some(&OsString::from("./run")));
    }
});
