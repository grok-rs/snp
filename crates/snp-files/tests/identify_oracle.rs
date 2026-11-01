//! snp-files' tags against identify 2.6.20 (`identify.tags_from_path`, the oracle venv) over a generated tree:
//! names, extensions, shebangs (incl. `env -S` and `nix-shell`), exec bits, symlinks, directories, sockets, FIFOs.
//! The tree and the expected tags are `tests/vectors/identify_tree.json` (generator: `gen_identify_tree.py`).
//!
//! Plus the exactness property of the tiers (ADR-041, S13): a filter evaluated with only the tiers its `Needs`
//! name gives the same outcome as the full tag set.
//!
//! Owner: WP-22.
#![cfg(unix)]

#[cfg(test)]
mod oracle {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use file_identify::TagSet;
    use proptest::prelude::*;
    use snp_core::cancel::CancelToken;
    use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
    use snp_proc::pool::JobPool;
    use snp_pyre::{Options, RegexCache};

    const FULL: Needs = Needs {
        stat: true,
        encoding: true,
        shebang: true,
    };

    struct Entry {
        path: Vec<u8>,
        tags: Vec<String>,
    }

    fn unhex(s: &str) -> Vec<u8> {
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < b.len() {
            out.push(u8::from_str_radix(std::str::from_utf8(&b[i..i + 2]).unwrap(), 16).unwrap());
            i += 2;
        }
        out
    }

    /// Materialises the vector tree under a fresh temp dir; returns (dir guard, root, entries, sockets).
    fn tree() -> (
        tempfile::TempDir,
        PathBuf,
        Vec<Entry>,
        Vec<std::os::unix::net::UnixListener>,
    ) {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("vectors/identify_tree.json")).unwrap();
        assert_eq!(json["identify"], "2.6.20");
        let dir = tempfile::Builder::new().prefix("snp-id").tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut entries = Vec::new();
        let mut socks = Vec::new();
        for e in json["entries"].as_array().unwrap() {
            let rel = unhex(e["path"].as_str().unwrap());
            let p = root.join(std::ffi::OsStr::from_bytes_ext(&rel));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            match e["kind"].as_str().unwrap() {
                "file" => {
                    std::fs::write(&p, unhex(e["content"].as_str().unwrap())).unwrap();
                    let mode = u32::try_from(e["mode"].as_u64().unwrap()).unwrap();
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
                }
                "dir" => std::fs::create_dir_all(&p).unwrap(),
                "symlink" => {
                    let target = unhex(e["target"].as_str().unwrap());
                    std::os::unix::fs::symlink(OsString::from_vec(target), &p).unwrap();
                }
                "fifo" => {
                    let mode = u32::try_from(e["mode"].as_u64().unwrap()).unwrap();
                    // mkfifo(1) is POSIX; rustix has no mknodat on Apple targets.
                    let st = std::process::Command::new("mkfifo")
                        .arg(&p)
                        .status()
                        .unwrap();
                    assert!(st.success(), "mkfifo {}: {st}", p.display());
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
                }
                "socket" => socks.push(std::os::unix::net::UnixListener::bind(&p).unwrap()),
                other => panic!("unknown kind {other}"),
            }
            let tags = e["tags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap().to_owned())
                .collect();
            entries.push(Entry { path: rel, tags });
        }
        (dir, root, entries, socks)
    }

    /// `OsStr::from_bytes` without importing the trait at every call site.
    trait FromBytesExt {
        fn from_bytes_ext(b: &[u8]) -> &Self;
    }
    impl FromBytesExt for std::ffi::OsStr {
        fn from_bytes_ext(b: &[u8]) -> &Self {
            std::os::unix::ffi::OsStrExt::from_bytes(b)
        }
    }

    fn table_of(root: &Path, entries: &[Entry]) -> FileTable {
        FileTable::from_args(
            entries
                .iter()
                .map(|e| OsString::from_vec(e.path.clone()))
                .collect(),
            root,
        )
    }

    fn names(t: TagSet) -> Vec<String> {
        t.names().map(str::to_owned).collect()
    }

