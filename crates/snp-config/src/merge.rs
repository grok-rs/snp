//! `repository._hook()`: manifest ∪ config overrides, defaults, env-less guards, overlay patches (01 §6,
//! `repository.py:109-144`, `hook.py:49-57`).
//!
//! Owner: WP-21.

use std::sync::Arc;

use cfgv::{Map, Value};
use snp_core::output::WarnSink;

use crate::convert::{arc, py_sorted_keys, stage_set, str_list, tag_set};
use crate::ext::{self, Condition};
use crate::items::DEFAULT;
use crate::overlay::HookPatch;
use crate::{Config, ConfigHook, HookDef, MergeError, messages};

/// The overlay `entry` placeholder for the merged base entry (ARCHITECTURE §8.4).
const ENTRY_PLACEHOLDER: &str = concat!("{", "entry", "}");

/// `ret.update(dct)` for one validated hook key; `false` when `key` is not one of the 22 hook keys.
fn apply_key(h: &mut HookDef, key: &str, v: &Value) -> bool {
    let s = || arc(v.as_str().unwrap_or_default());
    let b = || v.as_bool().unwrap_or(false);
    match key {
        "id" => h.id = s(),
        "name" => h.name = s(),
        "entry" => h.entry = s(),
        "language" => {
            h.language = v
                .as_str()
                .and_then(snp_core::lang::LanguageId::from_config)
                .unwrap_or(h.language);
        }
        "alias" => h.alias = s(),
        "files" => h.files = s(),
        "exclude" => h.exclude = s(),
        "types" => h.types = tag_set(v),
        "types_or" => h.types_or = tag_set(v),
        "exclude_types" => h.exclude_types = tag_set(v),
        "additional_dependencies" => h.additional_dependencies = str_list(v),
        "args" => h.args = str_list(v),
        "always_run" => h.always_run = b(),
        "fail_fast" => h.fail_fast = b(),
        "pass_filenames" => h.pass_filenames = b(),
        "description" => h.description = s(),
        "language_version" => h.language_version = s(),
        "log_file" => h.log_file = s(),
        "minimum_pre_commit_version" => h.minimum_pre_commit_version = s(),
        "require_serial" => h.require_serial = b(),
        "stages" => h.stages = stage_set(v),
        "verbose" => h.verbose = b(),
        _ => return false,
    }
    true
}

/// Applies the keys of `raw` (`ret.update(raw)`), collecting the keys that are not hook keys into `extra`.
fn update(h: &mut HookDef, raw: &Map, extra: &mut Vec<Value>) {
    for (k, v) in raw {
        let handled = k.as_str().is_some_and(|key| apply_key(h, key, v));
        if !handled {
            extra.push(k.clone());
        }
    }
}

/// The overlay hook patch (ARCHITECTURE §8.4), applied after manifest ∪ config: `entry` expands `{entry}`
/// against the merged entry, `env` merges (overlay wins), every other key replaces.
fn apply_patch(h: &mut HookDef, patch: &HookPatch, extra: &mut Vec<Value>) {
    for (k, v) in &patch.raw {
        let Some(key) = k.as_str() else {
            extra.push(k.clone());
            continue;
        };
        if key == "entry" {
            if let Some(entry) = v.as_str() {
                h.entry = Arc::from(entry.replace(ENTRY_PLACEHOLDER, &h.entry));
            }
        } else if ext::HOOK_EXT_KEYS.contains(&key) {
            ext::apply_hook_key(&mut h.ext, key, v);
        } else if !apply_key(h, key, v) {
            extra.push(k.clone());
        }
    }
}

