//! `check_support`: the verbatim `assert_version_default` / `assert_no_additional_deps` texts in both brands, the
//! per-language table, and no filesystem mutation before them (WBS WP-16 acceptance; 04b §5.1).

use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_lang::{LangError, VersionSupport, backend, check_support};

/// pre-commit 4.6.2's `install_environment` assertions: (language, asserts the version, asserts no deps).
const TABLE: [(LanguageId, bool, bool); 16] = [
    (LanguageId::Conda, true, false),
    (LanguageId::Coursier, true, false),
    (LanguageId::Dart, true, false),
    (LanguageId::Docker, true, true),
    (LanguageId::Dotnet, true, true),
    (LanguageId::Haskell, true, false),
    (LanguageId::Julia, false, false),
    (LanguageId::Lua, true, false),
    (LanguageId::Node, false, false),
    (LanguageId::Perl, true, false),
    (LanguageId::Python, false, false),
    (LanguageId::R, true, false),
    (LanguageId::Ruby, false, false),
    (LanguageId::Rust, false, false),
    (LanguageId::Swift, true, true),
    (LanguageId::Golang, false, false),
];

fn deps(xs: &[&str]) -> Vec<Arc<str>> {
    xs.iter().map(|d| Arc::from(*d)).collect()
}

#[test]
fn texts_in_both_brands_for_every_language() {
    for (id, version_default, no_deps) in TABLE {
        for (brand, tool) in [(Brand::PreCommit, "pre-commit"), (Brand::Snp, "snp")] {
            let r = check_support(id, "1.0", &[], brand);
            if version_default {
                let e = r.unwrap_err();
                assert_eq!(
                    e.to_string(),
                    format!(
                        "for now, {tool} requires system-installed {} -- you selected `language_version: 1.0`",
                        id.as_str()
                    )
                );
                // AssertionError, exit 3.
                let f = Failure::from(e);
                assert_eq!(f.exit_code(), 3);
                let Failure::Exc { exc, .. } = f else {
                    panic!()
                };
                assert_eq!(exc.ty, "AssertionError");
            } else {
                r.unwrap();
            }
            let r = check_support(id, "default", &deps(&["foo", "bar==1.0"]), brand);
            if no_deps {
                assert_eq!(
                    r.unwrap_err().to_string(),
                    format!(
                        "for now, {tool} does not support additional_dependencies for {} -- you selected \
                         `additional_dependencies: ['foo', 'bar==1.0']`",
                        id.as_str()
                    )
                );
            } else {
                r.unwrap();
            }
            check_support(id, "default", &[], brand).unwrap();
        }
    }
}

#[test]
fn version_is_checked_before_deps() {
    let e = check_support(LanguageId::Swift, "5.9", &deps(&["x"]), Brand::PreCommit).unwrap_err();
    assert!(e.to_string().contains("language_version: 5.9"), "{e}");
}

/// pre-commit formats the version with `str()` and the deps with the Python list `repr()`.
#[test]
fn python_formatting_of_values() {
    let e = check_support(LanguageId::Perl, "", &[], Brand::PreCommit).unwrap_err();
    assert_eq!(
        e.to_string(),
        "for now, pre-commit requires system-installed perl -- you selected `language_version: `"
    );
    let e = check_support(
        LanguageId::Dotnet,
        "default",
        &deps(&["it's", "a\\b", "é", "x\ny"]),
        Brand::Snp,
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "for now, snp does not support additional_dependencies for dotnet -- you selected \
         `additional_dependencies: [\"it's\", 'a\\\\b', 'é', 'x\\ny']`"
    );
    // The traversal version of 04b §5.1 is reported, never used as a path.
    let e = check_support(LanguageId::Perl, "/../../../victim", &[], Brand::PreCommit).unwrap_err();
    assert!(matches!(e, LangError::Assertion(_)));
}

/// Env-less languages are guarded at hook resolution (with the hook id); `check_support` lets them pass.
#[test]
fn envless_languages_pass() {
    for id in [
        LanguageId::DockerImage,
        LanguageId::Fail,
        LanguageId::Pygrep,
        LanguageId::Unsupported,
        LanguageId::UnsupportedScript,
    ] {
        check_support(id, "3.0", &deps(&["x"]), Brand::PreCommit).unwrap();
        let s = backend(id).support();
        assert_eq!(s.version, VersionSupport::DefaultOnly);
        assert!(!s.deps);
    }
}

/// No filesystem mutation happens before the assertions: they hold with a read-only prefix and leave it as it was
/// (pre-commit `rmtree`s the env directory first, 04b §5.1).
#[cfg(unix)]
#[test]
fn no_filesystem_mutation_before_the_texts() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let prefix = d.path().join("repo");
    std::fs::create_dir_all(prefix.join("perl_env-default")).unwrap();
    std::fs::write(prefix.join("perl_env-default/keep"), b"keep").unwrap();
    let victim = d.path().join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("file"), b"victim").unwrap();
    std::fs::set_permissions(&prefix, std::fs::Permissions::from_mode(0o555)).unwrap();
    let before: Vec<_> = std::fs::read_dir(&prefix)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    for (id, _, _) in TABLE {
        let _ignored: Result<(), LangError> =
            check_support(id, "/../../../victim", &deps(&["x"]), Brand::Snp);
    }
    let after: Vec<_> = std::fs::read_dir(&prefix)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert_eq!(
        std::fs::read(prefix.join("perl_env-default/keep")).unwrap(),
        b"keep"
    );
    assert_eq!(std::fs::read(victim.join("file")).unwrap(), b"victim");
    std::fs::set_permissions(&prefix, std::fs::Permissions::from_mode(0o755)).unwrap();
}
