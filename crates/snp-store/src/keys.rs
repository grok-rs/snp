//! Content-addressed repository keys, repository identity and markers (§5.2–§5.3, ADR-032, D-DATA-08).
//!
//! pre-commit keys its `repos` table by `(db_repo_name(repo, deps), rev)` (`store.py:125-171`) and finds the clone
//! through a SQLite row. snp derives a 128-bit blake3 key from the same inputs (with the canonical absolute path in
//! place of a relative local `repo:` string, D-DATA-08) and names the store directory after it, so a lookup is one
//! `stat` and nothing ever needs repair (D-CRASH-17).
//!
//! Owner: WP-22.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::profile::LOCAL_TEMPLATE_VERSION;

/// Domain separation of remote repository keys (ARCHITECTURE §5.2).
const REMOTE_DOMAIN: &[u8] = b"snp.repo.v1\0";
/// Domain separation of `repo: local` template keys.
const LOCAL_DOMAIN: &[u8] = b"snp.local.v1\0";
/// Domain separation of `keys-v1` message-parity markers.
const MARKER_DOMAIN: &[u8] = b"snp.keys.v1\0";
/// Domain separation of `configs-v1` entries.
const CONFIG_DOMAIN: &[u8] = b"snp.config.v1\0";

/// The key of a materialised repository (`repos-v1/<hex>`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RepoKey(pub [u8; 16]);

impl RepoKey {
    /// `hex(32)`, the on-disk name (lower-case).
    ///
    /// Replaces the `repo<8 random chars>` directory names of `tempfile.mkdtemp(prefix='repo')` (`store.py:162`):
    /// snp's names are derived, so two processes agree on the directory without a database.
    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    /// Parses a 32-character lower-case hex name back into a key (directory listings).
    pub(crate) fn from_hex(s: &[u8]) -> Option<Self> {
        if s.len() != 32 {
            return None;
        }
        let mut out = [0_u8; 16];
        for (i, slot) in out.iter_mut().enumerate() {
            let hi = s.get(i.checked_mul(2)?).copied().and_then(hex_digit)?;
            let lo = s
                .get(i.checked_mul(2)?.checked_add(1)?)
                .copied()
                .and_then(hex_digit)?;
            *slot = (hi << 4) | lo;
        }
        Some(Self(out))
    }
}

/// The value of one lower-case hex digit.
const fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b.wrapping_sub(b'0')),
        b'a'..=b'f' => Some(b.wrapping_sub(b'a').wrapping_add(10)),
        _ => None,
    }
}

/// Lower-case hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::new();
    for &b in bytes {
        out.push(char::from(
            DIGITS.get(usize::from(b >> 4)).copied().unwrap_or(b'0'),
        ));
        out.push(char::from(
            DIGITS.get(usize::from(b & 0x0f)).copied().unwrap_or(b'0'),
        ));
    }
    out
}

/// What a `repo:` value names.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RepoId {
    /// A URL (or anything that is not an existing local directory), verbatim.
    Url(Arc<str>),
    /// An existing local directory: its canonical absolute path (D-DATA-08).
    LocalPath(PathBuf),
}

impl RepoId {
    /// The single derivation used by snp-repo (resolution) and snp-cmd (gc marking): `repo` names an existing local
    /// directory (relative values resolved against `project_root` = `ConfigSource::project_root` at run time, the
    /// recorded project root of a tracked config at gc time) ⇒ `LocalPath(realpath)`, else `Url(repo verbatim)`
    /// (ADR-070).
    ///
    /// pre-commit's `_new_repo` keys the clone by the `repo:` string verbatim and `clone_strategy` adds the remote
    /// as `os.path.abspath(repo)` when `os.path.isdir(repo)` (`store.py:177-182`, evaluated in the project root
    /// after `_adjust_args_and_chdir`). snp keeps that test (`isdir` follows symlinks; an empty string is never a
    /// directory) but keys local directories by their `realpath`, so `repo: ../tools` in two projects never shares
    /// one cache entry (D-DATA-08). A directory whose `realpath` cannot be computed is keyed as a URL.
    pub fn from_config(repo: &str, project_root: &Path) -> Self {
        let url = || Self::Url(Arc::from(repo));
        if repo.is_empty() || repo.contains('\0') {
            return url();
        }
        let given = Path::new(repo);
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            project_root.join(given)
        };
        if !std::fs::metadata(&joined).is_ok_and(|m| m.is_dir()) {
            return url();
        }
        match snp_os::realpath::realpath_nonstrict(&joined, project_root) {
            Ok(real) => Self::LocalPath(real),
            Err(_) => url(),
        }
    }

    /// The `repo_id` hashed into the key: the URL bytes, or `abs:` + the canonical path bytes.
    pub(crate) fn key_bytes(&self) -> Vec<u8> {
        match self {
            Self::Url(u) => u.as_bytes().to_vec(),
            Self::LocalPath(p) => {
                let mut v = b"abs:".to_vec();
                v.extend_from_slice(p.as_os_str().as_encoded_bytes());
                v
            }
        }
    }
}

