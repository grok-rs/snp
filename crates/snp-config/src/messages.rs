//! Every user-visible text of snp-config, in both brands (ARCHITECTURE §1.3 "Messages"; 01 §7; COMPATIBILITY
//! §3.3 for the snp-brand substitutions).
//!
//! The compat brand is pre-commit 4.6.2 byte for byte (`clientlib.py`, `repository.py`, `hook.py`); the snp brand
//! is the `brand_map` substitution of the same text. snp-only texts (extension keys, overlay notes, regex
//! `FutureWarning` lines) are brand-independent.
//!
//! Owner: WP-21.

use std::fmt::Write as _;

use snp_core::brand::Brand;
use snp_core::profile;

/// `` `pre-commit autoupdate` `` / `` `snp autoupdate` `` (`clientlib.py:309`, `repository.py:185`).
const fn autoupdate_hint(brand: Brand) -> &'static str {
    match brand {
        Brand::PreCommit => "`pre-commit autoupdate`",
        Brand::Snp => "`snp autoupdate`",
    }
}

/// `` `pre-commit migrate-config` `` / `` `snp migrate-config` `` (`clientlib.py:158,184`).
const fn migrate_hint(brand: Brand) -> &'static str {
    match brand {
        Brand::PreCommit => "`pre-commit migrate-config`",
        Brand::Snp => "`snp migrate-config`",
    }
}

/// W1 `warn_unknown_keys_root` (`clientlib.py:358-363`).
pub(crate) fn unexpected_keys_root(keys: &str) -> String {
    format!("Unexpected key(s) present at root: {keys}")
}

/// W2 `warn_unknown_keys_repo` (`clientlib.py:366-373`).
pub(crate) fn unexpected_keys_repo(repo: &str, keys: &str) -> String {
    format!("Unexpected key(s) present on {repo}: {keys}")
}

/// W3 `WarnMutableRev` (`clientlib.py:302-310`); `key_repr` is `repr('rev')`, `repo_repr` the repr of the repo.
pub(crate) fn mutable_rev(brand: Brand, key_repr: &str, repo_repr: &str) -> String {
    format!(
        "The {key_repr} field of repo {repo_repr} appears to be a mutable reference (moving tag / branch).  \
         Mutable references are never updated after first install and are not supported.  \
         See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.  \
         Hint: {} often fixes this.",
        autoupdate_hint(brand)
    )
}

/// W4 `OptionalSensibleRegexAtHook` glob warning (`clientlib.py:318-322`).
pub(crate) fn hook_regex_glob(key_repr: &str, id_repr: &str) -> String {
    format!(
        "The {key_repr} field in hook {id_repr} is a regex, not a glob -- matching '/*' probably isn't what you \
         want here"
    )
}

/// W5 `OptionalSensibleRegexAtHook` slash warning (`clientlib.py:323-329`).
pub(crate) fn hook_regex_slash(brand: Brand, key_repr: &str, id_repr: &str, fwd: &str) -> String {
    format!(
        "{} normalizes slashes in the {key_repr} field in hook {id_repr} to forward slashes, so you can use / \
         instead of {fwd}",
        brand.prog()
    )
}

/// W6 `OptionalSensibleRegexAtTop` glob warning (`clientlib.py:336-340`).
pub(crate) fn top_regex_glob(key_repr: &str) -> String {
    format!(
        "The top-level {key_repr} field is a regex, not a glob -- matching '/*' probably isn't what you want here"
    )
}

/// W7 `OptionalSensibleRegexAtTop` slash warning (`clientlib.py:341-347`).
pub(crate) fn top_regex_slash(brand: Brand, key_repr: &str, fwd: &str) -> String {
    format!(
        "{} normalizes the slashes in the top-level {key_repr} field to forward slashes, so you can use / \
         instead of {fwd}",
        brand.prog()
    )
}

/// W8 `DeprecatedStagesWarning` (`clientlib.py:154-159`).
pub(crate) fn hook_deprecated_stages(brand: Brand, id: &str, legacy: &str) -> String {
    format!(
        "hook id `{id}` uses deprecated stage names ({legacy}) which will be removed in a future version.  \
         run: {} to automatically fix this.",
        migrate_hint(brand)
    )
}

/// W9 `DeprecatedDefaultStagesWarning` (`clientlib.py:180-185`).
pub(crate) fn top_deprecated_stages(brand: Brand, legacy: &str) -> String {
    format!(
        "top-level `default_stages` uses deprecated stage names ({legacy}) which will be removed in a future \
         version.  run: {} to automatically fix this.",
        migrate_hint(brand)
    )
}

/// W11 `Hook.create` (`hook.py:49-57`).
pub(crate) fn unexpected_keys_hook(src: &str, id: &str, keys: &str) -> String {
    format!("Unexpected key(s) present on {src} => {id}: {keys}")
}

