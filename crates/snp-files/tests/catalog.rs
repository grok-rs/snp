//! Catalog rows owned by WP-22 in snp-files (ARCHITECTURE §13.8, `tests/catalog/index.tsv`): X13-CLS-01…05 and
//! X13-GIT-05, 06, 08 (13 X.4.3–X.4.4). Each test is named after its row.
//!
//! Owner: WP-22.

#[cfg(test)]
mod catalog {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    #[cfg(unix)]
    use std::time::Duration;

    use file_identify::TagSet;
    use snp_core::cancel::CancelToken;
    use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
    use snp_proc::pool::JobPool;
    use snp_pyre::{Options, RegexCache};

    const FULL: Needs = Needs {
        stat: true,
        encoding: true,
        shebang: true,
    };

    fn pool() -> Arc<JobPool> {
        JobPool::new(std::num::NonZeroUsize::new(2).unwrap(), CancelToken::new())
    }

    fn root() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().to_path_buf();
        (d, p)
    }

    fn filter(files: &str, types: &[&str], cache: &RegexCache) -> HookFilter {
        HookFilter::compile(
            files,
            "^$",
            TagSet::from_names(types).unwrap(),
            TagSet::EMPTY,
            TagSet::EMPTY,
            None,
            cache,
        )
        .unwrap()
    }

    fn classifier(root: &Path, names: &[&str], cache: &RegexCache, pool: &JobPool) -> Classifier {
        let table = FileTable::from_args(names.iter().map(OsString::from).collect(), root);
        let all = cache.get("", Options::CONFIG).unwrap();
        let none = cache.get("^$", Options::CONFIG).unwrap();
        Classifier::new(Arc::new(table), &all, &none, pool).unwrap()
    }

    fn names_of(c: &Classifier, ids: &[FileId]) -> Vec<String> {
        ids.iter()
            .map(|&id| c.table.match_view(id).to_owned())
            .collect()
    }

    #[cfg(unix)]
    fn is_root() -> bool {
        #[cfg(unix)]
        {
            rustix::process::geteuid().is_root()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// X13-CLS-01: an earlier hook deletes a file whose tags were not computed yet → dropped from this and every
    /// later hook (pre-commit: `ValueError: b.txt does not exist.`, exit 3); a file whose tags were already
    /// computed before the deletion is still passed (parity).
    #[test]
    fn x13_cls_01_vanished_file_is_dropped_from_this_and_later_hooks() {
        let (_g, root) = root();
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(root.join(n), b"x\n").unwrap();
        }
        let cache = RegexCache::new();
        let pool = pool();
        let c = classifier(&root, &["a.txt", "b.txt", "c.txt"], &cache, &pool);
        // Hook 1 (`files: ^a\.txt$`) runs `rm -f b.txt` (tags of b.txt not computed yet) …
        let h1 = c.for_hook(&filter(r"^a\.txt$", &["file"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &h1), ["a.txt"]);
        std::fs::remove_file(root.join("b.txt")).unwrap();
        // … and hook 2 (`types: [file]`) no longer sees it; neither does hook 3.
        let h2 = c.for_hook(&filter("", &["file"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &h2), ["a.txt", "c.txt"]);
        let h3 = c.for_hook(&filter("", &["text"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &h3), ["a.txt", "c.txt"]);
        // c.txt was classified by hook 2; deleting it now keeps its cached tags (pre-commit's functools.cache).
        std::fs::remove_file(root.join("c.txt")).unwrap();
        let h4 = c.for_hook(&filter("", &["file"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &h4), ["a.txt", "c.txt"]);
    }

    /// X13-CLS-02: a regular, non-executable, unreadable file whose name gives no encoding tag aborts the run with
    /// pre-commit's `PermissionError` text (exit 3) — but only for a hook that needs its content (D-CRASH-18); a
    /// `.txt` name never opens the file.
    #[cfg(unix)]
    #[test]
    fn x13_cls_02_unreadable_file_needed_by_a_hook_fails_with_exit_3() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return; // root reads mode-000 files
        }
        let (_g, root) = root();
        for n in ["noext", "u.txt"] {
            let p = root.join(n);
            std::fs::write(&p, b"x\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let cache = RegexCache::new();
        let pool = pool();
        let c = classifier(&root, &["noext", "u.txt"], &cache, &pool);
        // `types: [file]` needs no content: both pass (pre-commit would already crash here).
        let files = c.for_hook(&filter("", &["file"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &files), ["noext", "u.txt"]);
        // `files: \.txt$` + `types: [text]`: the name decides, no open.
        let txt = c.for_hook(&filter(r"\.txt$", &["text"], &cache), &pool).unwrap();
        assert_eq!(names_of(&c, &txt), ["u.txt"]);
        // `types: [text]` over noext: fail closed with the identify open error.
        let err = c.for_hook(&filter("", &["text"], &cache), &pool).unwrap_err();
        assert_eq!(err.exit_code(), 3);
        let snp_core::failure::Failure::Exc { exc, .. } = err else {
            panic!("not an exception")
        };
        assert_eq!(&*exc.ty, "PermissionError");
        assert_eq!(
            String::from_utf8_lossy(&exc.text),
            "[Errno 13] Permission denied: 'noext'"
        );
        for n in ["noext", "u.txt"] {
            std::fs::set_permissions(root.join(n), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    /// Runs `f` on a thread and fails the test if it does not finish within 5 s (a blocking open would hang).
    #[cfg(unix)]
    fn within_5s<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .spawn(move || {
                let _sent = tx.send(f());
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("classification blocked")
    }

    #[cfg(unix)]
    fn mkfifo(p: &Path, mode: u32) {
        rustix::fs::mknodat(
            rustix::fs::CWD,
            p,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(mode),
            0,
        )
        .unwrap();
    }

    /// X13-CLS-03: a FIFO passed via `--files` never blocks (pre-commit hangs forever): opened `O_NONBLOCK`, an empty
    /// FIFO is `text`, as identify classifies it once a writer comes and goes. Also with a writer that holds the
    /// FIFO open without writing (`EAGAIN` = no data).
    #[cfg(unix)]
    #[test]
    fn x13_cls_03_fifo_never_blocks() {
        let (g, root) = root();
        mkfifo(&root.join("ff"), 0o644);
        mkfifo(&root.join("held"), 0o644);
        let r = root.clone();
        let tags = within_5s(move || {
            let t = FileTable::from_args(vec![OsString::from("ff")], &r);
            match t.tags(FileId(0), FULL) {
                TagOutcome::Tags(tags) => tags.names().map(str::to_owned).collect::<Vec<_>>(),
                other => panic!("{other:?}"),
            }
        });
        assert_eq!(tags, ["file", "non-executable", "text"]);
        // A writer that opened the FIFO (non-blocking: no reader yet would fail with ENXIO, so open it read-write).
        let _writer = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("held"))
            .unwrap();
        let r = root;
        let tags = within_5s(move || {
            let t = FileTable::from_args(vec![OsString::from("held")], &r);
            match t.tags(FileId(0), FULL) {
                TagOutcome::Tags(tags) => tags.names().map(str::to_owned).collect::<Vec<_>>(),
                other => panic!("{other:?}"),
            }
        });
        assert_eq!(tags, ["file", "non-executable", "text"]);
        // /dev/null is text, /dev/zero binary (pre-commit's answers; both are character devices).
        let t = FileTable::from_args(
            vec![OsString::from("/dev/null"), OsString::from("/dev/zero")],
            Path::new("/"),
        );
        let get = |i| match t.tags(FileId(i), FULL) {
            TagOutcome::Tags(tags) => tags,
            other => panic!("{other:?}"),
        };
        assert!(get(0).contains("text"));
        assert!(get(1).contains("binary"));
        drop(g);
    }

    /// X13-CLS-04: an `lstat` error other than ENOENT (ENOTDIR, ENAMETOOLONG, EACCES on a parent) counts as gone
    /// (identify maps every `OSError` to `does not exist.`): dropped by `lexists`, `Vanished` for tags.
    #[test]
    fn x13_cls_04_lstat_errors_other_than_enoent_count_as_vanished() {
        let (_g, root) = root();
        std::fs::write(root.join("plain"), b"x").unwrap();
        let long: String = std::iter::repeat_n('n', 300).collect();
        let names = vec![
            OsString::from("plain/child"),
            OsString::from(long),
            OsString::from("plain"),
        ];
        let table = FileTable::from_args(names.clone(), &root);
        for i in 0..2 {
            assert!(matches!(table.tags(FileId(i), FULL), TagOutcome::Vanished));
        }
        let mut table = FileTable::from_args(names, &root);
        table.retain_lexists();
        assert_eq!(table.len(), 1);
        assert_eq!(table.bytes(FileId(0)), b"plain");
        #[cfg(unix)]
        if !is_root() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(root.join("locked")).unwrap();
            std::fs::write(root.join("locked/f.py"), b"x").unwrap();
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o000))
                .unwrap();
            let t = FileTable::from_args(vec![OsString::from("locked/f.py")], &root);
            assert!(matches!(t.tags(FileId(0), FULL), TagOutcome::Vanished));
            std::fs::set_permissions(root.join("locked"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
    }

    /// X13-CLS-05: file-identify (0.5) itself never opens a FIFO, and snp's own content tier opens non-regular files
    /// non-blocking: neither path can hang.
    #[cfg(unix)]
    #[test]
    fn x13_cls_05_file_identify_does_not_block_on_fifos() {
        let (_g, root) = root();
        let p = root.join("pipe");
        mkfifo(&p, 0o755);
        let tags = within_5s(move || file_identify::tags_from_path(&p).unwrap());
        assert!(tags.contains("file") && tags.contains("executable"));
        let r = root;
        let snp = within_5s(move || {
            let t = FileTable::from_args(vec![OsString::from("pipe")], &r);
            matches!(t.tags(FileId(0), FULL), TagOutcome::Tags(_))
        });
        assert!(snp);
    }

    /// X13-GIT-05: non-UTF-8 path bytes in git's `-z` output (pre-commit: `UnicodeDecodeError`, exit 3) stay bytes:
    /// regexes see the lossy view, identify and argv the raw bytes.
    #[cfg(unix)]
    #[test]
    fn x13_git_05_non_utf8_paths_from_git_stay_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let (_g, root) = root();
        let raw: &[u8] = b"bad\xff.txt";
        std::fs::write(root.join(std::ffi::OsStr::from_bytes(raw)), b"x\n").unwrap();
        std::fs::write(root.join("ok.txt"), b"x\n").unwrap();
        let mut buf = b"ok.txt\0".to_vec();
        buf.extend_from_slice(raw);
        buf.push(0);
        let table = FileTable::from_zbuf(buf, vec![(0, 6), (7, 15)], &root);
        let bad = FileId(1);
        assert_eq!(table.bytes(bad), raw);
        assert_eq!(table.os(bad).as_bytes(), raw);
        assert_eq!(table.match_view(bad), "bad\u{fffd}.txt");
        let cache = RegexCache::new();
        let pool = pool();
        let all = cache.get("", Options::CONFIG).unwrap();
        let none = cache.get("^$", Options::CONFIG).unwrap();
        let c = Classifier::new(Arc::new(table), &all, &none, &pool).unwrap();
        let got = c.for_hook(&filter(r"\.txt$", &["text", "plain-text"], &cache), &pool).unwrap();
        assert_eq!(got, [FileId(0), bad]);
        let got = c.for_hook(&filter("\u{fffd}", &["file"], &cache), &pool).unwrap();
        assert_eq!(got, [bad]);
    }

    /// X13-GIT-06: `run --files <non-UTF-8 name>` that exists (pre-commit: `UnicodeEncodeError`, exit 3) is
    /// classified and passed on as the raw bytes.
    #[cfg(unix)]
    #[test]
    fn x13_git_06_non_utf8_files_argument_is_passed_as_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let (_g, root) = root();
        let raw = b"h\xffllo.py".to_vec();
        std::fs::write(root.join(std::ffi::OsStr::from_bytes(&raw)), b"print(1)\n").unwrap();
        let mut table = FileTable::from_args(vec![OsString::from_vec(raw.clone())], &root);
        table.retain_lexists();
        assert_eq!(table.len(), 1);
        assert_eq!(table.os(FileId(0)).as_bytes(), raw.as_slice());
        let TagOutcome::Tags(tags) = table.tags(FileId(0), FULL) else {
            panic!()
        };
        assert_eq!(
            tags.names().collect::<Vec<_>>(),
            ["file", "non-executable", "python", "text"]
        );
    }

    /// X13-GIT-08: non-UTF-8 stderr of a failing command (pre-commit: `UnicodeDecodeError` replaces the real error)
    /// is kept as raw bytes in the `CalledProcessError` text and decoded lossily only for display.
    #[cfg(unix)]
    #[test]
    fn x13_git_08_non_utf8_command_stderr_is_kept_as_bytes() {
        use snp_core::env::Env;
        use snp_proc::spawn::{Group, Role, Spec, Stdin, capture};
        let argv: Vec<OsString> = ["/bin/sh", "-c", r"printf 'fatal: \377 bad\n' >&2; exit 128"]
            .iter()
            .map(OsString::from)
            .collect();
        let env = Env::from_pairs([("PATH", "/usr/bin:/bin")]);
        let cancel = CancelToken::new();
        let out = capture(&Spec {
            argv: &argv,
            env: &env,
            cwd: Path::new("/"),
            stdin: Stdin::Null,
            group: Group::Inherit,
            role: Role::Tool,
            deadline: None,
            cancel: &cancel,
        })
        .unwrap();
        assert_eq!(out.status.0, 128);
        assert_eq!(out.stderr, b"fatal: \xff bad\n");
        let text = snp_core::failure::pyerr::called_process_a(
            &argv,
            out.status.0,
            Some(&out.stdout),
            Some(&out.stderr),
        );
        assert!(text.split(|&b| b == b'\n').any(|l| l.ends_with(b"\xff bad")));
        assert!(String::from_utf8_lossy(&text).contains("fatal: \u{fffd} bad"));
    }
}
