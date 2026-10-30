//! pre-commit's schemas reproduced item by item with cfgv 0.3 `MapItem` impls (01 §3–§5). `repo: builtin`
//! validation (no `rev`; `entry`/`additional_dependencies` forbidden; `language` absent or `system`) is part of the
//! config schema as cfgv `MapItem`s with cfgv-style contexts and messages (ADR-070); the unknown-id check stays in
//! snp-repo's merge.
//!
//! Item order is pre-commit's (`clientlib.py:230-539`): it decides which single error a user sees and the order
//! of the warnings. Extension keys (ARCHITECTURE §8.1) are appended after pre-commit's items and added to the
//! known-key list of the root `WarnAdditionalKeys`, so every pre-commit fixture keeps its output.
//!
//! Owner: WP-21.

use std::sync::OnceLock;

use cfgv::{CheckFn, ConditionValue, Cx, Map, Schema, Validator, Value};
use snp_core::brand::Brand;
use snp_core::stage::{HookType, Stage};

use crate::ExtMode;
use crate::ext::{self, HOOK_EXT_KEYS, ROOT_EXT_KEYS};
use crate::items::{
    BUILTIN, BuiltinNoRev, Compiled, DEFAULT, DeprecatedStagesWarning, ExtKey, LOCAL,
    LanguageMigration, META, NotAllowed, PriorityAliases, SensibleRegex, StagesMigration,
    UnlessBuiltin, WarnMutableRev, check_min_version, check_string_regex, check_type_tag,
    language_values,
};
use crate::messages;

/// pre-commit's root keys (`clientlib.py:516-529`), the known list of W1.
const ROOT_KEYS: [&str; 9] = [
    "repos",
    "default_install_hook_types",
    "default_language_version",
    "default_stages",
    "files",
    "exclude",
    "fail_fast",
    "minimum_pre_commit_version",
    "ci",
];

/// The meta hooks' ids and defaults (`clientlib.py:376-398`), in pre-commit's order.
const META_HOOKS: [(&str, &str); 3] = [
    ("check-hooks-apply", "check_hooks_apply"),
    ("check-useless-excludes", "check_useless_excludes"),
    ("identity", "identity"),
];

/// `'^' + re.escape(C.CONFIG_FILE) + '$'` (`clientlib.py:381,388`).
pub(crate) const META_FILES: &str = r"^\.pre\-commit\-config\.yaml$";

/// The non-overridable `entry` of a meta hook: snp runs meta hooks in-process (ARCHITECTURE §3.11), so the value
/// never reaches a shell; it mirrors pre-commit's `{python} -m pre_commit.meta_hooks.<module>` for display.
pub(crate) fn meta_entry(module: &str) -> String {
    format!("snp -m pre_commit.meta_hooks.{module}")
}

/// `cfgv.check_array(check_type_tag)`.
fn tags(brand: Brand) -> CheckFn {
    CheckFn::check_array(check_type_tag(brand))
}

/// `str()` of every key, `", "`-joined (cfgv already sorted them like Python's `sorted()`; non-string keys are
/// shown with `str()` instead of crashing, D-CRASH-02).
fn join_keys(keys: &[Value]) -> String {
    messages::join(
        keys.iter()
            .map(|k| k.try_to_string().unwrap_or_else(|_| k.to_string())),
    )
}

/// `MANIFEST_HOOK_DICT.items` (`clientlib.py:230-266`), in order. `stages` and `language` are the migration items.
fn manifest_items(brand: Brand) -> Vec<Validator> {
    vec![
        Validator::optional("minimum_pre_commit_version", check_min_version(brand), "0"),
        Validator::required("id", CheckFn::check_string()),
        Validator::required("name", CheckFn::check_string()),
        Validator::required("entry", CheckFn::check_string()),
        Validator::custom(LanguageMigration::new(
            CheckFn::check_one_of(language_values()),
            true,
        )),
        Validator::optional("alias", CheckFn::check_string(), ""),
        compiled(Validator::optional("files", check_string_regex(), "")),
        compiled(Validator::optional("exclude", check_string_regex(), "^$")),
        Validator::optional("types", tags(brand), Value::List(vec![Value::from("file")])),
        Validator::optional("types_or", tags(brand), Value::List(Vec::new())),
        Validator::optional("exclude_types", tags(brand), Value::List(Vec::new())),
        Validator::optional(
            "additional_dependencies",
            CheckFn::check_array(CheckFn::check_string()),
            Value::List(Vec::new()),
        ),
        Validator::optional(
            "args",
            CheckFn::check_array(CheckFn::check_string()),
            Value::List(Vec::new()),
        ),
        Validator::optional("always_run", CheckFn::check_bool(), false),
        Validator::optional("fail_fast", CheckFn::check_bool(), false),
        Validator::optional("pass_filenames", CheckFn::check_bool(), true),
        Validator::optional("description", CheckFn::check_string(), ""),
        Validator::optional("language_version", CheckFn::check_string(), DEFAULT),
        Validator::optional("log_file", CheckFn::check_string(), ""),
        Validator::optional("require_serial", CheckFn::check_bool(), false),
        Validator::custom(StagesMigration::new(
            "stages",
            Some(Value::List(Vec::new())),
        )),
        Validator::optional("verbose", CheckFn::check_bool(), false),
    ]
}

