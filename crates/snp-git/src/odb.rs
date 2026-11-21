//! Loose objects, read strictly, and the gates under which git reads an object from its loose copy (ADR-153). No
//! spawn.
//!
//! A loose object is `<objects>/<first two hex digits>/<the rest>` (64 hex digits in a SHA-256 repository), a zlib
//! stream of `<type> <decimal size>\0<body>`. [`read_loose`] accepts exactly that: the canonical header of the
//! expected type, a body of exactly the stated size, and the stream's end exactly at the file's end (git reports
//! "garbage at end of loose object" otherwise), and, in a SHA-1 repository, the whole object hashing to its name
//! (git's `parse_object` checks it). Anything else is a refusal, never an error of the run.
//!
//! [`loose_only`] is the gate under which git reads an object of the repository from that loose copy and from
//! nothing else: no pack or multi-pack index in `objects/pack` (git looks in packs before loose objects), no
//! alternates (whose packs come first too), no replace refs (`parse_object` reads the replacement), and none of the
//! variables that move the object directory or the replace refs.
//!
//! Owner: WP-18.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use snp_core::env::Env;

/// The largest compressed loose object [`read_loose`] reads.
const MAX_COMPRESSED: u64 = 16 << 20;

/// Why a loose object was not read (every refusal means "no proof").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OdbRefusal {
    /// The object id is neither 20 nor 32 bytes.
    HashSize,
    /// The file is missing or unreadable.
    Io,
    /// The file is larger than the reader takes, or a reservation failed.
    TooLarge,
    /// Not a complete zlib stream ending at the file's end.
    Stream,
    /// The header is not `<type> <size>\0` for the expected type, or the body has another size.
    Header,
    /// The object does not hash to its name.
    Hash,
    /// The hash cannot be checked (a SHA-256 repository).
    Unchecked,
}

/// Lowercase hex of `oid`.
pub fn hex(oid: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::new();
    if s.try_reserve(oid.len().saturating_mul(2)).is_err() {
        return s;
    }
    for &b in oid {
        for nibble in [b >> 4, b & 0xf] {
            if let Some(&d) = DIGITS.get(usize::from(nibble)) {
                s.push(char::from(d));
            }
        }
    }
    s
}

/// The loose path of `oid` under `objects`; `None` for an id that is neither 20 nor 32 bytes.
pub fn loose_path(objects: &Path, oid: &[u8]) -> Option<PathBuf> {
    if !matches!(oid.len(), 20 | 32) {
        return None;
    }
    let h = hex(oid);
    let (dir, rest) = (h.get(..2)?, h.get(2..)?);
    Some(objects.join(dir).join(rest))
}

/// Whether `oid` has a loose copy under `objects`: git's own test (`stat_loose_object`, an `lstat` that succeeds).
pub fn loose_exists(objects: &Path, oid: &[u8]) -> bool {
    loose_path(objects, oid).is_some_and(|p| std::fs::symlink_metadata(p).is_ok())
}

