//! Validated + defaulted `cfgv::Value` → the typed model (ARCHITECTURE §2.1). Runs only on values that passed the
//! schema, so every lookup has a well-typed answer; the conversions still never panic and fall back to
//! pre-commit's defaults on anything unexpected.
//!
//! Owner: WP-21.

use std::sync::{Arc, LazyLock};

use cfgv::{Cx, Map, Validator, Value};
use file_identify::TagSet;
use indexmap::IndexMap;
use snp_core::lang::LanguageId;
use snp_core::stage::{HookType, Stage, StageSet};

use crate::ext::{self, HookExt};
use crate::items::{BUILTIN, DEFAULT, LOCAL, META};
use crate::schema::HOOK_KEYS;
use crate::{Config, ConfigHook, ConfigSource, ExtMode, HookDef, Manifest, RepoConfig};

/// Shared `Arc<str>`s for the values almost every hook carries (ARCHITECTURE §2.1 memory shape).
static COMMON: LazyLock<[Arc<str>; 5]> = LazyLock::new(|| {
    [
        Arc::from(""),
        Arc::from("^$"),
        Arc::from(DEFAULT),
        Arc::from("0"),
        Arc::from("file"),
    ]
});

/// `s` as an `Arc<str>`, shared for the common defaults.
pub(crate) fn arc(s: &str) -> Arc<str> {
    COMMON
        .iter()
        .find(|c| &***c == s)
        .map_or_else(|| Arc::from(s), Arc::clone)
}

pub(crate) fn get<'a>(m: &'a Map, k: &str) -> Option<&'a Value> {
    m.get(&Value::from(k))
}

pub(crate) fn get_str<'a>(m: &'a Map, k: &str) -> Option<&'a str> {
    get(m, k).and_then(Value::as_str)
}

fn str_or(m: &Map, k: &str, default: &str) -> Arc<str> {
    arc(get_str(m, k).unwrap_or(default))
}

fn bool_or(m: &Map, k: &str, default: bool) -> bool {
    get(m, k).and_then(Value::as_bool).unwrap_or(default)
}

pub(crate) fn str_list(v: &Value) -> Arc<[Arc<str>]> {
    v.as_array()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(arc)
        .collect()
}

pub(crate) fn tag_set(v: &Value) -> TagSet {
    TagSet::from_names(
        v.as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str),
    )
    .unwrap_or_default()
}

/// A stage list (legacy names already transformed by `apply_defaults`) → the set; unknown names are ignored.
pub(crate) fn stage_set(v: &Value) -> StageSet {
    v.as_array()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|s| Stage::parse(Stage::transform_legacy(s)))
        .fold(StageSet::EMPTY, StageSet::with)
}

/// `str(key)` for messages.
pub(crate) fn key_str(k: &Value) -> String {
    k.try_to_string().unwrap_or_else(|_| k.to_string())
}

/// `sorted(keys)` rendered with `str()`, in Python's order: cfgv-rs's `py_sort` (reached through
/// `WarnAdditionalKeys`, which hands its callback the extra keys sorted), the order of the root and repo
/// warnings. Strings sort by code point, numbers and booleans numerically, other types by type rank (pre-commit
/// would crash on a non-string key, D-CRASH-02); keys a Python dict collapses (`1`, `true`) are one key.
pub(crate) fn py_sorted_keys(keys: impl IntoIterator<Item = Value>) -> Vec<Arc<str>> {
    static SORTER: LazyLock<Validator> = LazyLock::new(|| {
        Validator::warn_additional_keys(
            Vec::new(),
            |extra: &[Value], _keys: &[Value], _dct: &Map, cx: &mut Cx<'_>| {
                for k in extra {
                    cx.warn(key_str(k));
                }
            },
        )
    });
    let map: Map = keys.into_iter().map(|k| (k, Value::Null)).collect();
    if map.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // A `WarnAdditionalKeys` check never fails.
    if SORTER.check(&map, &mut Cx::collect(&mut out)).is_err() {
        return Vec::new();
    }
    out.into_iter().map(|s| Arc::from(s.as_str())).collect()
}

/// The keys of `m` that are neither one of the 22 hook keys nor (when `ext` is enabled) an extension key, as
/// `str()` values in `sorted()` order (`hook.py:49-57`).
pub(crate) fn extra_keys(m: &Map, ext: ExtMode) -> Vec<Arc<str>> {
    py_sorted_keys(
        m.keys()
            .filter(|k| !k.as_str().is_some_and(|s| HOOK_KEYS.contains(&s)))
            .filter(|k| ext == ExtMode::Strict || !ext::is_hook_ext_key(k))
            .cloned(),
    )
}

fn language(m: &Map) -> LanguageId {
    get_str(m, "language")
        .and_then(LanguageId::from_config)
        .unwrap_or(LanguageId::Unsupported)
}

