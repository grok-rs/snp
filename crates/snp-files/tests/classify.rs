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

    fn filter(
        files: &str,
        exclude: &str,
        types: &[&str],
        prefix: Option<&[u8]>,
        cache: &RegexCache,
    ) -> HookFilter {
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
        ids.iter()
            .map(|&id| c.table.match_view(id).to_owned())
            .collect()
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
            .args([
                "--exact",
                "classify::io_counter_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SNP_FILES_IO_CHILD", d.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
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
            std::fs::set_permissions(d.path().join(n), std::fs::Permissions::from_mode(0o000))
                .unwrap();
        }
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(FileTable::from_args(names.clone(), d.path()), &cache, &pool);
        assert_eq!(
            c.for_hook(&filter("", "^$", &["file"], None, &cache), &pool)
                .unwrap()
                .len(),
            20
        );
        assert!(
            c.for_hook(&filter("", "^$", &["binary"], None, &cache), &pool)
                .is_err()
        );
        for n in &names {
            std::fs::set_permissions(d.path().join(n), std::fs::Permissions::from_mode(0o644))
                .unwrap();
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
        let names: Vec<OsString> = [
            "sub/b.py",
            "a.py",
            "sub/b.py",
            "missing.py",
            "sub2/c.py",
            "sub/d.txt",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(FileTable::from_args(names, d.path()), &cache, &pool);
        assert_eq!(
            views(&c, &c.kept),
            ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py", "sub/d.txt"]
        );
        let py = c
            .for_hook(&filter(r"\.py$", "^$", &["python"], None, &cache), &pool)
            .unwrap();
        assert_eq!(
            views(&c, &py),
            ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py"]
        );
        let ex = c
            .for_hook(&filter("", "^sub/", &["file"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &ex), ["a.py", "sub2/c.py"]);
        let pre = c
            .for_hook(&filter("", "^$", &["file"], Some(b"sub"), &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &pre), ["sub/b.py", "sub/b.py", "sub/d.txt"]);
        // check-useless-excludes: by_types consults the types only.
        let only_types = filter("^nothing$", ".*", &["text"], Some(b"nowhere"), &cache);
        let all = c.by_types(&c.kept, &only_types).unwrap();
        assert_eq!(
            views(&c, &all),
            ["sub/b.py", "a.py", "sub/b.py", "sub2/c.py", "sub/d.txt"]
        );
        // Top-level include/exclude.
        let table = FileTable::from_args(
            ["a.py", "sub/b.py", "sub/d.txt"]
                .iter()
                .map(OsString::from)
                .collect(),
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
        let names: Vec<OsString> = ["x.py", "run", "link", "subm"]
            .iter()
            .map(OsString::from)
            .collect();
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
        let files = c
            .for_hook(&filter("", "^$", &["file"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &files), ["x.py", "run"]);
        // Symlink modes are not trusted on Windows (core.symlinks=false).
        if !cfg!(windows) {
            let s = c
                .for_hook(&filter("", "^$", &["symlink"], None, &cache), &pool)
                .unwrap();
            assert_eq!(views(&c, &s), ["link"]);
            let dirs = c
                .for_hook(&filter("", "^$", &["directory"], None, &cache), &pool)
                .unwrap();
            assert_eq!(views(&c, &dirs), ["subm"]);
        }
        let exe = c
            .for_hook(&filter("", "^$", &["executable"], None, &cache), &pool)
            .unwrap();
        assert!(views(&c, &exe).contains(&"run".to_owned()));
    }

    /// ADR-106: with trusted index modes the classifier keeps and identifies files without a syscall (none of them
    /// exists on disk), a filter on `executable` takes the `lstat` answer instead (gone here), and once the flag is
    /// cleared (the diff oracle after a hook) a file no hook identified yet takes its `lstat` too.
    #[cfg(unix)]
    #[test]
    fn index_trust_classifies_without_syscalls_until_cleared() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let names: Vec<OsString> = ["a.py", "b.py", "run.sh", "gone.py"]
            .iter()
            .map(OsString::from)
            .collect();
        let mut table = FileTable::from_args(names, Path::new("/definitely/not/here"));
        let valid = Arc::new(AtomicBool::new(true));
        let modes: Box<[u32]> = Box::new([0o100_644, 0o100_644, 0o100_755, 0]);
        assert!(table.trust_index_modes(modes, Arc::clone(&valid), tags(&["file", "python"])));
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(table, &cache, &pool);
        assert_eq!(
            views(&c, &c.kept),
            ["a.py", "b.py", "run.sh"],
            "gone.py has no mode"
        );
        let py = c
            .for_hook(&filter(r"^a\.py$", "^$", &["python"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &py), ["a.py"]);
        let files = c
            .for_hook(&filter(r"\.sh$", "^$", &["file"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &files), ["run.sh"]);
        // the executable bit is never the index's: a filter on it takes the lstat answer
        let exe = c
            .for_hook(&filter("", "^$", &["executable"], None, &cache), &pool)
            .unwrap();
        assert!(exe.is_empty(), "{:?}", views(&c, &exe));
        // after the flag is cleared, b.py (never identified) takes its lstat; a.py keeps its memoised tags
        valid.store(false, Ordering::Release);
        let files = c
            .for_hook(&filter("", "^$", &["file"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &files), ["a.py", "run.sh"]);
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
            assert_eq!(
                table.relative_to(FileId(0), &p).as_deref(),
                Some(&b"file.py"[..])
            );
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
            FileTable::from_args(
                ["real.py", "link.py", "pkg.py"]
                    .iter()
                    .map(OsString::from)
                    .collect(),
                d.path(),
            ),
            &cache,
            &pool,
        );
        let py = c
            .for_hook(&filter("", "^$", &["python"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &py), ["real.py"]);
        let none = c
            .for_hook(&filter("", "^$", &[], None, &cache), &pool)
            .unwrap();
        assert_eq!(none.len(), 3);
    }

    /// ADR-101: `--files $'x\xc3' $'\xa9y.py'` joins into a valid arena ("xéy.py") whose spans split the "é"; each
    /// name still matches on its own lossy view, so `files: \.py$` keeps only the second.
    #[cfg(target_os = "linux")]
    #[test]
    fn joined_args_splitting_a_character_match_their_own_view() {
        use std::os::unix::ffi::OsStringExt;
        let d = tempfile::tempdir().unwrap();
        let names = [b"x\xc3".to_vec(), b"\xa9y.py".to_vec()];
        for n in &names {
            std::fs::write(d.path().join(OsString::from_vec(n.clone())), b"x\n").unwrap();
        }
        let cache = RegexCache::new();
        let pool = pool(2);
        let c = classifier(
            FileTable::from_args(
                names.iter().cloned().map(OsString::from_vec).collect(),
                d.path(),
            ),
            &cache,
            &pool,
        );
        assert_eq!(views(&c, &c.kept), ["x\u{fffd}", "\u{fffd}y.py"]);
        let py = c
            .for_hook(&filter("\\.py$", "^$", &["file"], None, &cache), &pool)
            .unwrap();
        assert_eq!(py, [FileId(1)]);
    }

    /// The first unreadable file in table order is the one reported, with two of them in different partitions:
    /// inline (one permit), on the pool, and again once the head tier is memoised (the memory-only partition that
    /// runs on the calling thread, ADR-101).
    #[cfg(unix)]
    #[test]
    fn first_unreadable_file_in_order_wins_inline_and_on_the_pool() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let names = unknown_names(d.path(), 2_000);
        for k in [700, 1_900] {
            let p = d.path().join(&names[k]);
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let cache = RegexCache::new();
        let text_filter = filter("", "^$", &["text"], None, &cache);
        for permits in [1, 8] {
            let pool = pool(permits);
            let c = classifier(FileTable::from_args(names.clone(), d.path()), &cache, &pool);
            for round in 0..2 {
                let err = c.for_hook(&text_filter, &pool).unwrap_err();
                let snp_core::failure::Failure::Exc { exc, .. } = err else {
                    panic!("not an exception: {err:?}")
                };
                assert_eq!(
                    String::from_utf8_lossy(&exc.text),
                    "[Errno 13] Permission denied: 'data700.unknownext'",
                    "permits {permits}, round {round}"
                );
            }
            // Memoised, the files without content needs classify on the calling thread under a permit.
            let file = c
                .for_hook(&filter("", "^$", &["file"], None, &cache), &pool)
                .unwrap();
            assert_eq!(file.len(), 2_000);
        }
        for k in [700, 1_900] {
            let p = d.path().join(&names[k]);
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    /// The tree of the meta-only tests: every kind of entry, the index mode git would record for each (0 = none),
    /// and the names (with one that does not exist).
    #[cfg(unix)]
    fn mixed_tree(root: &Path) -> (Vec<OsString>, Box<[u32]>) {
        use std::os::unix::fs::PermissionsExt;
        let files: [(&str, u32, &[u8], u32); 7] = [
            ("a.py", 0o644, b"x = 1\n", 0o100_644),
            ("run.sh", 0o755, b"echo\n", 0o100_755),
            ("tool", 0o755, b"#!/usr/bin/env python3\n", 0o100_755),
            ("data.unknownext", 0o644, b"\0\x01binary", 0o100_644),
            ("notes", 0o644, b"plain words\n", 0o100_644),
            ("r654", 0o654, b"#!/bin/sh\n", 0o100_644),
            ("secret.unknownext", 0o000, b"x", 0o100_644),
        ];
        let mut names = Vec::new();
        let mut modes = Vec::new();
        for (name, mode, body, index) in files {
            std::fs::write(root.join(name), body).unwrap();
            std::fs::set_permissions(root.join(name), std::fs::Permissions::from_mode(mode))
                .unwrap();
            names.push(OsString::from(name));
            modes.push(index);
        }
        std::os::unix::fs::symlink("a.py", root.join("link.py")).unwrap();
        std::os::unix::fs::symlink("missing", root.join("broken")).unwrap();
        std::fs::create_dir_all(root.join("pkg.py")).unwrap();
        let _sock = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
        for (name, index) in [
            ("link.py", 0o120_000),
            ("broken", 0o120_000),
            ("pkg.py", 0o160_000),
            ("sock", 0),
            ("gone.py", 0),
        ] {
            names.push(OsString::from(name));
            modes.push(index);
        }
        (names, modes.into_boxed_slice())
    }

    /// One filter's outcome: the kept names, or the error text.
    #[cfg(unix)]
    fn outcome(c: &Classifier, f: &HookFilter, pool: &JobPool) -> Result<Vec<String>, String> {
        match c.for_hook(f, pool) {
            Ok(ids) => Ok(views(c, &ids)),
            Err(snp_core::failure::Failure::Exc { exc, .. }) => {
                Err(String::from_utf8_lossy(&exc.text).into_owned())
            }
            Err(other) => Err(format!("{other:?}")),
        }
    }

    /// ADR-110: a hook whose filter references only meta tags (`file`, `executable`, `symlink`, …) keeps the same
    /// files, and a content hook reports the same unreadable file, whether it runs first, after `text`/`python`
    /// hooks, or alone, without trusted index modes and with them, declared with and without a shebang filter (the
    /// meta tier and the stat tier agree, and the meta tier never reads content).
    #[cfg(unix)]
    #[test]
    fn meta_only_hooks_agree_in_any_order() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (names, modes) = mixed_tree(d.path());
        let unreadable = !rustix::process::geteuid().is_root();
        let cache = RegexCache::new();
        let f = |types: &[&str], types_or: &[&str], exclude_types: &[&str]| {
            HookFilter::compile(
                "",
                "^$",
                tags(types),
                tags(types_or),
                tags(exclude_types),
                None,
                &cache,
            )
            .unwrap()
        };
        let filters = [
            ("file", f(&["file"], &[], &[])),
            ("executable", f(&["executable"], &[], &[])),
            ("links-or-dirs", f(&[], &["symlink", "directory"], &[])),
            ("not-exec", f(&["file"], &[], &["executable"])),
            ("text", f(&["text"], &[], &[])),
            ("python", f(&["python"], &[], &[])),
            ("shell", f(&["shell"], &[], &[])),
        ];
        let orders: [&[usize]; 4] = [
            &[0, 1, 2, 3, 4, 5, 6],
            &[4, 5, 6, 0, 1, 2, 3],
            &[5, 0, 4, 2, 6, 3, 1],
            &[1, 6, 3, 5, 2, 4, 0],
        ];
        let pool = pool(2);
        // The declared tags of a trusted run: with a shebang filter (meta-only filters then take the stat tier,
        // ADR-106 decision (4)) and without one (they take the meta tier from the index modes).
        let table = |trust: Option<&[&str]>| {
            let mut t = FileTable::from_args(names.clone(), d.path());
            if let Some(declared) = trust {
                let valid = Arc::new(std::sync::atomic::AtomicBool::new(true));
                assert!(t.trust_index_modes(modes.clone(), valid, tags(declared)));
            }
            t
        };
        let alone: Vec<Result<Vec<String>, String>> = filters
            .iter()
            .map(|(_, flt)| outcome(&classifier(table(None), &cache, &pool), flt, &pool))
            .collect();
        assert!(
            alone[0]
                .as_ref()
                .unwrap()
                .contains(&"secret.unknownext".to_owned())
        );
        assert_eq!(alone[4].is_err(), unreadable, "{:?}", alone[4]);
        assert_eq!(alone[2].as_ref().unwrap(), &["link.py", "broken", "pkg.py"]);
        let trusts: [Option<&[&str]>; 3] = [
            None,
            Some(&["file", "python"]),
            Some(&["file", "symlink", "directory", "text"]),
        ];
        for trust in trusts {
            for order in orders {
                let c = classifier(table(trust), &cache, &pool);
                for &k in order {
                    let (name, flt) = &filters[k];
                    assert_eq!(
                        outcome(&c, flt, &pool),
                        alone[k],
                        "{name} in order {order:?}, trust {trust:?}"
                    );
                }
            }
        }
        std::fs::set_permissions(
            d.path().join("secret.unknownext"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
    }

    /// ADR-110 keeps D-CRASH-08's semantics: a file a meta-only hook identified and a later hook deleted keeps its
    /// first identification for name tags (no second `lstat`), is dropped by a later hook that needs its content
    /// (the content tier was not computed), and from then on by every hook, as with the stat tier before.
    #[cfg(unix)]
    #[test]
    fn deletion_after_a_meta_only_hook() {
        let d = tempfile::tempdir().unwrap();
        for name in ["a.py", "data.unknownext", "keep.unknownext"] {
            std::fs::write(d.path().join(name), b"x\n").unwrap();
        }
        let names: Vec<OsString> = ["a.py", "data.unknownext", "keep.unknownext"]
            .iter()
            .map(OsString::from)
            .collect();
        let cache = RegexCache::new();
        let pool = pool(1);
        let c = classifier(FileTable::from_args(names, d.path()), &cache, &pool);
        let file = filter("", "^$", &["file"], None, &cache);
        assert_eq!(c.for_hook(&file, &pool).unwrap().len(), 3);
        std::fs::remove_file(d.path().join("a.py")).unwrap();
        std::fs::remove_file(d.path().join("data.unknownext")).unwrap();
        let py = c
            .for_hook(&filter("", "^$", &["python"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &py), ["a.py"]);
        let text = c
            .for_hook(&filter("", "^$", &["text"], None, &cache), &pool)
            .unwrap();
        assert_eq!(views(&c, &text), ["a.py", "keep.unknownext"]);
        let files = c.for_hook(&file, &pool).unwrap();
        assert_eq!(views(&c, &files), ["a.py", "keep.unknownext"]);
    }

    /// One case of [`trusted_meta_only_first_hook_keeps_the_first_observation`]: the name, its content, its mode on
    /// disk and in the index, the mode hook 1 leaves (`None`: hook 1 deletes it), hook 2's `types` and
    /// `exclude_types`, and how many files hook 2 keeps under pre-commit 4.7.0.
    #[cfg(unix)]
    type FirstObservation = (
        &'static str,
        &'static [u8],
        u32,
        Option<u32>,
        &'static [&'static str],
        &'static [&'static str],
        usize,
    );

    /// ADR-110 under ADR-106 decision (4): when a filter of the run needs shebang tags, a file whose name gives no
    /// tags gets no trusted cell, also when a meta-only hook identifies it first. It takes its `lstat` there and then,
    /// since its executable bit decides whether identify reads its shebang. A first hook that flips that bit or
    /// deletes the file (the diff oracle then clears the trust) changes nothing a later hook sees, as pre-commit
    /// memoises the full tags of the first identification: trusted and untrusted runs agree, and a meta-only first
    /// hook agrees with a `text` one.
    #[cfg(unix)]
    #[test]
    fn trusted_meta_only_first_hook_keeps_the_first_observation() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        const PY: &[u8] = b"#!/usr/bin/env python3\nprint(1)\n";
        let cases: [FirstObservation; 4] = [
            // memo {file, non-executable, text}: chmod +x does not make it python
            ("tool", PY, 0o644, Some(0o755), &["python"], &[], 0),
            // memo {file, executable, python, text}: chmod -x does not stop it being python
            ("tool", PY, 0o755, Some(0o644), &["python"], &[], 1),
            ("tool", PY, 0o755, Some(0o644), &["file"], &["python"], 0),
            // memo {file, non-executable, text}: deleted, it is still a file that is not python
            (
                "notes",
                b"plain words\n",
                0o644,
                None,
                &["file"],
                &["python"],
                1,
            ),
        ];
        let cache = RegexCache::new();
        let pool = pool(1);
        let types_filter = |types: &[&str], exclude_types: &[&str]| {
            HookFilter::compile(
                "",
                "^$",
                tags(types),
                TagSet::EMPTY,
                tags(exclude_types),
                None,
                &cache,
            )
            .unwrap()
        };
        let mut wrong = Vec::new();
        for (name, body, mode, after, types, exclude_types, want) in cases {
            for first in [&["file"][..], &["text"]] {
                for trusted in [false, true] {
                    let then = after.map_or_else(|| "deleted".to_owned(), |to| format!("{to:o}"));
                    let case = format!(
                        "{name} {mode:o} -> {then}, first {first:?}, then {types:?} minus \
                         {exclude_types:?}, trusted {trusted}"
                    );
                    let dir = tempfile::tempdir().unwrap();
                    let path = dir.path().join(name);
                    std::fs::write(&path, body).unwrap();
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
                    let mut table = FileTable::from_args(vec![OsString::from(name)], dir.path());
                    let valid = Arc::new(AtomicBool::new(true));
                    if trusted {
                        let modes: Box<[u32]> = Box::new([0o100_000 | mode]);
                        let referenced = tags(&["file", "text", "python"]);
                        assert!(table.trust_index_modes(modes, Arc::clone(&valid), referenced));
                    }
                    let cls = classifier(table, &cache, &pool);
                    assert_eq!(
                        cls.for_hook(&types_filter(first, &[]), &pool)
                            .unwrap()
                            .len(),
                        1,
                        "{case}"
                    );
                    // hook 1 changes the worktree, then the diff oracle clears the trust
                    match after {
                        Some(to) => {
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(to))
                                .unwrap();
                        }
                        None => std::fs::remove_file(&path).unwrap(),
                    }
                    valid.store(false, Ordering::Release);
                    let hook2 = types_filter(types, exclude_types);
                    let kept = cls.for_hook(&hook2, &pool).unwrap().len();
                    if kept != want {
                        wrong.push(format!("{case}: kept {kept}, pre-commit keeps {want}"));
                    }
                }
            }
        }
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
