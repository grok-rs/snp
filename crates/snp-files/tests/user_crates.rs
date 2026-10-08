//! ADR-061 drift check for file-identify 0.5 (`snp-support`, pinned by `deps.lock`): names every
//! file-identify item of ARCHITECTURE §1.5 with explicit types and exercises the behaviour snp-files
//! relies on, so a breaking change on the branch fails here (the CI drift job) instead of inside a
//! later package.
//!
//! Owner: WP-00 (integration lead); changes only with a `deps.lock` bump.

#[cfg(test)]
mod file_identify_bindings {
    use std::fs::{FileType, Metadata};
    use std::io;
    use std::path::Path;

    use file_identify::{
        ALL_TAGS, Classification, ExecPolicy, FileKind, IDENTIFY_VERSION, IdentifyError,
        MAX_SHEBANG_BYTES, Meta, Pending, TEXT_PROBE_LEN, Tag, TagSet, UnknownTagError,
    };

    fn send_sync<T: Send + Sync>() {}
    fn copy<T: Copy>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    fn names(names: &[&str]) -> TagSet {
        TagSet::from_names(names).unwrap()
    }

    /// The content tier of a pending classification.
    fn pending(p: Pending) {
        has::<TagSet>(p.tags());
        has::<bool>(p.needs_shebang());
        has::<bool>(p.needs_encoding());
        has::<io::Result<TagSet>>(p.finish(io::empty()));
        has::<TagSet>(p.finish_bytes(b"#!/bin/sh\n"));
    }

    /// The path-based entry points (never called on a real file here).
    fn path_based(path: &Path, md: &Metadata) {
        has::<Result<TagSet, IdentifyError>>(file_identify::tags_from_metadata(
            path,
            md,
            ExecPolicy::PythonCompat,
        ));
        has::<Result<Vec<String>, IdentifyError>>(file_identify::parse_shebang_from_file_with(
            path,
            ExecPolicy::ModeBits,
        ));
    }

    #[test]
    fn constants() {
        assert_eq!(IDENTIFY_VERSION, "2.6.20");
        assert_eq!(MAX_SHEBANG_BYTES, 64 * 1024);
        has::<usize>(TEXT_PROBE_LEN);
        let all: &[&str] = &ALL_TAGS;
        assert!(all.contains(&"python") && all.contains(&"text") && all.contains(&"executable"));
    }

    #[test]
    fn tag_sets() {
        copy::<TagSet>();
        copy::<Tag>();
        send_sync::<TagSet>();
        send_sync::<Meta>();
        send_sync::<Classification>();
        let text: Option<Tag> = Tag::from_name("text");
        assert!(text.is_some());
        let unknown: Result<TagSet, UnknownTagError> = TagSet::from_names(["no-such-tag"]);
        assert!(unknown.is_err());
        let py = names(&["python", "text"]);
        let text_only = names(&["text"]);
        has::<fn(TagSet, TagSet) -> TagSet>(TagSet::union);
        has::<fn(TagSet, TagSet) -> TagSet>(TagSet::intersection);
        has::<fn(&TagSet, &TagSet) -> bool>(TagSet::is_subset);
        has::<fn(&TagSet, &TagSet) -> bool>(TagSet::is_disjoint);
        has::<fn(&TagSet, &TagSet) -> bool>(TagSet::intersects);
        assert_eq!(py.union(text_only), py);
        assert_eq!(py.intersection(text_only), text_only);
        assert!(text_only.is_subset(&py));
        assert!(py.intersects(&text_only));
        assert!(names(&["binary"]).is_disjoint(&py));
    }

    #[test]
    fn name_tier() {
        let from_name: TagSet = file_identify::tags_from_filename("src/setup.py");
        assert_eq!(from_name, names(&["python", "text"]));
        let from_bytes: TagSet = file_identify::tags_from_filename(b"src/setup.py".as_slice());
        assert_eq!(from_bytes, from_name);
        has::<fn(&str) -> TagSet>(file_identify::tags_from_interpreter);
        assert!(file_identify::tags_from_interpreter("/usr/bin/python3.12").is_subset(&py3()));
    }

    fn py3() -> TagSet {
        names(&["python", "python3"])
    }

    #[test]
    fn meta_and_classify() {
        assert_eq!(ExecPolicy::default(), ExecPolicy::PythonCompat);
        has::<[ExecPolicy; 2]>([ExecPolicy::PythonCompat, ExecPolicy::ModeBits]);
        has::<[FileKind; 5]>([
            FileKind::Regular,
            FileKind::Directory,
            FileKind::Symlink,
            FileKind::Socket,
            FileKind::Other,
        ]);
        has::<fn(FileType) -> FileKind>(FileKind::from_file_type);
        has::<fn(&Metadata, ExecPolicy) -> Meta>(Meta::from_metadata);
        has::<fn(u32, ExecPolicy) -> Option<Meta>>(Meta::from_git_mode_with);
        has::<fn(Pending)>(pending);
        has::<fn(&Path, &Metadata)>(path_based);
        let regular = Meta::from_git_mode_with(0o100_644, ExecPolicy::ModeBits).unwrap();
        let symlink = Meta::from_git_mode_with(0o120_000, ExecPolicy::ModeBits).unwrap();
        assert_eq!(symlink.kind(), FileKind::Symlink);
        match file_identify::classify(regular, b"src/setup.py".as_slice()) {
            Classification::Done(tags) => assert!(tags.is_superset(&names(&["python", "text"]))),
            Classification::NeedsContent(_) => panic!("a known name needs no content"),
        }
        match file_identify::classify(regular, "notes.unknown-ext") {
            Classification::NeedsContent(p) => {
                assert!(p.needs_encoding());
                assert!(p.finish_bytes(b"hello\n").is_superset(&names(&["text"])));
            }
            Classification::Done(_) => panic!("an unknown name needs content"),
        }
    }
}
