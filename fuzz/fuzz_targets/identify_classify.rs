//! `classify` + `Pending`: no panic; finishing from bytes and from a reader
//! (split into arbitrary reads, with interruptions) agree; every file gets
//! exactly one encoding tag; `is_text` agrees with `is_text_bytes`.
#![no_main]

mod identify_common;

use file_identify::{
    Classification, FileInfo, FileKind, Meta, Tag, classify, is_text, is_text_bytes, tags_from_info,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (kind_byte, rest) = identify_common::control(data);
    let (control, rest) = identify_common::control(rest);
    let (name_len, rest) = identify_common::control(rest);
    let (name, content) = rest.split_at(usize::from(name_len).min(rest.len()));
    let kind = match kind_byte % 5 {
        0 => FileKind::Directory,
        1 => FileKind::Symlink,
        2 => FileKind::Socket,
        3 => FileKind::Other,
        _ => FileKind::Regular,
    };
    let meta = Meta::new(kind, kind_byte & 0x80 != 0);

    let tags = match classify(meta, name) {
        Classification::Done(tags) => tags,
        Classification::NeedsContent(pending) => {
            assert_eq!(kind, FileKind::Regular);
            let tags = pending.finish_bytes(content);
            let streamed = pending
                .finish(identify_common::Chunked::new(content, control))
                .expect("in-memory read");
            assert_eq!(streamed, tags);
            assert!(tags.is_superset(&pending.tags()));
            tags
        }
    };
    match kind {
        FileKind::Directory => assert_eq!(tags.to_string(), "directory"),
        FileKind::Symlink => assert_eq!(tags.to_string(), "symlink"),
        FileKind::Socket => assert_eq!(tags.to_string(), "socket"),
        _ => {
            assert!(tags.contains(Tag::FILE));
            assert_ne!(
                tags.contains(Tag::EXECUTABLE),
                tags.contains(Tag::NON_EXECUTABLE)
            );
            assert_ne!(tags.contains(Tag::TEXT), tags.contains(Tag::BINARY));
        }
    }
    if let Ok(filename) = std::str::from_utf8(name) {
        let info = FileInfo {
            filename,
            file_kind: kind,
            is_executable: meta.is_executable(),
            content: Some(content),
        };
        assert_eq!(tags_from_info(&info), tags);
    }

    let text = is_text(identify_common::Chunked::new(content, control)).expect("in-memory read");
    assert_eq!(text, is_text_bytes(content));
});