/// `store.db_repo_name`: `repo` or `repo:dep1,dep2` (deps in config order, no sorting); local repos use
/// `local[:deps]`.
///
/// `Store.db_repo_name` (`store.py:125-130`): `f'{repo}:{",".join(deps)}'` when `deps` is non-empty. It is
/// order-sensitive (T10-STO-10) and is what `[INFO] Initializing environment for {…}.` prints.
pub fn db_repo_name(repo: &str, deps: &[Arc<str>]) -> String {
    if deps.is_empty() {
        return repo.to_owned();
    }
    let mut out = String::from(repo);
    out.push(':');
    for (i, d) in deps.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(d);
    }
    out
}

/// A shared toolchain an env links to (store-owned reference format; `sweep` reads only these, never
/// `InstallState`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolRef {
    /// The tool (`node`, `go`, `uv`, …).
    pub tool: Arc<str>,
    /// Its version.
    pub version: Arc<str>,
}

/// A tracked config and the project root its relative `repo:` paths resolve against (configs-v1 records both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackedConfig {
    /// The config's realpath.
    pub path: PathBuf,
    /// Its project root.
    pub project_root: PathBuf,
}

/// Remote clone or the `repo: local` template.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum RepoKind {
    /// A clone of `id` at `rev`.
    Remote {
        /// The repository identity.
        id: RepoId,
        /// The revision.
        rev: Arc<str>,
    },
    /// The local-hooks template (`LOCAL_TEMPLATE_VERSION`).
    Local,
}

/// What `.snp-repo.json` records.
#[derive(Clone, Debug)]
pub struct RepoMeta {
    /// Remote or local.
    pub kind: RepoKind,
    /// `additional_dependencies` in config order.
    pub deps: Arc<[Arc<str>]>,
}

impl RepoMeta {
    /// The key this metadata derives (`Store::remote_key` / `Store::local_key`).
    pub(crate) fn key(&self) -> RepoKey {
        match &self.kind {
            RepoKind::Remote { id, rev } => remote_key(id, rev, &self.deps),
            RepoKind::Local => local_key(&self.deps),
        }
    }
}

/// A materialised repository.
#[derive(Clone, Debug)]
pub struct RepoHandle {
    /// Its key.
    pub key: RepoKey,
    /// The checkout (pre-commit's `prefix`).
    pub src: PathBuf,
    /// The environments directory.
    pub envs: PathBuf,
    /// The recorded metadata.
    pub meta: RepoMeta,
}

/// Message-parity markers (ADR-032): `Initializing environment for {db_repo_name}.` and the virtual install
/// triplet.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum MarkerKey {
    /// pre-commit would have cloned `db_repo_name` at `rev` (keys-v1).
    Initialized {
        /// `store.db_repo_name`.
        db_repo_name: Arc<str>,
        /// The revision.
        rev: Arc<str>,
    },
    /// pre-commit would have installed this environment.
    Installed {
        /// The install key digest.
        install_key: [u8; 16],
    },
}

