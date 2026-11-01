//! `snp list` (snp brand, §9.3).
//!
//! Owner: WP-32.

use std::fmt::Write as _;

use snp_cli::ListArgs;
use snp_core::failure::Failure;
use snp_repo::{HookSrc, ResolvedHook};

use crate::RunCtx;
use crate::common::{self, Buffered, ResolveParts};
use crate::select::GroupFilter;

/// Lists the configured hooks (text or `--json`).
///
/// The hooks of the config after resolution (manifest names, stages, groups), in config order, filtered by
/// `--hook-stage` and the group flags exactly as `run` selects them (ADR-080). Text: one line per hook, `{id}` padded
/// to the widest id, then the name and the groups in brackets. JSON: an array of objects with `id`, `alias`, `name`,
/// `repo`, `language`, `stages` and `groups`.
#[expect(
    clippy::needless_pass_by_value,
    reason = "frozen signature (ARCHITECTURE §1.4.21)"
)]
pub fn list(a: ListArgs, cx: &RunCtx) -> Result<i32, Failure> {
    let (git, info) = common::discover(cx)?;
    let root = info.root;
    let config = common::resolve_config(cx, &a.config, &root, true)?;
    let src = common::config_source(&config, &root);
    common::mark_config_used(&cx.store, &src.path, &root);
    let buffered = Buffered::default();
    let loaded = common::load_config_with(cx, &src, &buffered.warn, &buffered.diag);
    buffered.replay(&cx.printer);
    let mut cfg = loaded?;
    let overlay = common::apply_overlay(cx, &mut cfg, &git, false)?;
    let _guard = if common::uses_store(&cfg) {
        Some(common::store_lock(cx, &cx.store)?)
    } else {
        None
    };
    let parts = ResolveParts::new(cx.env.clone(), root, &cx.store);
    let policy = common::fast_path_policy(&cfg, &cx.env, false, false);
    let rcx = parts.ctx(
        cx,
        &cfg,
        &cx.printer,
        &*cx.printer,
        policy,
        overlay.as_ref(),
        &cx.cancel,
    );
    let hooks = cx.resolver.all_hooks(&rcx)?;
    let groups = GroupFilter::new(&a.groups, &a.require_groups, &a.no_groups);
    let shown: Vec<&ResolvedHook> = hooks
        .iter()
        .filter(|h| a.stage.is_none_or(|s| h.def.stages.contains(s)))
        .filter(|h| groups.matches(&h.def.ext.groups))
        .collect();
    let text = if a.json { json(&shown) } else { text(&shown) };
    cx.printer.write(text.as_bytes());
    Ok(0)
}

/// The text listing.
pub(crate) fn text(hooks: &[&ResolvedHook]) -> String {
    let width = hooks
        .iter()
        .map(|h| snp_pycompat::eaw::len_cjk(&h.def.id))
        .max()
        .unwrap_or(0);
    let mut s = String::new();
    for h in hooks {
        s.push_str(&h.def.id);
        let pad = width.saturating_sub(snp_pycompat::eaw::len_cjk(&h.def.id));
        s.extend(core::iter::repeat_n(' ', pad));
        s.push_str("  ");
        s.push_str(&h.def.name);
        if !h.def.ext.groups.is_empty() {
            let g: Vec<&str> = h.def.ext.groups.iter().map(|g| &**g).collect();
            write!(s, " [{}]", g.join(", ")).unwrap_or_default();
        }
        s.push('\n');
    }
    s
}

/// Where a hook comes from, as written in the config.
fn repo_name(h: &ResolvedHook) -> &str {
    match &h.src {
        HookSrc::Remote { repo, .. } => repo,
        HookSrc::Local => "local",
        HookSrc::Meta => "meta",
        HookSrc::Builtin => "builtin",
        _ => "",
    }
}

/// The JSON listing (hand-written: snp-run has no JSON dependency).
pub(crate) fn json(hooks: &[&ResolvedHook]) -> String {
    let mut s = String::from("[");
    for (i, h) in hooks.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("\n  {\"id\": ");
        json_str(&mut s, &h.def.id);
        s.push_str(", \"alias\": ");
        json_str(&mut s, &h.def.alias);
        s.push_str(", \"name\": ");
        json_str(&mut s, &h.def.name);
        s.push_str(", \"repo\": ");
        json_str(&mut s, repo_name(h));
        s.push_str(", \"language\": ");
        json_str(&mut s, h.def.language.as_str());
        s.push_str(", \"stages\": [");
        for (j, st) in h.def.stages.iter().enumerate() {
            if j > 0 {
                s.push_str(", ");
            }
            json_str(&mut s, st.as_str());
        }
        s.push_str("], \"groups\": [");
        for (j, g) in h.def.ext.groups.iter().enumerate() {
            if j > 0 {
                s.push_str(", ");
            }
            json_str(&mut s, g);
        }
        s.push_str("]}");
    }
    if !hooks.is_empty() {
        s.push('\n');
    }
    s.push_str("]\n");
    s
}

/// A JSON string literal.
pub(crate) fn json_str(out: &mut String, v: &str) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(c)).unwrap_or_default();
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes() {
        let mut s = String::new();
        json_str(&mut s, "a\"b\\c\nd\u{1}é");
        assert_eq!(s, "\"a\\\"b\\\\c\\nd\\u0001é\"");
        assert_eq!(json(&[]), "[]\n");
        assert_eq!(text(&[]), "");
    }
}