/// A `files` / `exclude` item that reports its compiled pattern (`items::Compiled`).
fn compiled(item: Validator) -> Validator {
    Validator::custom(Compiled(item))
}

/// The key of an item (`item.key`).
fn item_key(item: &Validator) -> Option<&str> {
    item.key().and_then(Value::as_str)
}

/// `OptionalNoDefault(item.key, item.check_fn)` for the manifest items whose key is not in `except`
/// (`CONFIG_HOOK_DICT`, `META_HOOK_DICT`, the overlay patch).
fn no_default_items(brand: Brand, except: &[&str]) -> Vec<Validator> {
    manifest_items(brand)
        .into_iter()
        .filter_map(|item| {
            let key = item_key(&item)?.to_owned();
            if except.contains(&key.as_str()) {
                return None;
            }
            let check = item.check_fn()?.clone();
            let compiles = matches!(key.as_str(), "files" | "exclude");
            let item = Validator::optional_no_default(key, check);
            Some(if compiles { compiled(item) } else { item })
        })
        .collect()
}

/// `_COMMON_HOOK_WARNINGS` (`clientlib.py:407-411`).
fn common_hook_warnings(brand: Brand) -> Vec<Validator> {
    vec![
        Validator::custom(SensibleRegex::new("files", true, brand)),
        Validator::custom(SensibleRegex::new("exclude", true, brand)),
        Validator::custom(DeprecatedStagesWarning::hook(brand)),
    ]
}

/// The hook-level extension items (ARCHITECTURE §8.2), appended after pre-commit's items.
fn hook_ext_items() -> Vec<Validator> {
    HOOK_EXT_KEYS
        .iter()
        .map(|k| ExtKey::item(k, ext::hook_check(k)))
        .collect()
}

/// `MANIFEST_HOOK_DICT`.
fn manifest_hook_dict(brand: Brand) -> Schema {
    Schema::map("Hook", Some(Value::from("id")), manifest_items(brand))
}

/// `CONFIG_HOOK_DICT` (`clientlib.py:439-458`): no defaults (#227).
fn config_hook_dict(brand: Brand, ext: ExtMode) -> Schema {
    let mut items = vec![Validator::required("id", CheckFn::check_string())];
    items.extend(no_default_items(brand, &["id", "stages", "language"]));
    items.push(Validator::custom(StagesMigration::new("stages", None)));
    items.push(Validator::custom(LanguageMigration::new(
        CheckFn::check_one_of(language_values()),
        false,
    )));
    items.extend(common_hook_warnings(brand));
    if ext == ExtMode::Enabled {
        items.extend(hook_ext_items());
    }
    Schema::map("Hook", Some(Value::from("id")), items)
}

/// `LOCAL_HOOK_DICT` (`clientlib.py:459-464`).
fn local_hook_dict(brand: Brand, ext: ExtMode) -> Schema {
    let mut items = manifest_items(brand);
    items.extend(common_hook_warnings(brand));
    if ext == ExtMode::Enabled {
        items.extend(hook_ext_items());
    }
    Schema::map("Hook", Some(Value::from("id")), items)
}