/// The body of the loose object `oid` under `objects`, which must be of type `kind` (`b"tree"`, `b"blob"`, ...),
/// read strictly (see the module documentation).
pub fn read_loose(objects: &Path, oid: &[u8], kind: &[u8]) -> Result<Vec<u8>, OdbRefusal> {
    let path = loose_path(objects, oid).ok_or(OdbRefusal::HashSize)?;
    if oid.len() != 20 {
        return Err(OdbRefusal::Unchecked);
    }
    let mut raw: Vec<u8> = Vec::new();
    let f = std::fs::File::open(&path).map_err(|_| OdbRefusal::Io)?;
    f.take(MAX_COMPRESSED.saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(|_| OdbRefusal::Io)?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_COMPRESSED {
        return Err(OdbRefusal::TooLarge);
    }
    let object = inflate(&raw)?;
    let body = split_header(&object, kind)?;
    if crate::sha1::digest(&[&object]) != oid {
        return Err(OdbRefusal::Hash);
    }
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve_exact(body.len())
        .map_err(|_| OdbRefusal::TooLarge)?;
    out.extend_from_slice(body);
    Ok(out)
}

/// The whole zlib stream `raw` inflated; it must end exactly at the end of `raw`.
fn inflate(raw: &[u8]) -> Result<Vec<u8>, OdbRefusal> {
    use flate2::{Decompress, FlushDecompress, Status};
    // inflated objects are bounded by the same order of magnitude: zlib's best ratio is about 1032:1
    let cap = raw.len().saturating_mul(1032).saturating_add(64);
    let mut d = Decompress::new(true);
    let mut out: Vec<u8> = Vec::new();
    out.try_reserve(raw.len().saturating_mul(4).clamp(64, 1 << 20))
        .map_err(|_| OdbRefusal::TooLarge)?;
    loop {
        if out.len() == out.capacity() {
            if out.len() >= cap {
                return Err(OdbRefusal::TooLarge);
            }
            out.try_reserve(out.len().max(64))
                .map_err(|_| OdbRefusal::TooLarge)?;
        }
        let consumed = usize::try_from(d.total_in()).map_err(|_| OdbRefusal::Stream)?;
        let input = raw.get(consumed..).ok_or(OdbRefusal::Stream)?;
        let (in0, out0) = (d.total_in(), d.total_out());
        let status = d
            .decompress_vec(input, &mut out, FlushDecompress::None)
            .map_err(|_| OdbRefusal::Stream)?;
        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                let stuck = d.total_in() == in0 && d.total_out() == out0;
                if stuck && out.len() < out.capacity() {
                    // no progress with room to write: the input ended inside the stream
                    return Err(OdbRefusal::Stream);
                }
            }
        }
    }
    if usize::try_from(d.total_in()).ok() != Some(raw.len()) {
        return Err(OdbRefusal::Stream);
    }
    Ok(out)
}

/// The body after a canonical `<kind> <size>\0` header whose size is the body's length.
fn split_header<'a>(object: &'a [u8], kind: &[u8]) -> Result<&'a [u8], OdbRefusal> {
    let rest = object.strip_prefix(kind).ok_or(OdbRefusal::Header)?;
    let rest = rest.strip_prefix(b" ").ok_or(OdbRefusal::Header)?;
    let nul = memchr::memchr(0, rest).ok_or(OdbRefusal::Header)?;
    let (digits, body) = rest.split_at_checked(nul).ok_or(OdbRefusal::Header)?;
    let body = body.get(1..).ok_or(OdbRefusal::Header)?;
    let canonical = !digits.is_empty()
        && digits.iter().all(u8::is_ascii_digit)
        && (digits.len() == 1 || digits.first() != Some(&b'0'));
    if !canonical {
        return Err(OdbRefusal::Header);
    }
    let mut size: usize = 0;
    for &d in digits {
        size = size
            .checked_mul(10)
            .and_then(|s| s.checked_add(usize::from(d.wrapping_sub(b'0'))))
            .ok_or(OdbRefusal::Header)?;
    }
    if size != body.len() {
        return Err(OdbRefusal::Header);
    }
    Ok(body)
}

