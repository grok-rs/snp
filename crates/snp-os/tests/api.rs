//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.7 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-15 (created by WP-00).

#[cfg(test)]
mod api {
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use snp_core::cancel::CancelToken;
    use snp_core::env::Env;
    use snp_core::output::ColorSetting;
    use snp_os::fs::{self, Ownership};
    use snp_os::lock::{FileLock, LockMode};
    use snp_os::{fsinfo, home, realpath, term};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Ownership>();
        send_sync::<LockMode>();
        send_sync::<FileLock>();
    }

    #[test]
    fn fs() {
        has::<fn(&Path, &Path) -> io::Result<()>>(fs::rename_with_retry);
        has::<fn(&Path, &[u8]) -> io::Result<()>>(fs::write_new_durable);
        has::<fn(&Path, &mut dyn io::Read) -> io::Result<u64>>(fs::copy_into_new_durable);
        has::<fn(&Path, &[u8], Option<u32>) -> io::Result<()>>(fs::atomic_replace);
        has::<fn(&Path, &[u8]) -> io::Result<()>>(fs::rewrite_in_place);
        has::<fn(&Path, &Path) -> io::Result<()>>(fs::remove_tree_within);
        has::<fn(&Path) -> io::Result<File>>(fs::unnamed_temp);
        has::<fn(&Path, &Path) -> io::Result<()>>(fs::retire_dir);
        has::<fn(&Path, u64) -> io::Result<Vec<u8>>>(fs::read_capped);
        has::<fn(&Path) -> io::Result<File>>(fs::create_truncate_nofollow);
        has::<fn(&Path) -> io::Result<Ownership>>(fs::ensure_private_dir);
        has::<[Ownership; 2]>([
            Ownership::Own,
            Ownership::Foreign {
                root_over_user: true,
            },
        ]);
        has::<fn(&Path, &Path) -> io::Result<bool>>(snp_os::same_file);
    }

    #[test]
    fn lock() {
        has::<[LockMode; 2]>([LockMode::Shared, LockMode::Exclusive]);
        has::<fn(&Path, LockMode) -> io::Result<Option<FileLock>>>(FileLock::try_acquire);
        has::<
            fn(
                &Path,
                LockMode,
                &mut dyn FnMut(),
                &CancelToken,
                Option<Duration>,
            ) -> io::Result<FileLock>,
        >(FileLock::acquire);
        has::<fn(&FileLock) -> &File>(FileLock::file);
    }

    #[test]
    fn term_home_realpath_fsinfo() {
        has::<fn(ColorSetting, &Env) -> bool>(term::decide_color);
        has::<fn() -> Option<usize>>(term::stdout_tty_columns);
        has::<fn() -> bool>(term::stdout_is_tty);
        has::<fn() -> bool>(term::stdout_is_windows_console);
        has::<fn()>(term::ensure_vt);
        has::<fn(&OsStr, &Env) -> OsString>(home::expanduser);
        has::<fn(&Env) -> Option<PathBuf>>(home::default_cache_root);
        has::<fn(&Path, &Path) -> io::Result<PathBuf>>(realpath::realpath_nonstrict);
        has::<fn(&Path) -> io::Result<bool>>(fsinfo::stat_oracle_safe);
    }
}
