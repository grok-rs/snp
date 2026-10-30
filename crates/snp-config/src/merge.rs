//! `repository._hook()`: manifest ∪ config overrides, defaults, env-less guards, overlay patches (01 §6,
//! `repository.py:109-144`, `hook.py:49-57`).
//!
//! Owner: WP-21.

use std::sync::Arc;

use cfgv::{Map, Value};
use snp_core::output::WarnSink;

use crate::convert::{arc, key_str, stage_set, str_list, tag_set};
use crate::ext;
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
fn update(h: &mut HookDef, raw: &Map, extra: &mut Vec<String>) {
    for (k, v) in raw {
        let handled = k.as_str().is_some_and(|key| apply_key(h, key, v));
        if !handled {
            extra.push(key_str(k));
        }
    }
}

/// The overlay hook patch (ARCHITECTURE §8.4), applied after manifest ∪ config: `entry` expands `{entry}`
/// against the merged entry, `env` merges (overlay wins), every other key replaces.
fn apply_patch(h: &mut HookDef, patch: &HookPatch, extra: &mut Vec<String>) {
    for (k, v) in &patch.raw {
        let Some(key) = k.as_str() else {
            extra.push(key_str(k));
            continue;
        };
        if key == "entry" {
            if let Some(entry) = v.as_str() {
                h.entry = Arc::from(entry.replace(ENTRY_PLACEHOLDER, &h.entry));
            }
        } else if ext::HOOK_EXT_KEYS.contains(&key) {
            ext::apply_hook_key(&mut h.ext, key, v);
        } else if !apply_key(h, key, v) {
            extra.push(key_str(k));
        }
    }
}

/// The resolution rules of `_hook` after the dict merge (01 §6.3, §6.4) and `Hook.create`'s W11 warning.
fn finish(
    mut h: HookDef,
    mut extra: Vec<String>,
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
    extra.sort();
    extra.dedup();
    if !extra.is_empty() {
        warn.warn(&messages::unexpected_keys_hook(
            src,
            &h.id,
            &messages::join(&extra),
        ));
    }
    h.extra_keys = extra.iter().map(|k| Arc::from(k.as_str())).collect();
    Ok(h)
}

/// `repository._hook()`: manifest ∪ overrides (whole-key), `language_version` via `default_language_version` (the
/// language's own default is resolved by snp-repo with probes), empty stages ⇒ `default_stages`, env-less guards,
/// W11 unknown-key warning (sorted) after merging. `overlay` (§8.4) is applied last, after manifest ∪ config:
/// `{entry}` expands against the merged entry; fast-path G4 and the `InstallKey` see the patched `HookDef`.
///
/// The unknown-id check (E1) is the caller's (it needs the whole repo first, `repository.py:180-187`); its text
/// is [`MergeError::UnknownId`]'s [`message`](MergeError::message).
pub fn merge_hook(
    m: &HookDef,
    c: &ConfigHook,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    let mut h = m.clone();
    let mut extra: Vec<String> = m.extra_keys.iter().map(ToString::to_string).collect();
    update(&mut h, &c.raw, &mut extra);
    h.ext = c.ext.clone();
    if let Some(p) = overlay {
        apply_patch(&mut h, p, &mut extra);
    }
    finish(h, extra, cfg, src, warn)
}

/// The `repo: local` / `repo: meta` half of `_hook()` (no manifest: the hook dict is already defaulted).
pub fn finalize_local(
    h: &HookDef,
    cfg: &Config,
    src: &str,
    overlay: Option<&HookPatch>,
    warn: &dyn WarnSink,
) -> Result<HookDef, MergeError> {
    let mut h = h.clone();
    let mut extra: Vec<String> = h.extra_keys.iter().map(ToString::to_string).collect();
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
