//! The members every env-less language takes from `lang_base` (04a §8; `fail.py`, `pygrep.py`, `unsupported.py`,
//! `unsupported_script.py`, `docker_image.py`).

use snp_core::lang::LanguageId;
use snp_lang::backend;

const ENVLESS: [LanguageId; 5] = [
    LanguageId::Fail,
    LanguageId::Pygrep,
    LanguageId::Unsupported,
    LanguageId::UnsupportedScript,
    LanguageId::DockerImage,
];

#[test]
fn the_registry_maps_each_envless_language_to_its_backend() {
    for id in ENVLESS {
        assert_eq!(backend(id).id(), id);
        assert_eq!(id.env_dir_base(), None, "{id:?} has no ENVIRONMENT_DIR");
    }
    // The config aliases resolve to the same backends (`clientlib._translate_language`).
    assert_eq!(
        LanguageId::from_config("system"),
        Some(LanguageId::Unsupported)
    );
    assert_eq!(
        LanguageId::from_config("script"),
        Some(LanguageId::UnsupportedScript)
    );
}

#[cfg(feature = "testing")]
#[test]
fn lang_base_members() {
    use snp_core::env::Env;
    use snp_lang::harness::TestHost;
    use snp_lang::{EnvCtx, Health, InstallCtx, LangError};
    let dir = tempfile::tempdir().unwrap();
    let d = &dir;
    let t = TestHost::new(d.path(), Env::from_pairs([("PATH", "/usr/bin")]));
    let host = t.host();
    for id in ENVLESS {
        let lang = backend(id);
        assert_eq!(&*lang.default_version(&host).unwrap(), "default", "{id:?}");
        let cx = EnvCtx {
            host: &host,
            prefix: d.path(),
            version: "default",
            env_dir: &d.path().join("never"),
        };
        assert_eq!(lang.health(&cx, None).unwrap(), Health::Healthy);
        assert!(lang.activate(&cx).unwrap().is_empty());
        let e = lang
            .install(&InstallCtx {
                cx,
                deps: &[],
                is_local: true,
            })
            .unwrap_err();
        assert!(
            matches!(&e, LangError::Assertion(m) if m == "This language is not installable"),
            "{e:?}"
        );
        assert!(!d.path().join("never").exists());
    }
    // Nothing above ran a process.
    assert!(t.runner.calls().is_empty());
}