/// The resolution rules of `_hook` after the dict merge (01 §6.3, §6.4), the overlay's `skip_groups` against the
/// final groups, and `Hook.create`'s W11 warning.
///
/// `h.extra_keys` (the manifest's or the local hook's unknown keys, already in `sorted()` order) is united with
/// `added` (the config's and the patch's) and sorted again like Python's `sorted()` (`convert::py_sorted_keys`,
/// the order of the root and repo warnings and of `validate-config --strict`). A non-string key is shown with
/// `str()` where pre-commit's `", ".join(...)` / `sorted(...)` would crash (D-CRASH-02).
fn finish(
    mut h: HookDef,
    added: Vec<Value>,
    cfg: &Config,
    src: &str,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    if &*h.language_version == DEFAULT
        && let Some(v) = cfg.default_language_version.get(&h.language)
    {
        h.language_version = Arc::clone(v);
    }
    h.stages_set = !h.stages.is_empty();
    if h.stages.is_empty() {
        h.stages = cfg.default_stages;
    }
    if h.language.env_dir_base().is_none() {
        if &*h.language_version != DEFAULT {
            return Err(MergeError::LanguageVersionOnEnvless {
                id: Arc::clone(&h.id),
                lang: h.language,
            });
        }
        if !h.additional_dependencies.is_empty() {
            return Err(MergeError::DepsOnEnvless {
                id: Arc::clone(&h.id),
                lang: h.language,
            });
        }
    }
    if !added.is_empty() {
        let base = h.extra_keys.iter().map(|k| Value::from(&**k));
        h.extra_keys = py_sorted_keys(base.chain(added));
    }
    if crate::overlay::skips_groups(cfg, &h.ext.groups) {
        h.ext.skip.push(Condition::All(Vec::new()));
    }
    if !h.extra_keys.is_empty() {
        warn.warn(&messages::unexpected_keys_hook(
            src,
            &h.id,
            &messages::join(&h.extra_keys),
        ));
    }
    Ok(h)
}

/// `repository._hook()`: manifest ∪ overrides (whole-key), `language_version` via `default_language_version` (the
/// language's own default is resolved by snp-repo with probes), empty stages ⇒ `default_stages`, env-less guards,
/// W11 unknown-key warning (sorted) after merging. `overlay` (§8.4) is applied last, after manifest ∪ config:
/// `{entry}` expands against the merged entry; fast-path G4 and the `InstallKey` see the patched `HookDef`. The
/// overlay's `skip_groups` (recorded by [`crate::overlay::apply_root`]) is matched against the final `groups`
/// and adds an always-true `skip` condition after the patch's own `skip`.
///
/// The unknown-id check (E1) is the caller's (it needs the whole repo first, `repository.py:180-187`); its text
/// is [`MergeError::UnknownId`]'s [`message`](MergeError::message).
///
/// **Warning order.** For a cloned repo pre-commit runs `_hook()` (the env-less errors) for every hook before any
/// `Hook.create` (W11, `repository.py:189-200`): a remote repo whose third hook fails prints no W11 for its first
/// two. The caller reproduces that by passing one buffering sink per remote/builtin repo
/// (`snp_core::output::CollectWarnings`), flushing it to the real sink once every hook of the repo merged and
/// dropping it on the first error. ([`finalize_local`] needs no buffer: local and meta hooks are created one by
/// one, `repository.py:161-168`.)
pub fn merge_hook(
    m: &HookDef,
    c: &ConfigHook,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    let mut h = m.clone();
    let mut extra = Vec::new();
    update(&mut h, &c.raw, &mut extra);
    h.ext = c.ext.clone();
    if let Some(p) = overlay {
        apply_patch(&mut h, p, &mut extra);
    }
    finish(h, extra, cfg, src, warn)
}

/// The `repo: local` / `repo: meta` half of `_hook()` (no manifest: the hook dict is already defaulted). Each
/// hook's W11 is printed before the next hook merges (`_non_cloned_repository_hooks`, `repository.py:161-168`),
/// so the warnings go straight to the caller's sink.
pub fn finalize_local(
    h: &HookDef,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    let mut h = h.clone();
    let mut extra = Vec::new();
    if let Some(p) = overlay {
        apply_patch(&mut h, p, &mut extra);
    }
    finish(h, extra, cfg, src, warn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::HOOK_KEYS;

    #[test]
    fn every_hook_key_is_applied() {
        let mut h = crate::convert::hook_def(&Map::new(), crate::ExtMode::Enabled);
        for key in HOOK_KEYS {
            assert!(apply_key(&mut h, key, &Value::from("x")), "{key}");
        }
        assert!(!apply_key(&mut h, "priority", &Value::from(1)));
    }
}