impl MarkerKey {
    /// The marker file's contents: `init\0<db_repo_name>\0<rev>` or `inst\0<hex32>` (so `retain_markers` can hand
    /// the key back to the caller).
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            Self::Initialized { db_repo_name, rev } => {
                let mut v = b"init\0".to_vec();
                v.extend_from_slice(db_repo_name.as_bytes());
                v.push(0);
                v.extend_from_slice(rev.as_bytes());
                v
            }
            Self::Installed { install_key } => {
                let mut v = b"inst\0".to_vec();
                v.extend_from_slice(hex(install_key).as_bytes());
                v
            }
        }
    }

    /// Inverse of [`MarkerKey::encode`]; `None` for anything else (a foreign or truncated file).
    pub(crate) fn decode(b: &[u8]) -> Option<Self> {
        if let Some(rest) = b.strip_prefix(b"init\0") {
            let nul = memchr::memchr(0, rest)?;
            let name = std::str::from_utf8(rest.get(..nul)?).ok()?;
            let rev = std::str::from_utf8(rest.get(nul.checked_add(1)?..)?).ok()?;
            if rev.contains('\0') {
                return None;
            }
            return Some(Self::Initialized {
                db_repo_name: Arc::from(name),
                rev: Arc::from(rev),
            });
        }
        let digest = b.strip_prefix(b"inst\0")?;
        RepoKey::from_hex(digest).map(|k| Self::Installed { install_key: k.0 })
    }

    /// The marker's file name in `keys-v1`.
    pub(crate) fn file_name(&self) -> String {
        let mut h = blake3::Hasher::new();
        h.update(MARKER_DOMAIN);
        match self {
            Self::Initialized { db_repo_name, rev } => {
                h.update(b"init\0");
                lp(&mut h, db_repo_name.as_bytes());
                lp(&mut h, rev.as_bytes());
            }
            Self::Installed { install_key } => {
                h.update(b"inst\0");
                lp(&mut h, install_key);
            }
        }
        hex(digest16(&h).as_slice())
    }
}

/// Appends `len(bytes)` (u64 little endian) and `bytes`: unambiguous concatenation.
fn lp(h: &mut blake3::Hasher, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    h.update(&len.to_le_bytes());
    h.update(bytes);
}

/// The first 16 bytes of the digest.
fn digest16(h: &blake3::Hasher) -> [u8; 16] {
    let full = h.finalize();
    let mut out = [0_u8; 16];
    for (o, b) in out.iter_mut().zip(full.as_bytes()) {
        *o = *b;
    }
    out
}

/// `json(deps)`: the dependency list as a JSON array of strings (config order).
fn deps_json(deps: &[Arc<str>]) -> Vec<u8> {
    serde_json::to_vec(deps).unwrap_or_default()
}

/// `blake3("snp.repo.v1\0" ‖ lp(repo_id) ‖ lp(rev) ‖ json(deps))[..16]` (ARCHITECTURE §5.2).
pub(crate) fn remote_key(id: &RepoId, rev: &str, deps: &[Arc<str>]) -> RepoKey {
    let mut h = blake3::Hasher::new();
    h.update(REMOTE_DOMAIN);
    lp(&mut h, &id.key_bytes());
    lp(&mut h, rev.as_bytes());
    h.update(&deps_json(deps));
    RepoKey(digest16(&h))
}

/// `blake3("snp.local.v1\0" ‖ lp(LOCAL_TEMPLATE_VERSION) ‖ json(deps))[..16]`: pre-commit's
/// `_new_repo('local', C.LOCAL_REPO_VERSION, deps, …)` (`store.py:212-215`) with snp's template version.
pub(crate) fn local_key(deps: &[Arc<str>]) -> RepoKey {
    let mut h = blake3::Hasher::new();
    h.update(LOCAL_DOMAIN);
    lp(&mut h, LOCAL_TEMPLATE_VERSION.to_string().as_bytes());
    h.update(&deps_json(deps));
    RepoKey(digest16(&h))
}

/// The `configs-v1` entry name of a tracked `(config realpath, project root)` pair.
pub(crate) fn config_entry_name(cfg: &Path, project_root: &Path) -> String {
    let mut h = blake3::Hasher::new();
    h.update(CONFIG_DOMAIN);
    lp(&mut h, cfg.as_os_str().as_encoded_bytes());
    lp(&mut h, project_root.as_os_str().as_encoded_bytes());
    hex(&digest16(&h))
}

