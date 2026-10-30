//! Behaviour of the classifier beyond the catalog rows: zero content reads for the default filter (syscall
//! counter), the filter laws (order, duplicates, prefix, `by_types`), trusted git modes, pool re-entrancy and the
//! Windows matching view.
//!
//! Owner: WP-22.

#[cfg(test)]
mod classify {
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::Arc;

    use file_identify::TagSet;
    use snp_core::cancel::CancelToken;
    use snp_core::path::RepoPath;
    use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
    use snp_proc::pool::JobPool;
    use snp_pyre::{Options, RegexCache};

    fn pool(n: usize) -> Arc<JobPool> {
        JobPool::new(std::num::NonZeroUsize::new(n).unwrap(), CancelToken::new())
    }

    fn tags(names: &[&str]) -> TagSet {
        TagSet::from_names(names).unwrap()
    }

    fn filter(files: &str, exclude: &str, types: &[&str], prefix: Option<&[u8]>, cache: &RegexCache) -> HookFilter {
        HookFilter::compile(
            files,
            exclude,
            tags(types),
            TagSet::EMPTY,
            TagSet::EMPTY,
            prefix.and_then(RepoPath::new),
            cache,
        )
        .unwrap()
    }

    fn classifier(table: FileTable, cache: &RegexCache, pool: &JobPool) -> Classifier {
        let all = cache.get("", Options::CONFIG).unwrap();
        let none = cache.get("^$", Options::CONFIG).unwrap();
        Classifier::new(Arc::new(table), &all, &none, pool).unwrap()
    }

    fn views(c: &Classifier, ids: &[FileId]) -> Vec<String> {
        ids.iter().map(|&id| c.table.match_view(id).to_owned()).collect()
    }

    /// Writes `n` unknown-name files (identify would read every one of them) under `root`.
    fn unknown_names(root: &Path, n: usize) -> Vec<OsString> {
        (0..n)
            .map(|i| {
                let name = format!("data{i}.unknownext");
                std::fs::write(root.join(&name), b"payload\n").unwrap();
                OsString::from(name)
            })
            .collect()
    }