/// E1 `_cloned_repository_hooks` (`repository.py:182-187`).
pub(crate) fn unknown_hook_id(brand: Brand, id: &str, repo: &str) -> String {
    if repo == "builtin" {
        // `repo: builtin` has no rev, so autoupdate cannot help (ADR-084).
        return format!(
            "`{id}` is not present in repository builtin.  Typo? Not every pre-commit-hooks hook is builtin (see \
             `repo: builtin` in docs/user/configuration.md); use `repo: https://github.com/pre-commit/pre-commit-hooks` \
             with a `rev` for it."
        );
    }
    format!(
        "`{id}` is not present in repository {repo}.  Typo? Perhaps it is introduced in a newer version?  \
         Often {} fixes this.",
        autoupdate_hint(brand)
    )
}

/// E2 / E3 `_hook` env-less guard (`repository.py:126-142`); `what` is `language_version` or
/// `additional_dependencies`.
pub(crate) fn envless(id: &str, what: &str, lang: &str) -> String {
    format!(
        "The hook `{id}` specifies `{what}` but is using language `{lang}` which does not install an environment.  \
         Perhaps you meant to use a specific language?"
    )
}

/// `check_type_tag` (`clientlib.py:42-47`); `tag_repr` is the repr of the offending value.
pub(crate) fn type_tag(brand: Brand, tag_repr: &str) -> String {
    match brand {
        Brand::PreCommit => format!(
            "Type tag {tag_repr} is not recognized.  Try upgrading identify and pre-commit?"
        ),
        Brand::Snp => format!("Type tag {tag_repr} is not recognized.  Try upgrading snp?"),
    }
}

/// `check_min_version` (`clientlib.py:55-61`); the snp brand sentence is ADR-023's.
pub(crate) fn min_version(brand: Brand, version: &str) -> String {
    match brand {
        Brand::PreCommit => format!(
            "pre-commit version {version} is required but version {} is installed.  \
             Perhaps run `pip install --upgrade pre-commit`.",
            profile::PRE_COMMIT
        ),
        Brand::Snp => format!(
            "pre-commit version {version} is required but snp {} is compatible with pre-commit {}.  \
             Perhaps upgrade snp.",
            profile::SNP_VERSION,
            profile::PRE_COMMIT
        ),
    }
}

/// `cfgv.load_from_filename`: `{display_filename} is not a file` (`cfgv.py:436-437`).
pub(crate) fn not_a_file(display: &str) -> String {
    format!("{display} is not a file")
}

/// `NotAllowed` (`clientlib.py:401-404`).
pub(crate) fn cannot_be_overridden(key_repr: &str) -> String {
    format!("{key_repr} cannot be overridden")
}

/// `autoupdate._original_lines` second mismatch (`autoupdate.py:131-132`): `AssertionError`, exit 3.
pub(crate) const COULD_NOT_FIND_REV_LINES: &str = "could not find rev lines";

/// `migrate_config` outcome lines (`migrate_config.py:130-134`).
pub(crate) const MIGRATED: &str = "Configuration has been migrated.";
/// `migrate_config` with nothing to do.
pub(crate) const ALREADY_MIGRATED: &str = "Configuration is already migrated.";

// --- snp-only texts (brand-independent) -------------------------------------------------------------------------

/// D-MISC-04: a regex `FutureWarning` (01 Addendum §A.4), one stderr line per distinct (pattern, message).
pub(crate) fn future_warning(pattern_repr: &str, msg: &str) -> String {
    format!("[WARNING] regex {pattern_repr}: {msg} (Python FutureWarning)")
}

/// D-EXT-03: the active-overlay line (stderr).
pub(crate) fn overlay_active(path: &str) -> String {
    format!("snp: using local overrides from {path}")
}

/// D-EXT-03: an overlay that is tracked by git is ignored.
pub(crate) fn overlay_tracked(path: &str) -> String {
    format!(
        "snp: ignoring local config {path}: it is tracked by git (local overrides are per developer; \
         untrack it or name it in SNP_LOCAL_CONFIG)"
    )
}

/// D-EXT-03: an overlay is ignored when `CI` is truthy.
pub(crate) fn overlay_ci(path: &str) -> String {
    format!(
        "snp: ignoring local config {path}: CI is set (name it in SNP_LOCAL_CONFIG to use it in CI)"
    )
}

/// D-EXT-03: `SNP_LOCAL_CONFIG` names a file that does not exist.
pub(crate) fn overlay_missing(path: &str) -> String {
    format!("snp: ignoring local config {path}: SNP_LOCAL_CONFIG names a file that does not exist")
}

/// D-EXT-03: an overlay hook entry that matched no hook.
pub(crate) fn overlay_unknown_hook(id: &str) -> String {
    format!("snp: local config overrides unknown hook '{id}'")
}

/// `minimum_snp_version` not satisfied (COMPATIBILITY §7.2).
pub(crate) fn min_snp_version(version: &str) -> String {
    format!(
        "snp {version} is required but snp {} is installed.",
        profile::SNP_VERSION
    )
}