/// `META_HOOK_DICT` (`clientlib.py:412-438`).
fn meta_hook_dict(brand: Brand, ext: ExtMode) -> Schema {
    let mut items = vec![
        Validator::required("id", CheckFn::check_string()),
        Validator::required(
            "id",
            CheckFn::check_one_of(META_HOOKS.iter().map(|(id, _)| Value::from(*id))),
        ),
        Validator::optional(
            "language",
            CheckFn::check_one_of([Value::from("unsupported")]),
            "unsupported",
        ),
        Validator::custom(NotAllowed(Value::from("entry"))),
    ];
    for (id, module) in META_HOOKS {
        let cond = || ConditionValue::Exact(Value::from(id));
        let name = match id {
            "check-hooks-apply" => "Check hooks apply to the repository",
            "check-useless-excludes" => "Check for useless excludes",
            _ => "identity",
        };
        items.push(Validator::conditional_optional(
            "name",
            CheckFn::check_any(),
            name,
            "id",
            cond(),
        ));
        if id == "identity" {
            items.push(Validator::conditional_optional(
                "verbose",
                CheckFn::check_any(),
                true,
                "id",
                cond(),
            ));
        } else {
            items.push(Validator::conditional_optional(
                "files",
                CheckFn::check_any(),
                META_FILES,
                "id",
                cond(),
            ));
        }
        items.push(Validator::conditional_optional(
            "entry",
            CheckFn::check_any(),
            meta_entry(module),
            "id",
            cond(),
        ));
    }
    for item in manifest_items(brand) {
        match item_key(&item) {
            Some(key @ ("name" | "language" | "entry")) => {
                let key = key.to_owned();
                let check = item.check_fn().cloned().unwrap_or_else(CheckFn::check_any);
                items.push(Validator::optional_no_default(key, check));
            }
            _ => items.push(item),
        }
    }
    items.extend(common_hook_warnings(brand));
    if ext == ExtMode::Enabled {
        items.extend(hook_ext_items());
    }
    Schema::map("Hook", Some(Value::from("id")), items)
}

/// The hook of a `repo: builtin` entry (ARCHITECTURE §7.3): a config-level dict like `CONFIG_HOOK_DICT`, with
/// `entry` and `additional_dependencies` forbidden and `language` absent or `system` (= `unsupported`).
fn builtin_hook_dict(brand: Brand) -> Schema {
    let mut items = vec![
        Validator::required("id", CheckFn::check_string()),
        Validator::custom(NotAllowed(Value::from("entry"))),
        Validator::custom(NotAllowed(Value::from("additional_dependencies"))),
    ];
    items.extend(no_default_items(
        brand,
        &[
            "id",
            "stages",
            "language",
            "entry",
            "additional_dependencies",
        ],
    ));
    items.push(Validator::custom(StagesMigration::new("stages", None)));
    items.push(Validator::custom(LanguageMigration::new(
        CheckFn::check_one_of([Value::from("unsupported")]),
        false,
    )));
    items.extend(common_hook_warnings(brand));
    items.extend(hook_ext_items());
    Schema::map("Hook", Some(Value::from("id")), items)
}

/// `CONFIG_REPO_DICT` (`clientlib.py:465-490`), plus the `repo: builtin` items in [`ExtMode::Enabled`].
pub(crate) fn repo_dict(brand: Brand, ext: ExtMode) -> Schema {
    let remote = Validator::conditional_recurse(
        "hooks",
        Schema::array(config_hook_dict(brand, ext)),
        "repo",
        ConditionValue::NotIn(vec![Value::from(LOCAL), Value::from(META)]),
    );
    let local = Validator::conditional_recurse(
        "hooks",
        Schema::array(local_hook_dict(brand, ext)),
        "repo",
        ConditionValue::Exact(Value::from(LOCAL)),
    );
    let meta = Validator::conditional_recurse(
        "hooks",
        Schema::array(meta_hook_dict(brand, ext)),
        "repo",
        ConditionValue::Exact(Value::from(META)),
    );
    let rev = Validator::custom(WarnMutableRev::new(brand));
    let warn = Validator::warn_additional_keys(
        vec![
            Value::from("repo"),
            Value::from("rev"),
            Value::from("hooks"),
        ],
        |extra: &[Value], _keys: &[Value], dct: &Map, cx: &mut Cx<'_>| {
            let repo = dct
                .get(&Value::from("repo"))
                .map(|r| r.try_to_string().unwrap_or_else(|_| r.to_string()))
                .unwrap_or_default();
            cx.warn(messages::unexpected_keys_repo(&repo, &join_keys(extra)));
        },
    );
    let items = match ext {
        ExtMode::Strict => vec![
            Validator::required("repo", CheckFn::check_string()),
            remote,
            local,
            meta,
            rev,
            warn,
        ],
        ExtMode::Enabled => vec![
            Validator::required("repo", CheckFn::check_string()),
            Validator::custom(UnlessBuiltin(remote)),
            local,
            meta,
            Validator::conditional_recurse(
                "hooks",
                Schema::array(builtin_hook_dict(brand)),
                "repo",
                ConditionValue::Exact(Value::from(BUILTIN)),
            ),
            Validator::custom(UnlessBuiltin(rev)),
            Validator::custom(BuiltinNoRev),
            warn,
        ],
    };
    Schema::map("Repository", Some(Value::from("repo")), items)
}

