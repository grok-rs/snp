//! Fuzz target `store_env_dir_name`: environment directory naming.
//! Invariants (ARCHITECTURE §13.5): no panic, no hang > 5 s, ≤ 512 MB RSS, 256 KiB thread stacks.
//!
//! The input is a `language_version` (lossy UTF-8) and a language selector. Checked (ADR-009, D-SEC-01): the
//! environment directory is exactly one component directly inside `<repo>/envs` — never `.`/`..`, never containing
//! a separator, NUL or a drive prefix — named `<ENV_DIR>-<version>` for safe versions and `<ENV_DIR>-h<hex16>`
//! otherwise; the same holds for `tool_dir` and `cache_dir` under `tools-v1`/`cache-v1`. Pure path computation.
//!
//! Owner: WP-22.
#![no_main]

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use snp_core::lang::LanguageId;
use snp_store::{RepoHandle, RepoKey, RepoKind, RepoMeta, Store, StoreLocation, StoreSource};

fn safe(v: &str) -> bool {
    (1..=64).contains(&v.len())
        && v != "."
        && v != ".."
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// `child` is exactly one normal component below `parent`.
fn one_component_below(child: &Path, parent: &Path) -> bool {
    child.parent() == Some(parent)
        && matches!(child.components().next_back(), Some(Component::Normal(_)))
        && child.file_name().is_some_and(|n| {
            let b = n.as_encoded_bytes();
            !b.is_empty() && !b.contains(&0) && !b.contains(&b'/') && !b.contains(&b'\\') && !b.contains(&b':')
        })
}

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let version = String::from_utf8_lossy(rest);
    let lang = LanguageId::ALL[usize::from(sel) % LanguageId::ALL.len()];
    let root = PathBuf::from(if cfg!(windows) { r"C:\s" } else { "/s" });
    let store = Store::new(StoreLocation { path: root.clone(), source: StoreSource::SnpHome });
    let repo_dir = root.join("repos-v1").join("0123456789abcdef0123456789abcdef");
    let handle = RepoHandle {
        key: RepoKey([0; 16]),
        src: repo_dir.join("src"),
        envs: repo_dir.join("envs"),
        meta: RepoMeta { kind: RepoKind::Local, deps: Arc::from([]) },
    };
    let env = store.env_dir(&handle, lang, &version);
    assert!(one_component_below(&env, &handle.envs), "{env:?}");
    let name = env.file_name().and_then(|n| n.to_str()).expect("ASCII name");
    let base = lang.env_dir_base().unwrap_or_else(|| lang.as_str());
    if safe(&version) {
        assert_eq!(name, format!("{base}-{version}"));
    } else {
        let hash = name.strip_prefix(base).and_then(|r| r.strip_prefix("-h")).expect("hashed");
        assert!(hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{name}");
    }
    assert_eq!(env, store.env_dir(&handle, lang, &version), "deterministic");

    let tool = store.tool_dir(&version, &version);
    let tools = root.join("tools-v1");
    assert!(tool.parent().is_some_and(|p| one_component_below(p, &tools)), "{tool:?}");
    assert!(one_component_below(&tool, tool.parent().unwrap_or(&tools)));
    assert!(one_component_below(&store.cache_dir(&version), &root.join("cache-v1")));
});