    #[test]
    fn tags_equal_identify_over_the_generated_tree() {
        let (_guard, root, entries, _socks) = tree();
        let table = table_of(&root, &entries);
        for (i, e) in entries.iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap());
            let TagOutcome::Tags(tags) = table.tags(id, FULL) else {
                panic!("{:?}: not tagged", String::from_utf8_lossy(&e.path))
            };
            assert_eq!(
                names(tags),
                e.tags,
                "{:?}",
                String::from_utf8_lossy(&e.path)
            );
        }
    }

    #[test]
    fn git_z_buffer_gives_the_same_tags() {
        let (_guard, root, entries, _socks) = tree();
        let mut buf = Vec::new();
        let mut spans = Vec::new();
        for e in &entries {
            let s = u32::try_from(buf.len()).unwrap();
            buf.extend_from_slice(&e.path);
            spans.push((s, u32::try_from(buf.len()).unwrap()));
            buf.push(0);
        }
        let table = FileTable::from_zbuf(buf, spans, &root);
        let pool = JobPool::new(std::num::NonZeroUsize::new(4).unwrap(), CancelToken::new());
        let ids: Vec<FileId> = (0..entries.len())
            .map(|i| FileId(u32::try_from(i).unwrap()))
            .collect();
        table.prefetch(&ids, FULL, &pool);
        for (id, e) in ids.iter().zip(&entries) {
            let TagOutcome::Tags(tags) = table.tags(*id, FULL) else {
                panic!()
            };
            assert_eq!(names(tags), e.tags);
        }
    }

    /// Tags filters reference (a mix of type, mode, encoding, name and interpreter tags).
    const POOL: &[&str] = &[
        "file",
        "directory",
        "symlink",
        "socket",
        "executable",
        "non-executable",
        "text",
        "binary",
        "python",
        "python3",
        "shell",
        "bash",
        "sh",
        "ruby",
        "javascript",
        "yaml",
        "json",
        "image",
        "markdown",
        "plain-text",
        "plist",
        "makefile",
    ];

    fn set(picks: &[usize]) -> TagSet {
        TagSet::from_names(picks.iter().map(|&i| POOL[i % POOL.len()])).unwrap()
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

        /// Partial-tier filter outcome == full-tag filter outcome, for every file of the tree.
        #[test]
        fn partial_tiers_decide_like_full_tags(
            types in proptest::collection::vec(0usize..64, 0..3),
            types_or in proptest::collection::vec(0usize..64, 0..3),
            exclude in proptest::collection::vec(0usize..64, 0..3),
        ) {
            let (_guard, root, entries, _socks) = tree();
            let cache = RegexCache::new();
            let f = HookFilter::compile("", "^$", set(&types), set(&types_or), set(&exclude), None, &cache).unwrap();
            // A fresh table per case: no tier is cached by an earlier, larger request.
            let partial = table_of(&root, &entries);
            let full = table_of(&root, &entries);
            for (i, entry) in entries.iter().enumerate() {
                let id = FileId(u32::try_from(i).unwrap());
                let TagOutcome::Tags(all) = full.tags(id, FULL) else { panic!() };
                let expected = all.matches_types(&f.types, &f.types_or, &f.exclude_types);
                let got = match partial.tags(id, f.needs) {
                    TagOutcome::Tags(t) => t.matches_types(&f.types, &f.types_or, &f.exclude_types),
                    other => panic!("{other:?}"),
                };
                prop_assert_eq!(got, expected, "{:?} needs={:?}", String::from_utf8_lossy(&entry.path), f.needs);
            }
        }
    }

    /// A head-tier request after a name-tier and a stat-tier request returns the full set (no partial cell reuse).
    #[test]
    fn higher_tier_after_lower_tier_is_complete() {
        let (_guard, root, entries, _socks) = tree();
        let table = table_of(&root, &entries);
        let stat_only = Needs {
            stat: true,
            ..Needs::default()
        };
        for (i, entry) in entries.iter().enumerate() {
            let id = FileId(u32::try_from(i).unwrap());
            let _name: TagOutcome<'_> = table.tags(id, Needs::default());
            let _stat: TagOutcome<'_> = table.tags(id, stat_only);
            let TagOutcome::Tags(tags) = table.tags(id, FULL) else {
                panic!()
            };
            assert_eq!(names(tags), entry.tags);
        }
    }

    /// The classifier end to end over the tree: default `types: [file]` keeps exactly identify's files.
    #[test]
    fn classifier_default_types_file() {
        let (_guard, root, entries, _socks) = tree();
        let table = Arc::new(table_of(&root, &entries));
        let pool = JobPool::new(std::num::NonZeroUsize::new(2).unwrap(), CancelToken::new());
        let cache = RegexCache::new();
        let all = cache.get("", Options::CONFIG).unwrap();
        let none = cache.get("^$", Options::CONFIG).unwrap();
        let c = Classifier::new(Arc::clone(&table), &all, &none, &pool).unwrap();
        assert_eq!(c.kept.len(), entries.len());
        let f = HookFilter::compile(
            "",
            "^$",
            TagSet::from_names(["file"]).unwrap(),
            TagSet::EMPTY,
            TagSet::EMPTY,
            None,
            &cache,
        )
        .unwrap();
        let got: Vec<usize> = c
            .for_hook(&f, &pool)
            .unwrap()
            .into_iter()
            .map(|id| usize::try_from(id.0).unwrap())
            .collect();
        let want: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.tags.iter().any(|t| t == "file"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(got, want);
    }
}
