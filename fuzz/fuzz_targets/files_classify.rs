//! Fuzz target `files_classify`: tiered classification.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input describes a small tree (`name\0content\0` records written into a per-process scratch dir, plus names
//! that do not exist), trusted git modes and a few filters built from the files' own tags. Checked: no panic; the
//! name/stat/head tiers agree with full identification for every filter (partial-tier outcome == full-tag outcome,
//! ADR-041); `Classifier::for_hook` keeps table order and only ids of the table; `relative_to` is boundary-aware
//! and never returns a slice that is not a suffix of the path.
//!
//! Owner: WP-22.
#![no_main]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use libfuzzer_sys::fuzz_target;
use snp_core::cancel::CancelToken;
use snp_core::path::RepoPath;
use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
use snp_proc::pool::JobPool;
use snp_pyre::{Options, RegexCache};

const FULL: Needs = Needs {
    stat: true,
    encoding: true,
    shebang: true,
};

/// One scratch dir per fuzz process.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d =
            std::env::temp_dir().join(format!("snp-fuzz-files-classify-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("scratch dir");
        d
    })
}

/// A name usable as one file in the scratch dir (no separators, not `.`/`..`).
fn on_disk_name(raw: &[u8]) -> Option<Vec<u8>> {
    let name: Vec<u8> = raw
        .iter()
        .map(|&b| if b == b'/' || b == b'\\' { b'_' } else { b })
        .take(64)
        .collect();
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    Some(name)
}

#[cfg(unix)]
fn os(b: &[u8]) -> &std::ffi::OsStr {
    std::os::unix::ffi::OsStrExt::from_bytes(b)
}

#[cfg(not(unix))]
fn os(b: &[u8]) -> std::ffi::OsString {
    String::from_utf8_lossy(b).into_owned().into()
}

fuzz_target!(|data: &[u8]| {
    let Some((&ctl, rest)) = data.split_first() else {
        return;
    };
    let dir = scratch();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    // Records: name \0 content \0 (at most 8 files written, contents capped at 4 KiB).
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut it = rest.split(|&b| b == 0);
    while let (Some(name), content) = (it.next(), it.next()) {
        if names.len() >= 16 {
            break;
        }
        let Some(n) = on_disk_name(name) else {
            continue;
        };
        if names.len() < 8
            && let Some(c) = content
        {
            let p = dir.join(os(&n));
            if std::fs::write(&p, c.get(..c.len().min(4096)).unwrap_or_default()).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = if ctl & 1 == 1 { 0o755 } else { 0o644 };
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode));
                }
            }
        }
        names.push(n);
    }
    // The table: the names (duplicated when ctl asks) as a -z buffer.
    let mut buf = Vec::new();
    let mut spans = Vec::new();
    for n in names.iter().chain(if ctl & 2 == 2 {
        names.iter().take(2)
    } else {
        names.iter().take(0)
    }) {
        let s = u32::try_from(buf.len()).unwrap_or(u32::MAX);
        buf.extend_from_slice(n);
        spans.push((s, u32::try_from(buf.len()).unwrap_or(u32::MAX)));
        buf.push(0);
    }
    let mut table = FileTable::from_zbuf(buf, spans, dir);
    let len = table.len();
    if ctl & 4 == 4 {
        let modes = [
            0o100_644, 0o100_755, 0o120_000, 0o160_000, 0o040_000, 0o777, 0,
        ];
        let entries: Vec<(&[u8], u32)> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.as_slice(),
                    modes[(i + usize::from(ctl >> 4)) % modes.len()],
                )
            })
            .collect();
        table.set_trusted_modes(entries);
    }
    if ctl & 8 == 8 {
        table.retain_lexists();
    }
    assert!(table.len() <= len);
    // Full tags (fresh table each time: no partial cell is reused).
    let full_tags: Vec<_> = (0..table.len())
        .map(|i| table.tags(FileId(u32::try_from(i).unwrap_or(u32::MAX)), FULL))
        .map(|o| match o {
            TagOutcome::Tags(t) => Some(t),
            _ => None,
        })
        .collect();
    let cache = RegexCache::new();
    let pool = JobPool::new(std::num::NonZeroUsize::MIN, CancelToken::new());
    let all = cache.get("", Options::CONFIG).expect("''");
    let none = cache.get("^$", Options::CONFIG).expect("'^$'");
    // Filters built from the files' own tags: types = A, types_or = B ∩ C, exclude = D \ A.
    let pick = |k: usize| {
        full_tags
            .iter()
            .flatten()
            .nth(k)
            .copied()
            .unwrap_or_default()
    };
    let (a, b, c, d) = (pick(0), pick(1), pick(2), pick(3));
    for (types, types_or, exclude) in [
        (a, Default::default(), Default::default()),
        (Default::default(), b.intersection(c), d.difference(a)),
        (a.intersection(b), b, Default::default()),
        (Default::default(), Default::default(), Default::default()),
    ] {
        let prefix = names
            .first()
            .and_then(|n| RepoPath::new(n.get(..n.len().min(3)).unwrap_or_default()));
        let f = HookFilter::compile("", "^$", types, types_or, exclude, prefix, &cache)
            .expect("filter");
        // The tiers decide exactly like full tags, on an independent table with the same tree.
        let partial = FileTable::from_zbuf(table_bytes(&table), table_spans(&table), dir);
        for (i, ft) in full_tags.iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap_or(u32::MAX));
            let Some(ft) = ft else { continue };
            if let TagOutcome::Tags(pt) = partial.tags(id, f.needs) {
                // Trusted modes are not copied to `partial`; compare only lstat-based tables.
                if ctl & 4 == 0 {
                    assert_eq!(
                        pt.matches_types(&f.types, &f.types_or, &f.exclude_types),
                        ft.matches_types(&f.types, &f.types_or, &f.exclude_types)
                    );
                }
            }
        }
        let shared = Arc::new(FileTable::from_zbuf(
            table_bytes(&table),
            table_spans(&table),
            dir,
        ));
        if let Ok(c) = Classifier::new(shared, &all, &none, &pool)
            && let Ok(ids) = c.for_hook(&f, &pool)
        {
            assert!(ids.windows(2).all(|w| w[0] < w[1]));
            for id in ids {
                assert!(usize::try_from(id.0).is_ok_and(|i| i < c.table.len()));
                if let Some(p) = &f.prefix {
                    let rel = c.table.relative_to(id, p).expect("prefix filter");
                    assert!(c.table.bytes(id).ends_with(&rel) || cfg!(windows));
                }
            }
        }
    }
});

fn table_bytes(t: &FileTable) -> Vec<u8> {
    let mut buf = Vec::new();
    for i in 0..t.len() {
        buf.extend_from_slice(t.bytes(FileId(u32::try_from(i).unwrap_or(u32::MAX))));
        buf.push(0);
    }
    buf
}

fn table_spans(t: &FileTable) -> Vec<(u32, u32)> {
    let mut spans = Vec::new();
    let mut at = 0_u32;
    for i in 0..t.len() {
        let n =
            u32::try_from(t.bytes(FileId(u32::try_from(i).unwrap_or(u32::MAX))).len()).unwrap_or(0);
        spans.push((at, at + n));
        at += n + 1;
    }
    spans
}
