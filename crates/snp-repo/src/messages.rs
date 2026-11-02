//! Every user-visible text snp-repo produces (ARCHITECTURE §1.3 "Messages").
//!
//! The compat brand is pre-commit 4.7.0 byte for byte (`store.py`, `clientlib.py`, `repository.py`); the snp brand is
//! the `brand_map` substitution of the same text (only the tool name in the autoupdate hint changes). The texts
//! that `snp-config` (unknown id, env-less guards), `snp-store` (lock contention) and `snp-lang` (support checks)
//! own are used through those crates.
//!
//! Owner: WP-31.

use std::borrow::Cow;

use snp_core::brand::Brand;

/// SM-01 (`store.py:160`): `Initializing environment for {db_repo_name}.` (logged at INFO).
pub(crate) fn initializing(db_repo_name: &str) -> String {
    format!("Initializing environment for {db_repo_name}.")
}

/// SM-02 (`repository.py:66-68`): the three INFO lines of `_hook_install`, `src` being the config's `repo` string
/// (`local`, `meta`, a URL) without dependencies.
pub(crate) fn installing(src: &str) -> [String; 3] {
    [
        format!("Installing environment for {src}."),
        "Once installed this environment will be reused.".to_owned(),
        "This may take a few minutes...".to_owned(),
    ]
}

/// `repository.py:84-89`: the post-install health failure (`AssertionError`, exit 3).
pub(crate) fn unhealthy_after_install(language: &str, health_error: &str) -> String {
    format!(
        "BUG: expected environment for {language} to be healthy immediately after install, please open an issue \
         describing your environment\n\nmore info:\n\n{health_error}"
    )
}

/// `clientlib.warn_for_stages_on_repo_init` (`clientlib.py:84-109`): the deprecated-stage warning printed once per
/// new clone; `legacy` are the legacy names in first-seen order.
pub(crate) fn deprecated_repo_stages(brand: Brand, repo: &str, legacy: &[&str]) -> String {
    format!(
        "repo `{repo}` uses deprecated stage names ({}) which will be removed in a future version.  Hint: often \
         `{} autoupdate --repo {}` will fix this.  if it does not -- consider reporting an issue to that repo.",
        legacy.join(", "),
        brand.prog(),
        shlex_quote(repo),
    )
}

/// The assertion of a run plan requested for a hook that does not run through its language (snp-run maps native
/// and meta hooks itself; reaching this is a caller bug, reported instead of panicking).
pub(crate) fn no_language_plan(id: &str) -> String {
    format!("snp: hook `{id}` runs natively and has no language run plan")
}

/// Python's `shlex.quote` (`shlex.py:323-333`): `''` for an empty string, the string itself when it only contains
/// `[\w@%+=:,./-]` (ASCII), otherwise single quotes with every `'` written as `'"'"'`.
pub(crate) fn shlex_quote(s: &str) -> Cow<'_, str> {
    if s.is_empty() {
        return Cow::Borrowed("''");
    }
    let safe = s
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if safe {
        return Cow::Borrowed(s);
    }
    let mut out = String::from("'");
    out.push_str(&s.replace('\'', "'\"'\"'"));
    out.push('\'');
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_lines_are_verbatim() {
        assert_eq!(
            initializing("https://example.com/r:six,attrs==23.1.0"),
            "Initializing environment for https://example.com/r:six,attrs==23.1.0."
        );
        assert_eq!(
            installing("local"),
            [
                "Installing environment for local.",
                "Once installed this environment will be reused.",
                "This may take a few minutes...",
            ]
        );
    }

    #[test]
    fn deprecated_stage_warning_in_both_brands() {
        let legacy = ["commit", "push", "merge-commit"];
        assert_eq!(
            deprecated_repo_stages(Brand::PreCommit, "/tmp/my repo", &legacy),
            "repo `/tmp/my repo` uses deprecated stage names (commit, push, merge-commit) which will be removed in \
             a future version.  Hint: often `pre-commit autoupdate --repo '/tmp/my repo'` will fix this.  if it \
             does not -- consider reporting an issue to that repo."
        );
        assert_eq!(
            deprecated_repo_stages(Brand::Snp, "https://x/y", &legacy[..1]),
            "repo `https://x/y` uses deprecated stage names (commit) which will be removed in a future version.  \
             Hint: often `snp autoupdate --repo https://x/y` will fix this.  if it does not -- consider reporting an \
             issue to that repo."
        );
    }

    #[test]
    fn shlex_quote_matches_python() {
        assert_eq!(shlex_quote(""), "''");
        assert_eq!(shlex_quote("abc-1.2/x@y%z+=:,"), "abc-1.2/x@y%z+=:,");
        assert_eq!(shlex_quote("a b"), "'a b'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote("é"), "'é'");
    }

    #[test]
    fn health_assertion_is_verbatim() {
        assert_eq!(
            unhealthy_after_install("python", "virtualenv is broken"),
            "BUG: expected environment for python to be healthy immediately after install, please open an issue \
             describing your environment\n\nmore info:\n\nvirtualenv is broken"
        );
    }
}
