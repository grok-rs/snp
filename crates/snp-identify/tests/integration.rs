//! Behavioural tests of the public API on real files (all platforms).

#[cfg(test)]
mod integration {
    use std::error::Error as _;
    use std::fs;
    use std::io::{self, Read};
    use std::path::Path;

    use file_identify::{
        Classification, ExecPolicy, FileKind, IdentifyError, Meta, Tag, TagSet, classify,
        file_is_text, parse_shebang_from_file, tags_from_meta, tags_from_path,
    };

    fn tags_of(path: &Path) -> String {
        tags_from_path(path).expect("identify").to_string()
    }

    #[test]
    fn regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        fs::write(d.join("test.py"), "").expect("write");
        fs::write(d.join("README.md"), "# x").expect("write");
        fs::write(d.join("noext"), "hello\n").expect("write");
        fs::write(d.join("blob"), [0x7f, b'E', b'L', b'F', 0]).expect("write");
        fs::write(d.join("t.plist"), b"bplist00\xd1\x01\x02_\x10\x0f").expect("write");
        fs::write(d.join("empty"), "").expect("write");
        // identify reports every file as executable on Windows.
        let exec = if cfg!(windows) {
            "executable"
        } else {
            "non-executable"
        };
        let expect = |names: String| names.parse::<TagSet>().expect("tags").to_string();
        for (name, rest) in [
            ("test.py", "python, text"),
            ("README.md", "markdown, plain-text, text"),
            ("noext", "text"),
            ("blob", "binary"),
            ("t.plist", "binary, plist"),
            ("empty", "text"),
        ] {
            assert_eq!(
                tags_of(&d.join(name)),
                expect(format!("file, {exec}, {rest}")),
                "{name}"
            );
        }
        assert_eq!(tags_of(d), "directory");
    }

    #[test]
    fn missing_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.py");
        let err = tags_from_path(&path).expect_err("missing");
        assert_eq!(
            err.to_string(),
            format!("{} does not exist.", path.display())
        );
        assert!(matches!(err, IdentifyError::PathNotFound { .. }));
        assert!(err.source().is_some());
        assert!(matches!(
            file_is_text(&path),
            Err(IdentifyError::PathNotFound { .. })
        ));
        assert!(matches!(
            parse_shebang_from_file(&path),
            Err(IdentifyError::PathNotFound { .. })
        ));
        // A known Meta that needs content reports the missing file when opening.
        let meta = Meta::regular(false);
        assert!(matches!(
            tags_from_meta(dir.path().join("noext"), meta),
            Err(IdentifyError::PathNotFound { .. })
        ));
    }

    #[test]
    fn git_mode_fast_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("run");
        fs::write(&script, "#!/usr/bin/env ruby\nputs 1\n").expect("write");
        let meta = Meta::from_git_mode(0o100_755).expect("mode");
        assert_eq!(
            tags_from_meta(&script, meta).expect("identify").to_string(),
            "executable, file, ruby, text"
        );
        let meta = Meta::from_git_mode(0o120_000).expect("mode");
        assert_eq!(
            tags_from_meta("whatever", meta)
                .expect("identify")
                .to_string(),
            "symlink"
        );
        let meta = Meta::from_git_mode(0o160_000).expect("mode");
        assert_eq!(
            tags_from_meta("submodule", meta)
                .expect("identify")
                .to_string(),
            "directory"
        );
    }

    #[test]
    fn bounded_reads() {
        /// Endless content that counts what was read.
        struct Endless {
            first: &'static [u8],
            fill: u8,
            served: usize,
        }
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                for b in buf.iter_mut() {
                    *b = self.first.get(self.served).copied().unwrap_or(self.fill);
                    self.served += 1;
                }
                Ok(buf.len())
            }
        }
        let Classification::NeedsContent(pending) = classify(Meta::regular(true), "huge") else {
            panic!("needs content");
        };
        // Not a script: only the 1 KiB head is read.
        let mut plain = Endless {
            first: b"",
            fill: b'a',
            served: 0,
        };
        assert_eq!(
            pending.finish(&mut plain).expect("read").to_string(),
            "executable, file, text"
        );
        assert_eq!(plain.served, 1024);
        // A shebang line that never ends: reading stops at MAX_SHEBANG_BYTES.
        let mut script = Endless {
            first: b"#!/bin/sh ",
            fill: b' ',
            served: 0,
        };
        assert_eq!(
            pending.finish(&mut script).expect("read").to_string(),
            "executable, file, text"
        );
        assert!(
            script.served <= file_identify::MAX_SHEBANG_BYTES + 16 * 1024,
            "{}",
            script.served
        );
    }

    #[test]
    fn large_file_without_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big");
        let mut content = b"#!/bin/sh ".to_vec();
        content.resize(4 * 1024 * 1024, b'x');
        fs::write(&path, &content).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let tags = tags_from_path(&path).expect("identify");
        assert!(tags.contains(Tag::TEXT) && tags.contains(Tag::EXECUTABLE));
        assert!(
            !tags.contains("shell"),
            "line exceeds MAX_SHEBANG_BYTES: no shebang"
        );
    }

    #[test]
    fn file_kind_and_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let md = fs::symlink_metadata(dir.path()).expect("lstat");
        assert_eq!(
            FileKind::from_file_type(md.file_type()),
            FileKind::Directory
        );
        assert_eq!(
            Meta::from_metadata(&md, ExecPolicy::default()).kind(),
            FileKind::Directory
        );
    }

    #[test]
    fn public_types_are_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<Tag>();
        check::<TagSet>();
        check::<file_identify::tags::Iter>();
        check::<file_identify::TagTable>();
        check::<Meta>();
        check::<FileKind>();
        check::<ExecPolicy>();
        check::<Classification>();
        check::<file_identify::Pending>();
        check::<IdentifyError>();
        check::<file_identify::UnknownTagError>();
        check::<file_identify::ShlexError>();
        check::<file_identify::FileInfo<'static>>();
    }
}
