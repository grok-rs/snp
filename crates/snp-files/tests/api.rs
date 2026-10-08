//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.17 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-22 (created by WP-00).

#[cfg(test)]
mod api {
    use std::borrow::Cow;
    use std::ffi::{OsStr, OsString};
    use std::path::Path;
    use std::sync::Arc;

    use file_identify::TagSet;
    use snp_core::failure::{Failure, PyExc};
    use snp_core::path::RepoPath;
    use snp_files::{Classifier, FileId, FileTable, HookFilter, Needs, TagOutcome};
    use snp_proc::pool::JobPool;
    use snp_pyre::{PyReError, PyRegex, RegexCache};

    fn send_sync<T: Send + Sync>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    /// The `impl IntoIterator` method of `FileTable` with `snp_git::IndexEntries`-shaped items.
    fn trusted_modes(t: &mut FileTable, entries: Vec<(&[u8], u32)>) {
        t.set_trusted_modes(entries);
    }

    /// The public fields of `HookFilter` and `Classifier`.
    fn fields(table: Arc<FileTable>, files: Arc<PyRegex>, exclude: Arc<PyRegex>) {
        has::<HookFilter>(HookFilter {
            files,
            exclude,
            types: TagSet::new(),
            types_or: TagSet::new(),
            exclude_types: TagSet::new(),
            needs: Needs::default(),
            prefix: RepoPath::new(b"sub"),
        });
        has::<Classifier>(Classifier {
            table,
            kept: vec![FileId(0)],
        });
    }

    #[test]
    fn send_sync_types() {
        send_sync::<FileId>();
        send_sync::<TagOutcome<'static>>();
        send_sync::<Needs>();
        send_sync::<FileTable>();
        send_sync::<HookFilter>();
        send_sync::<Classifier>();
    }

    #[test]
    fn table() {
        let FileId(raw) = FileId(7);
        has::<u32>(raw);
        let exc = PyExc::new("PermissionError", "[Errno 13] Permission denied: 'x'");
        has::<[TagOutcome<'_>; 3]>([
            TagOutcome::Tags(TagSet::new()),
            TagOutcome::Vanished,
            TagOutcome::Unreadable(&exc),
        ]);
        has::<Needs>(Needs {
            stat: true,
            encoding: false,
            shebang: false,
        });
        assert_eq!(
            Needs::default(),
            Needs {
                stat: false,
                encoding: false,
                shebang: false
            }
        );
        has::<fn(Vec<u8>, Vec<(u32, u32)>, &Path) -> FileTable>(FileTable::from_zbuf);
        has::<fn(Vec<OsString>, &Path) -> FileTable>(FileTable::from_args);
        has::<fn(&mut FileTable, Vec<(&[u8], u32)>)>(trusted_modes);
        has::<fn(&mut FileTable)>(FileTable::retain_lexists);
        has::<fn(&FileTable) -> usize>(FileTable::len);
        has::<fn(&FileTable) -> bool>(FileTable::is_empty);
        has::<fn(&FileTable, FileId) -> &[u8]>(FileTable::bytes);
        has::<fn(&FileTable, FileId) -> Cow<'_, OsStr>>(FileTable::os);
        has::<fn(&FileTable, FileId) -> &str>(FileTable::match_view);
        has::<for<'a> fn(&'a FileTable, FileId, &RepoPath) -> Option<Cow<'a, [u8]>>>(
            FileTable::relative_to,
        );
        has::<fn(&FileTable, FileId, Needs) -> TagOutcome<'_>>(FileTable::tags);
        has::<fn(&FileTable, &[FileId], Needs, &JobPool)>(FileTable::prefetch);
    }

    #[test]
    fn filter_and_classifier() {
        has::<fn(Arc<FileTable>, Arc<PyRegex>, Arc<PyRegex>)>(fields);
        has::<
            fn(
                &str,
                &str,
                TagSet,
                TagSet,
                TagSet,
                Option<RepoPath>,
                &RegexCache,
            ) -> Result<HookFilter, PyReError>,
        >(HookFilter::compile);
        has::<fn(Arc<FileTable>, &PyRegex, &PyRegex, &JobPool) -> Result<Classifier, Failure>>(
            Classifier::new,
        );
        has::<fn(&Classifier, &HookFilter, &JobPool) -> Result<Vec<FileId>, Failure>>(
            Classifier::for_hook,
        );
        has::<fn(&Classifier, &[FileId], &HookFilter) -> Result<Vec<FileId>, Failure>>(
            Classifier::by_types,
        );
    }
}