/// `minimum_snp_version` that is not `MAJOR[.MINOR[.PATCH]]`.
pub(crate) fn bad_snp_version(version_repr: &str) -> String {
    format!("invalid version {version_repr}: expected MAJOR.MINOR.PATCH")
}

/// `interactive: true` on a hook that runs inside snp (ADR-085).
pub(crate) fn interactive_in_process(kind: &str) -> String {
    format!("interactive: true needs a hook that runs a process; {kind} hooks run inside snp")
}

/// A reserved extension key (`follow`; ARCHITECTURE §8.2).
pub(crate) fn reserved_key(key_repr: &str) -> String {
    format!("{key_repr} is not available in this version")
}

/// `priority` / `priorities` value out of range.
pub(crate) fn priority_range(value_repr: &str) -> String {
    format!("priority {value_repr} is out of range (expected 0..=4294967295)")
}

/// `priority` of the wrong type.
pub(crate) fn priority_type(type_name: &str) -> String {
    format!("Expected a non-negative integer or a priority name got {type_name}")
}

/// `priority: <name>` without a matching root `priorities` entry.
pub(crate) fn priority_unknown(name_repr: &str) -> String {
    format!("priority name {name_repr} is not defined in the top-level `priorities`")
}

/// An `env` variable name that cannot be exported.
pub(crate) fn env_name(name_repr: &str) -> String {
    format!(
        "invalid environment variable name {name_repr}: names must be non-empty without '=' or NUL"
    )
}

/// An `env` value containing NUL.
pub(crate) fn env_value(value_repr: &str) -> String {
    format!("invalid environment variable value {value_repr}: values must not contain NUL")
}

/// A `groups` name that is empty, contains whitespace or starts with `@` (ADR-080).
pub(crate) fn group_name(name_repr: &str) -> String {
    format!(
        "invalid group name {name_repr}: names must be non-empty, without whitespace and must not start with '@' \
         (reserved for selectors such as @ungrouped)"
    )
}

/// `timeout` out of range.
pub(crate) fn timeout_range(value_repr: &str) -> String {
    format!("timeout {value_repr} must be greater than 0s and at most 24h")
}

/// `root` that is not a relative directory inside the repository.
pub(crate) fn root_dir(value_repr: &str) -> String {
    format!(
        "root {value_repr} must be a relative directory inside the repository (no '..', not absolute)"
    )
}

/// A `skip` / `only` map without any condition.
pub(crate) const EMPTY_CONDITION: &str = "Expected at least one condition (merge, rebase, merge-commit, \
                                          cherry-pick, revert, bisect, ref, env, ci, run)";

/// A `{ref: glob}` condition whose glob does not compile.
pub(crate) fn bad_glob(glob_repr: &str, err: &str) -> String {
    format!("invalid glob {glob_repr}: {err}")
}

/// An empty `{env: …}` / `{run: …}` condition value.
pub(crate) fn empty_condition_value(key: &str) -> String {
    format!("Expected a non-empty string for {key}")
}

/// `rev` on `repo: builtin` (ARCHITECTURE §7.3).
pub(crate) fn builtin_rev(found_repr: &str) -> String {
    format!("Expected rev to be absent when repo is 'builtin', found rev: {found_repr}")
}

/// `", ".join(items)`.
pub(crate) fn join<I, S>(items: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for (i, s) in items.into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(s.as_ref());
    }
    out
}

/// The note `snp config dump` puts above an overlay-patched config.
pub(crate) fn dump_overlay_header(path: &str, fields: &[String]) -> String {
    let mut out = String::new();
    writeln!(out, "# snp: local overrides from {path}").unwrap_or_default();
    for f in fields {
        writeln!(out, "#   overridden: {f}").unwrap_or_default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_substitutions_follow_the_brand_map() {
        let compat = mutable_rev(Brand::PreCommit, "'rev'", "'r'");
        let snp = mutable_rev(Brand::Snp, "'rev'", "'r'");
        assert_eq!(
            compat.replace("`pre-commit autoupdate`", "`snp autoupdate`"),
            snp
        );
        let compat = hook_regex_slash(Brand::PreCommit, "'files'", "'x'", r"[\/]");
        assert!(compat.starts_with("pre-commit normalizes slashes"));
        assert_eq!(
            compat.replace("pre-commit normalizes ", "snp normalizes "),
            hook_regex_slash(Brand::Snp, "'files'", "'x'", r"[\/]")
        );
        assert_eq!(
            type_tag(Brand::Snp, "'x'"),
            type_tag(Brand::PreCommit, "'x'").replace(
                "Try upgrading identify and pre-commit?",
                "Try upgrading snp?"
            )
        );
        assert_eq!(
            min_version(Brand::PreCommit, "99"),
            "pre-commit version 99 is required but version 4.6.2 is installed.  Perhaps run `pip install --upgrade pre-commit`."
        );
        assert_eq!(
            min_version(Brand::Snp, "99"),
            "pre-commit version 99 is required but snp 2.0.0 is compatible with pre-commit 4.6.2.  Perhaps upgrade snp."
        );
    }
}