/// Whether git reads the objects of the repository whose common directory is `common_dir` from their loose copies
/// only (see the module documentation): `objects/pack` is missing or empty, no `objects/info/alternates`, no
/// `refs/replace` directory, no `refs/replace/` line in `packed-refs` (which must be readable and at most 1 MiB), no
/// `reftable` directory, and none of `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES` and
/// `GIT_REPLACE_REF_BASE` set.
pub fn loose_only(common_dir: &Path, env: &Env) -> bool {
    let moved = [
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_REPLACE_REF_BASE",
    ]
    .iter()
    .any(|k| env.get(k).is_some());
    if moved {
        return false;
    }
    let absent = |p: PathBuf| {
        std::fs::symlink_metadata(p).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    };
    let objects = common_dir.join("objects");
    let no_packs = match std::fs::read_dir(objects.join("pack")) {
        Ok(mut rd) => rd.next().is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    };
    if !no_packs
        || !absent(objects.join("info").join("alternates"))
        || !absent(common_dir.join("refs").join("replace"))
        || !absent(common_dir.join("reftable"))
    {
        return false;
    }
    match snp_os::fs::read_capped(&common_dir.join("packed-refs"), (1 << 20) + 1) {
        Ok(b) => b.len() <= 1 << 20 && memchr::memmem::find(&b, b"refs/replace/").is_none(),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn deflate(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    fn store(objects: &Path, object: &[u8]) -> [u8; 20] {
        let oid = crate::sha1::digest(&[object]);
        let p = loose_path(objects, &oid).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, deflate(object)).unwrap();
        oid
    }

    #[test]
    fn reads_a_strict_loose_object() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path();
        let oid = store(objects, b"tree 3\0abc");
        assert!(loose_exists(objects, &oid));
        assert_eq!(read_loose(objects, &oid, b"tree").unwrap(), b"abc");
        assert_eq!(
            read_loose(objects, &oid, b"blob").unwrap_err(),
            OdbRefusal::Header
        );
        let h = hex(&oid);
        let (dir, file) = h.split_at_checked(2).unwrap();
        assert_eq!(
            loose_path(objects, &oid).unwrap(),
            objects.join(dir).join(file)
        );
        assert_eq!(
            read_loose(objects, &[0; 20], b"tree").unwrap_err(),
            OdbRefusal::Io
        );
        assert_eq!(
            read_loose(objects, &[0; 32], b"tree").unwrap_err(),
            OdbRefusal::Unchecked
        );
        assert_eq!(
            read_loose(objects, &[0; 21], b"tree").unwrap_err(),
            OdbRefusal::HashSize
        );
    }

    #[test]
    fn refusals() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path();
        let put = |name: [u8; 20], bytes: &[u8]| {
            let p = loose_path(objects, &name).unwrap();
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            name
        };
        // garbage after the stream, a stream cut short, not zlib at all
        let good = b"tree 3\0abc";
        let name = crate::sha1::digest(&[good]);
        let mut z = deflate(good);
        z.push(0);
        put(name, &z);
        assert_eq!(
            read_loose(objects, &name, b"tree").unwrap_err(),
            OdbRefusal::Stream
        );
        let z = deflate(good);
        put(name, &z[..z.len() - 3]);
        assert_eq!(
            read_loose(objects, &name, b"tree").unwrap_err(),
            OdbRefusal::Stream
        );
        put(name, b"not zlib");
        assert_eq!(
            read_loose(objects, &name, b"tree").unwrap_err(),
            OdbRefusal::Stream
        );
        // the right bytes under another name: git's parse_object refuses the hash
        put([7; 20], &deflate(good));
        assert_eq!(
            read_loose(objects, &[7; 20], b"tree").unwrap_err(),
            OdbRefusal::Hash
        );
        // non-canonical or wrong sizes
        for bad in [
            &b"tree 4\0abc"[..],
            b"tree 03\0abc",
            b"tree \0abc",
            b"tree 3abc",
            b"tree+3\0abc",
        ] {
            let n = store(objects, bad);
            assert_eq!(
                read_loose(objects, &n, b"tree").unwrap_err(),
                OdbRefusal::Header,
                "{bad:?}"
            );
        }
        let n = store(objects, b"tree 0\0");
        assert_eq!(read_loose(objects, &n, b"tree").unwrap(), b"");
    }

    #[test]
    fn loose_only_gates() {
        let dir = tempfile::tempdir().unwrap();
        let c = dir.path();
        let env = Env::from_pairs([("PATH", "/bin")]);
        assert!(loose_only(c, &env));
        std::fs::create_dir_all(c.join("objects/pack")).unwrap();
        std::fs::create_dir_all(c.join("objects/info")).unwrap();
        std::fs::create_dir_all(c.join("refs/heads")).unwrap();
        assert!(loose_only(c, &env));
        for k in [
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_REPLACE_REF_BASE",
        ] {
            assert!(!loose_only(c, &Env::from_pairs([(k, "x")])), "{k}");
        }
        std::fs::write(
            c.join("packed-refs"),
            "# pack-refs with: peeled\n0000 refs/heads/x\n",
        )
        .unwrap();
        assert!(loose_only(c, &env));
        std::fs::write(c.join("packed-refs"), "0000 refs/replace/abc\n").unwrap();
        assert!(!loose_only(c, &env));
        std::fs::remove_file(c.join("packed-refs")).unwrap();
        for p in ["objects/pack/pack-1.idx", "objects/info/alternates"] {
            std::fs::write(c.join(p), "").unwrap();
            assert!(!loose_only(c, &env), "{p}");
            std::fs::remove_file(c.join(p)).unwrap();
        }
        for d in ["refs/replace", "reftable"] {
            std::fs::create_dir_all(c.join(d)).unwrap();
            assert!(!loose_only(c, &env), "{d}");
            std::fs::remove_dir(c.join(d)).unwrap();
        }
        assert!(loose_only(c, &env));
    }
}
