//! On-disk names of the versioned layout (ARCHITECTURE §5.2) and small filesystem helpers shared by the store and
//! its maintenance code.
//!
//! Owner: WP-22.

use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// `repos-v1/<key>/` + `repos-v1/<key>.lock`.
pub(crate) const REPOS: &str = "repos-v1";
/// `keys-v1/<hex32>`: message-parity markers.
pub(crate) const KEYS: &str = "keys-v1";
/// `configs-v1/<hex32>`: tracked configs.
pub(crate) const CONFIGS: &str = "configs-v1";
/// `tools-v1/<tool>/<version>/` + `tools-v1/<tool>.lock`.
pub(crate) const TOOLS: &str = "tools-v1";
/// `cache-v1/<name>/`.
pub(crate) const CACHE: &str = "cache-v1";
/// Stash patches and journals.
pub(crate) const PATCHES: &str = "patches";
/// Unnamed spill files.
pub(crate) const SPILL: &str = "spill";
/// In-progress clones and extractions.
pub(crate) const SCRATCH: &str = "scratch";
/// Retired directories pending deletion.
pub(crate) const TRASH: &str = "trash";
/// The global lock file.
pub(crate) const LOCK: &str = ".lock";
/// The store README.
pub(crate) const README: &str = "README";
/// Relocation detection.
pub(crate) const STORE_FILE: &str = "store-v1.json";
/// A repository's commit marker (written last).
pub(crate) const REPO_MARKER: &str = ".snp-repo.json";
/// An environment's tool references (store-owned).
pub(crate) const TOOLS_SIDECAR: &str = ".snp-tools.json";
/// An environment's install state (the commit point, opaque to the store).
pub(crate) const INSTALL_STATE: &str = ".snp-install-state.json";
/// Stash journals in `patches/`.
pub(crate) const JOURNAL_SUFFIX: &str = ".snp-journal";

/// The versioned buckets (`<name>-v<N>`): a bucket of another format version is never parsed and is removed by
/// `sweep` (11 §A.10.3).
pub(crate) const BUCKETS: [&str; 5] = ["repos", "keys", "configs", "tools", "cache"];

/// The current format version of every bucket.
pub(crate) const BUCKET_VERSION: u32 = 1;

/// snp 1.x top-level entries (never created by snp 2; removed only behind the 1.x README signature).
pub(crate) const SNP1_DIRS: [&str; 3] = ["repos", "environments", "resource-pools"];
/// snp 1.x's SQLite file.
pub(crate) const SNP1_DB: &str = "db.db";

/// `mkdir -p` with every created component 0700 on Unix (ARCHITECTURE §5.1).
pub(crate) fn mkdir_private(path: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path)
}

/// A fresh random hex name (scratch directories).
pub(crate) fn random_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let v = RandomState::new().hash_one((std::process::id(), n, SystemTime::now()));
    format!("{v:016x}")
}

/// Whether `name` is a safe single path component: `^[A-Za-z0-9._+-]{1,64}$`, not `.`/`..` (ADR-009), and not
/// aliased to another name by the filesystem: no trailing `.` (Win32 strips trailing dots, so `3.12.` would open
/// `3.12`) and, on the case-insensitive defaults of Windows and macOS, no upper-case ASCII (`3.12A` and `3.12a`
/// would share one directory). An aliasable name is hashed instead, so distinct versions never share an
/// environment.
pub(crate) fn is_safe_component(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.ends_with('.')
        && !(CASE_INSENSITIVE_FS && name.bytes().any(|b| b.is_ascii_uppercase()))
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// Whether the platform's default filesystems fold ASCII case (NTFS, APFS/HFS+ as installed).
pub(crate) const CASE_INSENSITIVE_FS: bool = cfg!(any(windows, target_vendor = "apple"));

/// `name` itself when it is a safe component, else `h<hex16(blake3(name))>`: never a separator, never `..`.
pub(crate) fn safe_component(name: &str) -> String {
    if is_safe_component(name) {
        name.to_owned()
    } else {
        format!("h{}", crate::keys::version_hash16(name))
    }
}

/// `<versioned bucket>` names: `Some((bucket, N))` for `<bucket>-v<N>`.
pub(crate) fn parse_bucket(name: &str) -> Option<(&str, u32)> {
    let (bucket, v) = name.rsplit_once("-v")?;
    if !BUCKETS.contains(&bucket) || v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((bucket, v.parse().ok()?))
}

/// fsync a directory (Unix; a no-op elsewhere): makes a rename into it durable.
pub(crate) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(f) = fs::File::open(dir) {
            let _synced: io::Result<()> = f.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _: &Path = dir;
    }
}

/// Age of an entry at `now` from its `lstat` mtime (`None` when it cannot be read or lies in the future).
pub(crate) fn age(path: &Path, now: SystemTime) -> Option<std::time::Duration> {
    let m = fs::symlink_metadata(path).ok()?.modified().ok()?;
    now.duration_since(m).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_components() {
        for ok in [
            "3.12",
            "python3.12",
            "default",
            "system",
            "1.0+local",
            "a-b_c",
            "x",
        ] {
            assert!(is_safe_component(ok), "{ok}");
            assert_eq!(safe_component(ok), ok);
        }
        let long: String = std::iter::repeat_n('a', 65).collect();
        for bad in [
            "",
            ".",
            "..",
            "../../x",
            "/abs",
            "C:\\x",
            "a/b",
            "a\\b",
            "a\0b",
            "a b",
            "é",
            "3.12.",
            "3.12 ",
            "...",
            long.as_str(),
        ] {
            assert!(!is_safe_component(bad), "{bad:?}");
            let h = safe_component(bad);
            assert!(h.starts_with('h') && h.len() == 17, "{h}");
            assert!(is_safe_component(&h));
        }
        assert_ne!(safe_component("../../x"), safe_component("/abs"));
        assert_ne!(safe_component("3.12."), "3.12");
        // Case aliases: hashed where the filesystem folds case, kept verbatim elsewhere.
        assert_eq!(is_safe_component("3.12A"), !CASE_INSENSITIVE_FS);
        assert!(is_safe_component("3.12a"));
        assert_ne!(safe_component("3.12A"), safe_component("3.12a"));
    }

    #[test]
    fn buckets() {
        assert_eq!(parse_bucket("repos-v1"), Some(("repos", 1)));
        assert_eq!(parse_bucket("keys-v12"), Some(("keys", 12)));
        assert_eq!(parse_bucket("repos-v"), None);
        assert_eq!(parse_bucket("other-v1"), None);
        assert_eq!(parse_bucket("repos"), None);
        assert_eq!(parse_bucket("repos-vx"), None);
    }

    #[test]
    fn random_names_differ() {
        assert_ne!(random_name(), random_name());
        assert_eq!(random_name().len(), 16);
    }
}