/// `DEFAULT_LANGUAGE_VERSION` (`clientlib.py:491-495`).
fn default_language_version() -> Schema {
    let mut items = vec![Validator::no_additional_keys(language_values())];
    items.extend(
        language_values()
            .into_iter()
            .map(|lang| Validator::optional(lang, CheckFn::check_string(), DEFAULT)),
    );
    Schema::map("DefaultLanguageVersion", None, items)
}

/// `STAGES` in source order (the `default_stages` default).
fn all_stages() -> Value {
    Value::List(Stage::ALL.iter().map(|s| Value::from(s.as_str())).collect())
}

/// `CONFIG_SCHEMA` items (`clientlib.py:496-539`), then the root extension items.
fn build_config_schema(brand: Brand, ext: ExtMode) -> Schema {
    let mut known: Vec<Value> = ROOT_KEYS.iter().map(|k| Value::from(*k)).collect();
    if ext == ExtMode::Enabled {
        known.extend(ROOT_EXT_KEYS.iter().map(|k| Value::from(*k)));
    }
    let mut items = vec![
        Validator::optional("minimum_pre_commit_version", check_min_version(brand), "0"),
        Validator::required_recurse("repos", Schema::array(repo_dict(brand, ext))),
        Validator::optional(
            "default_install_hook_types",
            CheckFn::check_array(CheckFn::check_one_of(
                HookType::ALL.iter().map(|h| Value::from(h.as_str())),
            )),
            Value::List(vec![Value::from("pre-commit")]),
        ),
        Validator::optional_recurse(
            "default_language_version",
            default_language_version(),
            Value::Map(Map::new()),
        ),
        Validator::custom(StagesMigration::new("default_stages", Some(all_stages()))),
        Validator::custom(DeprecatedStagesWarning::top(brand)),
        compiled(Validator::optional("files", check_string_regex(), "")),
        compiled(Validator::optional("exclude", check_string_regex(), "^$")),
        Validator::optional("fail_fast", CheckFn::check_bool(), false),
        Validator::warn_additional_keys(
            known,
            |extra: &[Value], _keys: &[Value], _dct: &Map, cx: &mut Cx<'_>| {
                cx.warn(messages::unexpected_keys_root(&join_keys(extra)));
            },
        ),
        Validator::custom(SensibleRegex::new("files", false, brand)),
        Validator::custom(SensibleRegex::new("exclude", false, brand)),
        Validator::optional_no_default("ci", CheckFn::check_type(cfgv::ValueType::Map, None)),
    ];
    if ext == ExtMode::Enabled {
        items.extend(
            ROOT_EXT_KEYS
                .iter()
                .map(|k| ExtKey::item(k, ext::root_check(k))),
        );
        items.push(Validator::custom(PriorityAliases));
    }
    Schema::map("Config", None, items)
}

/// The per-brand slot of a schema cache.
const fn by_brand<T>(slots: &[T; 2], brand: Brand) -> &T {
    let [pc, snp] = slots;
    match brand {
        Brand::PreCommit => pc,
        Brand::Snp => snp,
    }
}

/// `CONFIG_SCHEMA` (`clientlib.py`) plus the extension keys unless `ext` is `Strict` (§8.1).
pub fn config_schema(brand: Brand, ext: ExtMode) -> &'static Schema {
    static SCHEMAS: [OnceLock<Schema>; 4] = [const { OnceLock::new() }; 4];
    let [pc, pc_strict, snp, snp_strict] = &SCHEMAS;
    let slot = match (brand, ext) {
        (Brand::PreCommit, ExtMode::Enabled) => pc,
        (Brand::PreCommit, ExtMode::Strict) => pc_strict,
        (Brand::Snp, ExtMode::Enabled) => snp,
        (Brand::Snp, ExtMode::Strict) => snp_strict,
    };
    slot.get_or_init(|| build_config_schema(brand, ext))
}

/// `MANIFEST_SCHEMA = Array(MANIFEST_HOOK_DICT)` (`clientlib.py`). Manifests never carry extension keys: an
/// extension key in a manifest is an unknown key (W11 at merge time).
pub fn manifest_schema(brand: Brand) -> &'static Schema {
    static SCHEMAS: [OnceLock<Schema>; 2] = [const { OnceLock::new() }; 2];
    by_brand(&SCHEMAS, brand).get_or_init(|| Schema::array(manifest_hook_dict(brand)))
}