    /// `read` syscalls of this process so far (Linux `/proc/self/io`).
    #[cfg(target_os = "linux")]
    fn syscr() -> u64 {
        let io = std::fs::read_to_string("/proc/self/io").unwrap();
        io.lines()
            .find_map(|l| l.strip_prefix("syscr: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// Child half of `default_types_file_reads_no_contents_syscall_counter`: prints the `read(2)` counts of a
    /// `types: [file]` classification and of a `types: [text]` one over 200 unknown-name files.
    #[cfg(target_os = "linux")]
    #[test]
    fn io_counter_child() {
        let env = snp_core::env::Env::capture_process();
        let Some(dir) = env.get("SNP_FILES_IO_CHILD") else {
            return;
        };
        let root = Path::new(dir).to_path_buf();
        let names = unknown_names(&root, 200);
        let cache = RegexCache::new();
        let pool = pool(1);
        let f_file = filter("", "^$", &["file"], None, &cache);
        let f_text = filter("", "^$", &["text"], None, &cache);
        let c = classifier(FileTable::from_args(names, &root), &cache, &pool);
        let before = syscr();
        let got = c.for_hook(&f_file, &pool).unwrap();
        let file_reads = syscr() - before;
        assert_eq!(got.len(), 200);
        let before = syscr();
        let got = c.for_hook(&f_text, &pool).unwrap();
        let text_reads = syscr() - before;
        assert_eq!(got.len(), 200);
        println!("SNP-IO file={file_reads} text={text_reads}");
    }

    /// Acceptance: the default `types: [file]` filter reads zero file contents (`read(2)` counter, Linux), while a
    /// `types: [text]` filter must read each unknown-name file once.
    #[cfg(target_os = "linux")]
    #[test]
    fn default_types_file_reads_no_contents_syscall_counter() {
        let d = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "classify::io_counter_child", "--nocapture", "--test-threads=1"])
            .env("SNP_FILES_IO_CHILD", d.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        let line = stdout
            .lines()
            .find_map(|l| l.split_once("SNP-IO ").map(|(_, rest)| rest))
            .unwrap();
        let num = |key: &str| -> u64 {
            line.split_whitespace()
                .find_map(|kv| kv.strip_prefix(key))
                .unwrap()
                .parse()
                .unwrap()
        };
        let (file, text) = (num("file="), num("text="));
        // Two /proc/self/io reads (≈ 2 read(2) each) bracket each measurement; nothing else may read.
        assert!(file <= 8, "types: [file] read {file} times");
        assert!(text >= 200, "types: [text] read only {text} times");
    }

    /// Portable form of the same law: unreadable contents never matter to `types: [file]`.
    #[cfg(unix)]
    #[test]
    fn default_types_file_never_opens_unreadable_files() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let names = unknown_names(d.path(), 20);
        for n in &names {
            std::fs::set_permissions(d.path().join(n), std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(FileTable::from_args(names.clone(), d.path()), &cache, &pool);
        assert_eq!(c.for_hook(&filter("", "^$", &["file"], None, &cache), &pool).unwrap().len(), 20);
        assert!(c.for_hook(&filter("", "^$", &["binary"], None, &cache), &pool).is_err());
        for n in &names {
            std::fs::set_permissions(d.path().join(n), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    /// Order and duplicates are kept; `files`/`exclude` are `re.search`; the prefix is boundary-aware.
    #[test]
    fn filter_laws() {
        let d = tempfile::tempdir().unwrap();
        for p in ["a.py", "sub/b.py", "sub2/c.py", "sub/d.txt"] {
            let full = d.path().join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, b"x\n").unwrap();
        }
        let names: Vec<OsString> = ["sub/b.py", "a.py", "sub/b.py", "missing.py", "sub2/c.py", "sub/d.txt"]
            .iter()
            .map(OsString::from)
            .collect();
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(FileTable::from_args(names, d.path()), &cache, &pool);
        assert_eq!(views(&c, &c.kept), ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py", "sub/d.txt"]);
        let py = c.for_hook(&filter(r"\.py$", "^$", &["python"], None, &cache), &pool).unwrap();
        assert_eq!(views(&c, &py), ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py"]);
        let ex = c.for_hook(&filter("", "^sub/", &["file"], None, &cache), &pool).unwrap();
        assert_eq!(views(&c, &ex), ["a.py", "sub2/c.py"]);
        let pre = c.for_hook(&filter("", "^$", &["file"], Some(b"sub"), &cache), &pool).unwrap();
        assert_eq!(views(&c, &pre), ["sub/b.py", "sub/b.py", "sub/d.txt"]);
        // check-useless-excludes: by_types consults the types only.
        let only_types = filter("^nothing$", ".*", &["text"], Some(b"nowhere"), &cache);
        let all = c.by_types(&c.kept, &only_types).unwrap();
        assert_eq!(views(&c, &all), ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py", "sub/d.txt"]);
        // Top-level include/exclude.
        let table = FileTable::from_args(
            ["a.py", "sub/b.py", "sub/d.txt"].iter().map(OsString::from).collect(),
            d.path(),
        );
        let include = cache.get("^sub/", Options::CONFIG).unwrap();
        let exclude = cache.get(r"\.txt$", Options::CONFIG).unwrap();
        let c = Classifier::new(Arc::new(table), &include, &exclude, &pool).unwrap();
        assert_eq!(views(&c, &c.kept), ["sub/b.py"]);
    }

    /// Trusted git modes give the stat tier with zero syscalls (the files do not even exist on disk).
    #[test]
    fn trusted_modes_need_no_syscalls() {
        let names: Vec<OsString> = ["x.py", "run", "link", "subm"].iter().map(OsString::from).collect();
        let mut table = FileTable::from_args(names, Path::new("/definitely/not/here"));
        table.set_trusted_modes(vec![
            (&b"x.py"[..], 0o100_644),
            (&b"run"[..], 0o100_755),
            (&b"link"[..], 0o120_000),
            (&b"subm"[..], 0o160_000),
        ]);
        table.retain_lexists();
        // Windows ignores symlink modes (core.symlinks=false): `link` then needs an lstat and is gone.
        assert_eq!(table.len(), if cfg!(windows) { 3 } else { 4 });
        let cache = RegexCache::new();
        let pool = pool(1);
        let c = classifier(table, &cache, &pool);
        let files = c.for_hook(&filter("", "^$", &["file"], None, &cache), &pool).unwrap();
        assert_eq!(views(&c, &files), ["x.py", "run"]);
        // Symlink modes are not trusted on Windows (core.symlinks=false).
        if !cfg!(windows) {
            let s = c.for_hook(&filter("", "^$", &["symlink"], None, &cache), &pool).unwrap();
            assert_eq!(views(&c, &s), ["link"]);
            let dirs = c.for_hook(&filter("", "^$", &["directory"], None, &cache), &pool).unwrap();
            assert_eq!(views(&c, &dirs), ["subm"]);
        }
        let exe = c.for_hook(&filter("", "^$", &["executable"], None, &cache), &pool).unwrap();
        assert!(views(&c, &exe).contains(&"run".to_owned()));
    }

    /// `prefetch` inside a permit holder runs inline: a single-permit pool cannot deadlock (ADR-070).
    #[test]
    fn prefetch_is_reentrant_under_a_held_permit() {
        let d = tempfile::tempdir().unwrap();
        let names = unknown_names(d.path(), 600);
        let table = Arc::new(FileTable::from_args(names, d.path()));
        let pool = pool(1);
        let ids: Vec<FileId> = (0..600).map(FileId).collect();
        let n = Needs {
            stat: true,
            encoding: true,
            shebang: false,
        };
        let results = pool.map_ordered(std::num::NonZeroUsize::new(1).unwrap(), 2, |_| {
            table.prefetch(&ids, n, &pool);
            Ok(())
        });
        assert!(results.iter().all(Result::is_ok));
        assert!(matches!(table.tags(FileId(5), n), TagOutcome::Tags(t) if t.contains("text")));
        // And in parallel on a larger pool.
        let pool = pool_n(4);
        let table = Arc::new(FileTable::from_args(unknown_names(d.path(), 600), d.path()));
        table.prefetch(&ids, n, &pool);
        for &id in &ids {
            assert!(matches!(table.tags(id, n), TagOutcome::Tags(t) if t.contains("text")));
        }
    }

    fn pool_n(n: usize) -> Arc<JobPool> {
        pool(n)
    }

    /// The matching view: `\` is a literal character on POSIX and a separator (normalised to `/`) on Windows
    /// (`run.py:119-124`, 14 P-60).
    #[test]
    fn windows_separator_normalisation() {
        let table = FileTable::from_args(
            vec![OsString::from("dir\\file.py"), OsString::from("plain/x.py")],
            Path::new("/r"),
        );
        let cache = RegexCache::new();
        let slash = cache.get("^dir/", Options::CONFIG).unwrap();
        if cfg!(windows) {
            assert_eq!(table.match_view(FileId(0)), "dir/file.py");
            assert_eq!(table.os(FileId(0)).to_str(), Some("dir/file.py"));
            assert!(slash.search(table.match_view(FileId(0))).unwrap());
            let p = RepoPath::new(b"dir").unwrap();
            assert_eq!(table.relative_to(FileId(0), &p).as_deref(), Some(&b"file.py"[..]));
        } else {
            assert_eq!(table.match_view(FileId(0)), "dir\\file.py");
            assert!(!slash.search(table.match_view(FileId(0))).unwrap());
        }
        assert_eq!(table.match_view(FileId(1)), "plain/x.py");
    }

    /// A tag the name gives is still decided by the kind: `link.py` (a symlink) is not `python` (identify returns
    /// `{symlink}` alone for links, directories and sockets).
    #[cfg(unix)]
    #[test]
    fn name_tags_never_apply_to_links_or_directories() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("real.py"), b"x\n").unwrap();
        std::os::unix::fs::symlink("real.py", d.path().join("link.py")).unwrap();
        std::fs::create_dir_all(d.path().join("pkg.py")).unwrap();
        let cache = RegexCache::new();
        let pool = pool(1);
        let c = classifier(
            FileTable::from_args(["real.py", "link.py", "pkg.py"].iter().map(OsString::from).collect(), d.path()),
            &cache,
            &pool,
        );
        let py = c.for_hook(&filter("", "^$", &["python"], None, &cache), &pool).unwrap();
        assert_eq!(views(&c, &py), ["real.py"]);
        let none = c.for_hook(&filter("", "^$", &[], None, &cache), &pool).unwrap();
        assert_eq!(none.len(), 3);
    }
}
