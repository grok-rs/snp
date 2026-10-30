//! Fuzz target `net_archive_extract`: safe archive extraction (ADR-077).
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks; and the
//! extraction never creates anything outside its destination: a canary directory next to it stays empty, on
//! failure no destination appears, and on success every extracted symlink that resolves at all resolves (physically,
//! `fs::canonicalize`) inside the destination.
//!
//! Input: byte 0 selects the format (tar stream gzipped by the target, so the tar parser sees the fuzzer's bytes;
//! raw `.tar.gz`; raw `.zip`), byte 1 the options, the rest is the archive.
//!
//! Owner: WP-16.
#![no_main]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use libfuzzer_sys::fuzz_target;
use snp_net::NetError;
use snp_net::archive::{self, Extract};

/// A fresh work directory per input (removed afterwards).
fn workdir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("snp-fuzz-net-{}-{n}", std::process::id()))
}

/// `bytes` as a gzip member of stored (uncompressed) deflate blocks: the fuzz crate has no compressor, and stored
/// blocks keep the fuzzer's bytes visible to the tar parser.
fn gzip_stored(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut rest = bytes;
    loop {
        let n = rest.len().min(0xffff);
        let (block, tail) = rest.split_at(n);
        out.push(u8::from(tail.is_empty()));
        let len = u16::try_from(n).unwrap_or(u16::MAX);
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
        rest = tail;
        if rest.is_empty() {
            break;
        }
    }
    out.extend_from_slice(&crc32(bytes).to_le_bytes());
    let isize = u64::try_from(bytes.len()).unwrap_or(0) & 0xffff_ffff;
    out.extend_from_slice(&u32::try_from(isize).unwrap_or(0).to_le_bytes());
    out
}

/// CRC-32 (IEEE, reflected), bitwise.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The first symlink below `dest` whose physical resolution leaves `dest` (dangling links are skipped).
fn escaping_symlink(dest: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(dest).ok()?;
    let mut stack = vec![(dest.to_path_buf(), 0_usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else {
                continue;
            };
            let p = e.path();
            if t.is_symlink() {
                if let Ok(real) = std::fs::canonicalize(&p)
                    && !real.starts_with(&root)
                {
                    return Some(p);
                }
            } else if t.is_dir() && depth < 256 {
                stack.push((p, depth.saturating_add(1)));
            }
        }
    }
    None
}

/// Removes `dir` even when the extraction left read-only directories behind.
fn cleanup(dir: &Path) {
    if std::fs::remove_dir_all(dir).is_ok() {
        return;
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

fuzz_target!(|data: &[u8]| {
    let [mode, opts, body @ ..] = data else {
        return;
    };
    let work = workdir();
    if std::fs::create_dir_all(work.join("out")).is_err()
        || std::fs::create_dir(work.join("canary")).is_err()
    {
        cleanup(&work);
        return;
    }
    let archive_path = work.join("archive");
    let bytes = match mode % 3 {
        0 => gzip_stored(body),
        _ => body.to_vec(),
    };
    if std::fs::write(&archive_path, &bytes).is_err() {
        cleanup(&work);
        return;
    }
    let o = Extract {
        strip_components: opts % 3,
        allow_symlinks_inside: opts & 0x80 != 0,
        max_total_bytes: 1 << 20,
    };
    let dest = work.join("out").join("dest");
    let result: Result<(), NetError> = if mode % 3 == 2 {
        archive::zip(&archive_path, &dest, &o)
    } else {
        archive::tar_gz(&archive_path, &dest, &o)
    };
    let canary_empty = std::fs::read_dir(work.join("canary")).is_ok_and(|mut d| d.next().is_none());
    let out: Vec<_> = std::fs::read_dir(work.join("out"))
        .map(|d| d.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    let ok = result.is_ok();
    let escaping = if ok { escaping_symlink(&dest) } else { None };
    cleanup(&work);
    assert!(
        escaping.is_none(),
        "a symlink resolves outside the destination: {escaping:?}"
    );
    assert!(canary_empty, "extraction wrote outside its destination");
    if ok {
        assert_eq!(
            out,
            [std::ffi::OsString::from("dest")],
            "unexpected leftovers"
        );
    } else {
        assert!(out.is_empty(), "a failed extraction left {out:?}");
    }
});