/// The hook entry of the local overlay (ARCHITECTURE §8.4): any hook key except `id`/`alias`/`repo`, no
/// defaults, plus the extension keys.
fn overlay_patch_dict(brand: Brand) -> Schema {
    let mut items = vec![
        Validator::custom(NotAllowed(Value::from("id"))),
        Validator::custom(NotAllowed(Value::from("alias"))),
        Validator::custom(NotAllowed(Value::from("repo"))),
    ];
    items.extend(no_default_items(
        brand,
        &["id", "alias", "stages", "language"],
    ));
    items.push(Validator::custom(StagesMigration::new("stages", None)));
    items.push(Validator::custom(LanguageMigration::new(
        CheckFn::check_one_of(language_values()),
        false,
    )));
    items.extend(hook_ext_items());
    Schema::map("Hook", None, items)
}

/// The keys of the local overlay's root.
pub(crate) const OVERLAY_KEYS: [&str; 4] = ["hooks", "skip_groups", "fail_on_changes", "repos"];

/// The local overlay schema (`.pre-commit-config.local.yaml`, ARCHITECTURE §8.4).
pub(crate) fn overlay_schema(brand: Brand) -> &'static Schema {
    static SCHEMAS: [OnceLock<Schema>; 2] = [const { OnceLock::new() }; 2];
    by_brand(&SCHEMAS, brand).get_or_init(|| {
        Schema::map(
            "LocalConfig",
            None,
            vec![
                Validator::optional_recurse(
                    "hooks",
                    Schema::key_value_map(
                        "Hooks",
                        CheckFn::check_string(),
                        overlay_patch_dict(brand),
                    ),
                    Value::Map(Map::new()),
                ),
                Validator::optional("skip_groups", ext::check_groups(), Value::List(Vec::new())),
                ExtKey::item("fail_on_changes", ext::check_fail_on_changes()),
                Validator::optional_recurse(
                    "repos",
                    Schema::array(repo_dict(brand, ExtMode::Enabled)),
                    Value::List(Vec::new()),
                ),
                Validator::warn_additional_keys(
                    OVERLAY_KEYS.iter().map(|k| Value::from(*k)).collect(),
                    |extra: &[Value], _keys: &[Value], _dct: &Map, cx: &mut Cx<'_>| {
                        cx.warn(messages::unexpected_keys_root(&join_keys(extra)));
                    },
                ),
            ],
        )
    })
}

/// The 22 hook keys of `hook.py` (`_KEYS`), for the W11 unknown-key computation.
pub(crate) const HOOK_KEYS: [&str; 22] = [
    "id",
    "name",
    "entry",
    "language",
    "alias",
    "files",
    "exclude",
    "types",
    "types_or",
    "exclude_types",
    "additional_dependencies",
    "args",
    "always_run",
    "fail_fast",
    "pass_filenames",
    "description",
    "language_version",
    "log_file",
    "minimum_pre_commit_version",
    "require_serial",
    "stages",
    "verbose",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warn_additional_keys_lists_every_item_key() {
        // clientlib_test.py:539-545 (T10-CFG-22): the allowed-key list == the schema's keys.
        for ext in [ExtMode::Strict, ExtMode::Enabled] {
            let Schema::Map { items, .. } = config_schema(Brand::PreCommit, ext) else {
                panic!("not a map");
            };
            let mut keys: Vec<String> = items
                .iter()
                .filter_map(|i| i.key().and_then(Value::as_str).map(str::to_owned))
                .collect();
            keys.sort();
            keys.dedup();
            let mut known: Vec<String> = ROOT_KEYS.iter().map(|k| (*k).to_owned()).collect();
            if ext == ExtMode::Enabled {
                known.extend(ROOT_EXT_KEYS.iter().map(|k| (*k).to_owned()));
            }
            known.sort();
            assert_eq!(keys, known);
        }
    }

    #[test]
    fn config_hook_dict_has_no_defaults() {
        // clientlib_test.py:103-108 (T10-CFG-06).
        let Schema::Map { items, .. } = config_hook_dict(Brand::PreCommit, ExtMode::Enabled) else {
            panic!("not a map");
        };
        assert!(items.iter().all(|i| i.default_value().is_none()));
    }

    #[test]
    fn hook_keys_match_the_manifest_items() {
        let items = manifest_items(Brand::PreCommit);
        let mut keys: Vec<&str> = items.iter().filter_map(item_key).collect();
        keys.sort_unstable();
        let mut hook_keys = HOOK_KEYS.to_vec();
        hook_keys.sort_unstable();
        assert_eq!(keys, hook_keys);
    }
}
