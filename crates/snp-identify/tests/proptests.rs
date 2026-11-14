//! Property tests: no panics on arbitrary input, and agreement between the
//! different entry points and with simple models.

#[cfg(test)]
mod proptests {
    use std::collections::BTreeSet;
    use std::io::{self, Read};

    use file_identify::extensions::{EXTENSIONS, NAMES};
    use file_identify::{
        ALL_TAGS, Classification, FileKind, Meta, TAG_COUNT, Tag, TagSet, classify, is_text,
        is_text_bytes, parse_shebang, parse_shebang_bytes, shlex_split, tags_from_filename,
        tags_from_interpreter,
    };
    use proptest::prelude::*;

    fn tag_set() -> impl Strategy<Value = BTreeSet<usize>> {
        proptest::collection::btree_set(0..TAG_COUNT, 0..12)
    }

    fn to_set(ids: &BTreeSet<usize>) -> TagSet {
        ids.iter()
            .map(|&i| Tag::from_id(i).expect("valid id"))
            .collect()
    }

    fn ids(set: TagSet) -> BTreeSet<usize> {
        set.iter().map(Tag::id).collect()
    }

    /// Returns the input in pieces of the given sizes (cycled).
    struct Chunked<'a> {
        data: &'a [u8],
        sizes: Vec<usize>,
        n: usize,
    }

    impl Read for Chunked<'_> {
        #[expect(
            clippy::disallowed_methods,
            reason = "test double: `k` is at most both lengths"
        )]
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let want = self
                .sizes
                .get(self.n % self.sizes.len().max(1))
                .copied()
                .unwrap_or(1)
                .max(1);
            self.n += 1;
            let k = want.min(buf.len()).min(self.data.len());
            buf[..k].copy_from_slice(&self.data[..k]);
            self.data = &self.data[k..];
            Ok(k)
        }
    }

    fn shebangish() -> impl Strategy<Value = Vec<u8>> {
        let piece = prop_oneof![
            Just(b"#!".to_vec()),
            Just(b"/usr/bin/env ".to_vec()),
            Just(b"nix-shell".to_vec()),
            Just(b" -i ".to_vec()),
            Just(b"-S ".to_vec()),
            Just(b"\n".to_vec()),
            Just(b"\"".to_vec()),
            Just(b"'".to_vec()),
            Just(b"\\".to_vec()),
            Just(b"python3".to_vec()),
            proptest::collection::vec(any::<u8>(), 0..4),
        ];
        proptest::collection::vec(piece, 0..12).prop_map(|v| v.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn tagset_matches_btreeset_model(a in tag_set(), b in tag_set()) {
            let (sa, sb) = (to_set(&a), to_set(&b));
            prop_assert_eq!(ids(sa | sb), a.union(&b).copied().collect::<BTreeSet<_>>());
            prop_assert_eq!(ids(sa & sb), a.intersection(&b).copied().collect::<BTreeSet<_>>());
            prop_assert_eq!(ids(sa - sb), a.difference(&b).copied().collect::<BTreeSet<_>>());
            prop_assert_eq!(ids(sa ^ sb), a.symmetric_difference(&b).copied().collect::<BTreeSet<_>>());
            prop_assert_eq!(sa.is_subset(&sb), a.is_subset(&b));
            prop_assert_eq!(sa.is_superset(&sb), a.is_superset(&b));
            prop_assert_eq!(sa.is_disjoint(&sb), a.is_disjoint(&b));
            prop_assert_eq!(sa.intersects(&sb), !a.is_disjoint(&b));
            prop_assert_eq!(sa.len(), a.len());
            prop_assert_eq!(sa.iter().len(), a.len());
            prop_assert_eq!(sa.iter().rev().map(Tag::id).collect::<Vec<_>>(), a.iter().rev().copied().collect::<Vec<_>>());
            prop_assert_eq!((!sa).len(), TAG_COUNT - a.len());
            prop_assert_eq!(sa.first().map(Tag::id), a.first().copied());
            prop_assert_eq!(sa.last().map(Tag::id), a.last().copied());
        }

        #[test]
        fn tagset_display_round_trips(a in tag_set()) {
            let set = to_set(&a);
            prop_assert_eq!(set.to_string().parse::<TagSet>(), Ok(set));
            let names: Vec<&str> = a.iter().map(|&i| ALL_TAGS[i]).collect();
            prop_assert_eq!(TagSet::from_names(&names), Ok(set));
            for name in names {
                prop_assert!(set.contains(name));
            }
        }

        #[test]
        fn matches_types_is_precommit_filter(t in tag_set(), ty in tag_set(), or in tag_set(), ex in tag_set()) {
            let model = ty.is_subset(&t) && (or.is_empty() || !or.is_disjoint(&t)) && ex.is_disjoint(&t);
            prop_assert_eq!(to_set(&t).matches_types(&to_set(&ty), &to_set(&or), &to_set(&ex)), model);
        }

        #[test]
        fn tag_lookup_never_false_positive(s in "\\PC{0,24}") {
            match Tag::from_name(&s) {
                Some(tag) => prop_assert_eq!(tag.as_str(), s.as_str()),
                None => prop_assert!(!ALL_TAGS.contains(&s.as_str())),
            }
        }

        #[test]
        fn table_lookup_agrees_with_scan(s in "[a-zA-Z0-9.+#_-]{0,12}") {
            for table in [&EXTENSIONS, &NAMES] {
                let scanned = table.iter().find(|(k, _)| *k == s).map(|(_, v)| v);
                prop_assert_eq!(table.get(&s), scanned);
            }
        }

        #[test]
        fn filename_never_panics(name in proptest::collection::vec(any::<u8>(), 0..64)) {
            let tags = tags_from_filename(&name);
            prop_assert!(tags.is_subset(&TagSet::ALL));
        }

        #[test]
        fn filename_ignores_directories(dir in "[a-z.]{0,8}", name in "[A-Za-z0-9._-]{0,16}") {
            let joined = format!("{dir}/{name}");
            prop_assert_eq!(tags_from_filename(&joined), tags_from_filename(&name));
        }

        #[test]
        fn ascii_extension_is_case_insensitive(stem in "[a-z]{1,6}", key in proptest::sample::select(EXTENSIONS.keys().collect::<Vec<_>>())) {
            let lower = tags_from_filename(format!("{stem}.{key}"));
            let upper = tags_from_filename(format!("{stem}.{}", key.to_ascii_uppercase()));
            prop_assert_eq!(lower, upper);
            prop_assert!(EXTENSIONS.get(key).is_some_and(|t| lower.is_superset(&t)));
        }

        #[test]
        fn interpreter_never_panics(s in "\\PC{0,32}") {
            prop_assert!(tags_from_interpreter(&s).is_subset(&TagSet::ALL));
        }

        #[test]
        fn shlex_never_panics(s in "\\PC{0,64}") {
            drop(shlex_split(&s));
        }

        #[test]
        fn shlex_plain_words(words in proptest::collection::vec("[a-zA-Z0-9_/.=-]{1,8}", 0..6)) {
            prop_assert_eq!(shlex_split(&words.join(" ")), Ok(words.clone()));
            let quoted: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
            prop_assert_eq!(shlex_split(&quoted.join("\t")), Ok(words));
        }

        #[test]
        fn shebang_reader_agrees_with_bytes(data in shebangish(), sizes in proptest::collection::vec(1usize..9, 1..4)) {
            let expected = parse_shebang_bytes(&data);
            let chunked = Chunked { data: &data, sizes, n: 0 };
            prop_assert_eq!(parse_shebang(chunked).ok(), Some(expected.clone()));
            for token in &expected {
                prop_assert!(!token.is_empty() || data.contains(&b'\'') || data.contains(&b'"'));
            }
        }

        #[test]
        fn is_text_matches_model(data in proptest::collection::vec(any::<u8>(), 0..1100), sizes in proptest::collection::vec(1usize..300, 1..4)) {
            let model = data.iter().take(1024).all(|&b| matches!(b, 7..=13 | 27 | 0x20..=0x7e | 0x80..=0xff));
            prop_assert_eq!(is_text_bytes(&data), model);
            prop_assert_eq!(is_text(Chunked { data: &data, sizes, n: 0 }).ok(), Some(model));
        }

        #[test]
        fn finish_agrees_across_entry_points(
            data in shebangish(),
            executable in any::<bool>(),
            name in proptest::sample::select(vec!["noext", "x.plist", "script", "f.ppm", "x.py"]),
            sizes in proptest::collection::vec(1usize..64, 1..3),
        ) {
            let meta = Meta::new(FileKind::Regular, executable);
            let via_info = file_identify::tags_from_info(&file_identify::FileInfo {
                filename: name,
                file_kind: FileKind::Regular,
                is_executable: executable,
                content: Some(&data),
            });
            match classify(meta, name) {
                Classification::Done(tags) => {
                    prop_assert!(tags.has_encoding());
                    prop_assert_eq!(tags, via_info);
                }
                Classification::NeedsContent(pending) => {
                    let from_bytes = pending.finish_bytes(&data);
                    prop_assert!(from_bytes.has_encoding());
                    prop_assert_eq!(from_bytes, via_info);
                    let from_reader = pending.finish(Chunked { data: &data, sizes, n: 0 }).ok();
                    prop_assert_eq!(from_reader, Some(from_bytes));
                }
            }
        }
    }
}
