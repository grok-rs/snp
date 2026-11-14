//! Invariants of the generated tables (identify's `tests/extensions_test.py`).

#[cfg(test)]
mod extensions {
    use file_identify::extensions::{
        EXTENSIONS, EXTENSIONS_NEED_BINARY_CHECK, NAMES, get_extension_tags,
        get_extensions_need_binary_check_tags, get_name_tags,
    };
    use file_identify::interpreters::{INTERPRETERS, get_interpreter_tags};
    use file_identify::tags::ENCODING_TAGS;
    use file_identify::{TagSet, tags_from_filename};

    #[test]
    fn extensions_have_binary_or_text() {
        for (ext, tags) in EXTENSIONS.iter() {
            assert_eq!(tags.intersection(ENCODING_TAGS).len(), 1, "{ext}: {tags}");
        }
    }

    #[test]
    fn names_have_binary_or_text() {
        for (name, tags) in NAMES.iter() {
            assert_eq!(tags.intersection(ENCODING_TAGS).len(), 1, "{name}: {tags}");
        }
    }

    #[test]
    fn need_binary_check_do_not_specify_text_binary() {
        for (ext, tags) in EXTENSIONS_NEED_BINARY_CHECK.iter() {
            assert!(tags.is_disjoint(&ENCODING_TAGS), "{ext}: {tags}");
        }
    }

    #[test]
    fn mutually_exclusive_check_types() {
        for ext in EXTENSIONS_NEED_BINARY_CHECK.keys() {
            assert!(!EXTENSIONS.contains_key(ext), "{ext}");
        }
    }

    #[test]
    fn interpreters_have_no_encoding() {
        for (name, tags) in INTERPRETERS.iter() {
            assert!(
                !tags.is_empty() && tags.is_disjoint(&ENCODING_TAGS),
                "{name}: {tags}"
            );
        }
    }

    #[test]
    fn table_sizes_match_identify_2_6_20() {
        assert_eq!(EXTENSIONS.len(), 343);
        assert_eq!(EXTENSIONS_NEED_BINARY_CHECK.len(), 5);
        assert_eq!(NAMES.len(), 96);
        assert_eq!(INTERPRETERS.len(), 25);
    }

    #[test]
    fn union_of_tables_is_all_tags() {
        let mut union =
            file_identify::tags::TYPE_TAGS | file_identify::tags::MODE_TAGS | ENCODING_TAGS;
        for table in [
            &EXTENSIONS,
            &EXTENSIONS_NEED_BINARY_CHECK,
            &NAMES,
            &INTERPRETERS,
        ] {
            for tags in table.values() {
                union |= tags;
            }
        }
        assert_eq!(union, TagSet::ALL);
    }

    #[test]
    fn invented_names_are_gone() {
        // file-identify 0.4 shipped these; identify never had them.
        for name in [
            ".eslintrc",
            ".eslintrc.js",
            ".eslintrc.json",
            ".eslintrc.yaml",
            ".eslintrc.yml",
            ".prettierrc",
            ".prettierrc.json",
            ".prettierrc.toml",
            ".prettierrc.yaml",
            ".prettierrc.yml",
            ".stylintrc",
            "BUILD.bazel",
            "WORKSPACE.bazel",
            "composer.json",
            "composer.lock",
            "package-lock.json",
            "package.json",
            "yarn.lock",
        ] {
            assert!(!NAMES.contains_key(name), "{name}");
        }
        assert_eq!(tags_from_filename("yarn.lock").to_string(), "");
        assert_eq!(tags_from_filename(".eslintrc").to_string(), "");
        assert_eq!(tags_from_filename("composer.lock").to_string(), "");
        assert_eq!(tags_from_filename("package.json").to_string(), "json, text");
    }

    #[test]
    fn previously_missing_entries_exist() {
        for (name, expected) in [
            ("pyproject.toml", "pyproject, text, toml"),
            ("uv.lock", "text, toml, uv-lock"),
            ("Brewfile", "ruby, text"),
            ("SConstruct", "scons, text"),
            ("PKGBUILD", "alpm, bash, pkgbuild, shell, text"),
            ("x.cmd", "batch, text"),
            ("x.nu", "nushell, text"),
            ("x.tif", "binary, image, tiff"),
            ("x.xcprivacy", "plist, xcodebuild, xcprivacy"),
        ] {
            assert_eq!(tags_from_filename(name).to_string(), expected, "{name}");
        }
        assert_eq!(get_interpreter_tags("nu").to_string(), "nushell");
    }

    #[test]
    fn compat_helpers() {
        assert_eq!(get_extension_tags("py").to_string(), "python, text");
        assert!(get_extension_tags("PY").is_empty());
        assert_eq!(
            get_extensions_need_binary_check_tags("plist").to_string(),
            "plist"
        );
        assert_eq!(get_name_tags("Makefile").to_string(), "makefile, text");
        assert!(get_name_tags("makefile.am").is_empty());
    }
}