/// A fully defaulted manifest-style hook dict (`MANIFEST_HOOK_DICT`, `LOCAL_HOOK_DICT`, `META_HOOK_DICT`) →
/// `HookDef` (`hook.py` fields).
pub(crate) fn hook_def(m: &Map, ext_mode: ExtMode) -> HookDef {
    let stages = get(m, "stages").map_or(StageSet::EMPTY, stage_set);
    HookDef {
        id: str_or(m, "id", ""),
        name: str_or(m, "name", ""),
        entry: str_or(m, "entry", ""),
        language: language(m),
        alias: str_or(m, "alias", ""),
        files: str_or(m, "files", ""),
        exclude: str_or(m, "exclude", "^$"),
        types: get(m, "types")
            .map_or_else(|| TagSet::from_names(["file"]).unwrap_or_default(), tag_set),
        types_or: get(m, "types_or").map_or_default(tag_set),
        exclude_types: get(m, "exclude_types").map_or_default(tag_set),
        additional_dependencies: get(m, "additional_dependencies")
            .map_or_else(|| Arc::from([]), str_list),
        args: get(m, "args").map_or_else(|| Arc::from([]), str_list),
        always_run: bool_or(m, "always_run", false),
        fail_fast: bool_or(m, "fail_fast", false),
        pass_filenames: bool_or(m, "pass_filenames", true),
        description: str_or(m, "description", ""),
        language_version: str_or(m, "language_version", DEFAULT),
        log_file: str_or(m, "log_file", ""),
        minimum_pre_commit_version: str_or(m, "minimum_pre_commit_version", "0"),
        require_serial: bool_or(m, "require_serial", false),
        stages_set: !stages.is_empty(),
        stages,
        verbose: bool_or(m, "verbose", false),
        extra_keys: extra_keys(m, ext_mode),
        ext: match ext_mode {
            ExtMode::Enabled => ext::parse_hook_ext(m),
            ExtMode::Strict => HookExt::default(),
        },
    }
}

/// A config-level hook dict of a remote/builtin repo → `ConfigHook` (extension keys parsed and, when enabled,
/// removed from `raw` so the merge sees only hook keys).
pub(crate) fn config_hook(m: &Map, ext_mode: ExtMode) -> ConfigHook {
    let (raw, ext) = match ext_mode {
        ExtMode::Strict => (m.clone(), HookExt::default()),
        ExtMode::Enabled => {
            let raw: Map = m
                .iter()
                .filter(|(k, _)| !ext::is_hook_ext_key(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (raw, ext::parse_hook_ext(m))
        }
    };
    ConfigHook {
        id: str_or(m, "id", ""),
        raw,
        ext,
    }
}

fn hook_maps(repo: &Map) -> impl Iterator<Item = &Map> {
    get(repo, "hooks")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_map)
}

/// One validated `repos` entry → `RepoConfig`.
pub(crate) fn repo_config(repo: &Map, ext_mode: ExtMode) -> RepoConfig {
    let url = get_str(repo, "repo").unwrap_or_default();
    match url {
        LOCAL => RepoConfig::Local {
            hooks: hook_maps(repo).map(|h| hook_def(h, ext_mode)).collect(),
        },
        META => RepoConfig::Meta {
            hooks: hook_maps(repo).map(|h| hook_def(h, ext_mode)).collect(),
        },
        BUILTIN if ext_mode == ExtMode::Enabled => RepoConfig::Builtin {
            hooks: hook_maps(repo).map(|h| config_hook(h, ext_mode)).collect(),
        },
        _ => RepoConfig::Remote {
            repo: Arc::from(url),
            rev: Arc::from(get_str(repo, "rev").unwrap_or_default()),
            hooks: hook_maps(repo).map(|h| config_hook(h, ext_mode)).collect(),
        },
    }
}

/// The validated + defaulted config → `Config`.
pub(crate) fn config(source: ConfigSource, raw: Value, ext_mode: ExtMode) -> Config {
    let empty = Map::new();
    let m = raw.as_map().unwrap_or(&empty);
    let repos = get(m, "repos")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_map)
        .map(|r| repo_config(r, ext_mode))
        .collect();
    let default_install_hook_types = get(m, "default_install_hook_types")
        .and_then(Value::as_array)
        .map_or_else(
            || vec![HookType::PreCommit],
            |l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .filter_map(HookType::parse)
                    .collect()
            },
        );
    let mut default_language_version = IndexMap::new();
    for lang in LanguageId::ALL {
        let v = get(m, "default_language_version")
            .and_then(Value::as_map)
            .and_then(|d| get_str(d, lang.as_str()))
            .unwrap_or(DEFAULT);
        default_language_version.insert(lang, arc(v));
    }
    let default_stages = get(m, "default_stages").map_or(StageSet::ALL, stage_set);
    let root_ext = match ext_mode {
        ExtMode::Enabled => ext::parse_root_ext(m),
        ExtMode::Strict => ext::RootExt::default(),
    };
    Config {
        source,
        repos,
        default_install_hook_types,
        default_language_version,
        default_stages,
        files: str_or(m, "files", ""),
        exclude: str_or(m, "exclude", "^$"),
        fail_fast: bool_or(m, "fail_fast", false),
        minimum_pre_commit_version: str_or(m, "minimum_pre_commit_version", "0"),
        root_ext,
        raw,
    }
}

/// The validated + defaulted manifest → `Manifest` (extension keys are unknown keys in a manifest).
pub(crate) fn manifest(raw: &Value) -> Manifest {
    Manifest {
        hooks: raw
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_map)
            .map(|h| hook_def(h, ExtMode::Strict))
            .collect(),
    }
}