/// `hex16(blake3(version))`: the hashed environment-directory suffix (ADR-009).
pub(crate) fn version_hash16(version: &str) -> String {
    let full = blake3::hash(version.as_bytes());
    hex(full.as_bytes().get(..8).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(xs: &[&str]) -> Vec<Arc<str>> {
        xs.iter().map(|s| Arc::from(*s)).collect()
    }

    #[test]
    fn hex_round_trip() {
        let k = RepoKey([
            0x00, 0x01, 0x7f, 0x80, 0xff, 0x10, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56, 0x78, 0x9a,
            0xbc, 0xde,
        ]);
        let h = k.hex();
        assert_eq!(h, "00017f80ff10abcdef123456789abcde");
        assert_eq!(RepoKey::from_hex(h.as_bytes()), Some(k));
        assert_eq!(RepoKey::from_hex(b"00017F80ff10abcdef123456789abcde"), None);
        assert_eq!(RepoKey::from_hex(b"0001"), None);
        assert_eq!(RepoKey::from_hex(b"g0017f80ff10abcdef123456789abcde"), None);
    }

    #[test]
    fn keys_are_domain_separated_and_order_sensitive() {
        let url = RepoId::Url(Arc::from("https://github.com/pre-commit/pre-commit-hooks"));
        let ab = remote_key(&url, "v6.0.0", &deps(&["a", "b"]));
        let ba = remote_key(&url, "v6.0.0", &deps(&["b", "a"]));
        let none = remote_key(&url, "v6.0.0", &[]);
        let older = remote_key(&url, "v5.0.0", &[]);
        assert_ne!(ab, ba);
        assert_ne!(ab, none);
        assert_ne!(none, older);
        // Concatenation is unambiguous: ("ab", "c") != ("a", "bc").
        assert_ne!(
            remote_key(&RepoId::Url(Arc::from("ab")), "c", &[]),
            remote_key(&RepoId::Url(Arc::from("a")), "bc", &[])
        );
        // A local path never collides with the URL spelling of the same string.
        assert_ne!(
            remote_key(&RepoId::LocalPath(PathBuf::from("/x")), "r", &[]),
            remote_key(&RepoId::Url(Arc::from("/x")), "r", &[])
        );
        assert_ne!(local_key(&[]), local_key(&deps(&["six"])));
        assert_eq!(local_key(&deps(&["six"])), local_key(&deps(&["six"])));
    }

    #[test]
    fn db_repo_name_keeps_dependency_order() {
        assert_eq!(db_repo_name("repo", &[]), "repo");
        assert_eq!(db_repo_name("repo", &deps(&["b", "a", "c"])), "repo:b,a,c");
        assert_eq!(db_repo_name("local", &deps(&["six"])), "local:six");
    }

    #[test]
    fn marker_encoding_round_trips() {
        let m = MarkerKey::Initialized {
            db_repo_name: Arc::from("https://x/y:a,b"),
            rev: Arc::from("v1"),
        };
        let back = MarkerKey::decode(&m.encode()).unwrap();
        assert_eq!(back.file_name(), m.file_name());
        let i = MarkerKey::Installed {
            install_key: [7; 16],
        };
        let back = MarkerKey::decode(&i.encode()).unwrap();
        assert_eq!(back.file_name(), i.file_name());
        assert_ne!(m.file_name(), i.file_name());
        assert!(MarkerKey::decode(b"").is_none());
        assert!(MarkerKey::decode(b"init\0no-nul").is_none());
        assert!(MarkerKey::decode(b"inst\0zz").is_none());
        assert!(MarkerKey::decode(b"init\0a\0b\0c").is_none());
    }

    #[test]
    fn from_config_resolves_against_the_project_root() {
        let d = tempfile::tempdir().unwrap();
        let root = snp_os::realpath::realpath_nonstrict(d.path(), Path::new("/")).unwrap();
        std::fs::create_dir_all(root.join("proj/sub")).unwrap();
        std::fs::create_dir_all(root.join("tools")).unwrap();
        let proj = root.join("proj/sub");
        match RepoId::from_config("../../tools", &proj) {
            RepoId::LocalPath(p) => assert_eq!(p, root.join("tools")),
            RepoId::Url(u) => panic!("expected a local path, got {u}"),
        }
        assert!(matches!(
            RepoId::from_config("https://github.com/x/y", &proj),
            RepoId::Url(u) if &*u == "https://github.com/x/y"
        ));
        assert!(matches!(RepoId::from_config("", &proj), RepoId::Url(_)));
        assert!(matches!(
            RepoId::from_config("../missing", &proj),
            RepoId::Url(u) if &*u == "../missing"
        ));
    }
}
