//! `check_support` and `Language::support` of the group-2 languages: the verbatim `assert_version_default` /
//! `assert_no_additional_deps` texts in both brands, checked before any command or filesystem mutation (04b §3.1,
//! §5.1).

use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_lang::{VersionSupport, backend, check_support};

use super::langs::GROUP2;
use super::util::Fx;

/// (language, `DefaultOnly`, deps allowed) as pre-commit 4.6.2's `install_environment` asserts them.
const TABLE: [(LanguageId, bool, bool); 11] = [
    (LanguageId::Conda, true, true),
    (LanguageId::Coursier, true, true),
    (LanguageId::Dart, true, true),
    (LanguageId::Docker, true, false),
    (LanguageId::Dotnet, true, false),
    (LanguageId::Haskell, true, true),
    (LanguageId::Julia, false, true),
    (LanguageId::Lua, true, true),
    (LanguageId::Perl, true, true),
    (LanguageId::R, true, true),
    (LanguageId::Swift, true, false),
];

#[test]
fn lang_group2_support_table() {
    assert_eq!(TABLE.map(|t| t.0), GROUP2);
    for (id, default_only, deps) in TABLE {
        let s = backend(id).support();
        assert_eq!(s.deps, deps, "{id:?}");
        assert_eq!(
            s.version,
            if default_only {
                VersionSupport::DefaultOnly
            } else {
                VersionSupport::Ignored
            },
            "{id:?}"
        );
    }
}

#[test]
fn lang_group2_check_support_texts_in_both_brands() {
    for (id, default_only, deps) in TABLE {
        for brand in [Brand::PreCommit, Brand::Snp] {
            let prog = brand.prog();
            let lang = id.as_str();
            let r = check_support(id, "5.9", &[], brand);
            if default_only {
                let e = r.unwrap_err();
                assert_eq!(
                    e.to_string(),
                    format!(
                        "for now, {prog} requires system-installed {lang} -- you selected `language_version: 5.9`"
                    )
                );
                assert_eq!(Failure::from(e).exit_code(), 3);
            } else {
                r.unwrap();
            }
            let d: Vec<Arc<str>> = vec![Arc::from("foo"), Arc::from("it's")];
            let r = check_support(id, "default", &d, brand);
            if deps {
                r.unwrap();
            } else {
                assert_eq!(
                    r.unwrap_err().to_string(),
                    format!(
                        "for now, {prog} does not support additional_dependencies for {lang} -- you selected \
                         `additional_dependencies: ['foo', \"it's\"]`"
                    )
                );
            }
        }
    }
}

#[test]
fn lang_group2_install_asserts_before_touching_anything() {
    // A hostile version never reaches the filesystem or a command (04b §5.1).
    for (id, default_only, _) in TABLE {
        if !default_only {
            continue;
        }
        let fx = Fx::new(&[]);
        let e = fx.install_v(id, "../../victim", &[]).unwrap_err();
        assert_eq!(e.py_type(), "AssertionError", "{id:?}");
        assert!(fx.t.runner.calls().is_empty(), "{id:?}");
        assert!(!fx.envs.join("..").join("victim").exists());
        assert_eq!(std::fs::read_dir(&fx.envs).unwrap().count(), 0, "{id:?}");
    }
}
