//! Every user-visible text snp-cmd produces (ARCHITECTURE §1.3 "Messages").
//!
//! pre-commit's texts are reproduced verbatim for the compat brand; the texts that name the tool or a command to
//! type take the brand (`brand_map`, ARCHITECTURE §9.6). The snp-only notes (11 §A.5.4 M1/M1b/M2/M3/M12,
//! `doctor`, `self`, `cache`, the git config hooks) read the same in both brands except for `{prog}`.
//!
//! Path-bearing messages are built as bytes: on Unix a path is arbitrary bytes and is printed unaltered, as
//! pre-commit's `output.write_line` does with `surrogateescape`d `str`s.
//!
//! Owner: WP-33.

use std::ffi::OsStr;

use snp_core::brand::Brand;

/// The bytes of a path or string as pre-commit prints them (`os.fsencode`; WTF-8 on Windows).
pub(crate) fn os_bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// Concatenates byte fragments.
pub(crate) fn cat(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// install / uninstall / init-templatedir (`commands/install_uninstall.py`, `init_templatedir.py`; 03 §2–§5)
// ---------------------------------------------------------------------------------------------------------------

/// `install_uninstall.py:111`: `pre-commit installed at {hook_path}` (`{prog}` by brand, COMPATIBILITY §3.3).
pub(crate) fn installed_at(brand: Brand, hook_path: &OsStr) -> Vec<u8> {
    cat(&[
        brand.prog().as_bytes(),
        b" installed at ",
        os_bytes(hook_path),
    ])
}

/// `install_uninstall.py:83-86`: one `write_line` with an embedded newline (`Use -f to use only {prog}.`).
pub(crate) fn migration_mode(brand: Brand, legacy_path: &OsStr) -> Vec<u8> {
    cat(&[
        b"Running in migration mode with existing hooks at ",
        os_bytes(legacy_path),
        b"\nUse -f to use only ",
        brand.prog().as_bytes(),
        b".",
    ])
}

/// `install_uninstall.py:124-128`: logged at ERROR (`[ERROR] …`), exit 1.
pub(crate) const HOOKS_PATH_REFUSAL: &str = "Cowardly refusing to install hooks with `core.hooksPath` set.\nhint: `git config --unset-all core.hooksPath`";

/// `install_uninstall.py:157`: `{hook_type} uninstalled`.
pub(crate) fn uninstalled(hook_type: &str) -> Vec<u8> {
    cat(&[hook_type.as_bytes(), b" uninstalled"])
}

/// `install_uninstall.py:161`: `Restored previous hooks to {hook_path}`.
pub(crate) fn restored(hook_path: &OsStr) -> Vec<u8> {
    cat(&[b"Restored previous hooks to ", os_bytes(hook_path)])
}

/// `init_templatedir.py:36`, logged at WARNING.
pub(crate) const TEMPLATEDIR_NOT_SET: &str = "`init.templateDir` not set to the target directory";

/// `init_templatedir.py:37`, logged at WARNING: ``maybe `git config --global init.templateDir {dest}`?``.
pub(crate) fn templatedir_maybe(dest: &OsStr) -> String {
    format!(
        "maybe `git config --global init.templateDir {}`?",
        dest.to_string_lossy()
    )
}

/// D-DATA-03 (03 §20.3): a foreign `<type>.legacy` would be overwritten by moving another foreign script onto it
/// (pre-commit's `shutil.move` replaces it silently); `FatalError`, exit 1, nothing written.
pub(crate) fn refuse_legacy(legacy_path: &OsStr) -> Vec<u8> {
    cat(&[
        b"refusing to overwrite existing ",
        os_bytes(legacy_path),
        b"; use -f to discard it",
    ])
}

/// `git.get_root` (`git.py:63-67`): no git, or not inside a repository.
pub(crate) const GIT_FAILED: &str =
    "git failed. Is it installed, and are you in a Git repository directory?";

// ---------------------------------------------------------------------------------------------------------------
// 11 §A.5.4 notes (stderr in both brands, D-DIAG-02)
// ---------------------------------------------------------------------------------------------------------------

/// Index of M1 in [`migration_notes`]: `Replaced hook script from {owner} at {hook_path}`.
pub(crate) const M1: usize = 0;
/// M1's ` (Python: {python})` suffix (a pre-commit script with a parsed, non-empty `INSTALL_PYTHON`).
pub(crate) const M1_PYTHON: usize = 1;
/// M1's ` (could not read its options; defaults used)` suffix (`doctor --fix` over an unparseable script).
pub(crate) const M1_DEFAULTS: usize = 2;
/// M1b: the replaced prek script used prek-only options.
pub(crate) const M1B: usize = 3;
/// M2: a manager script in `.legacy` was deleted.
pub(crate) const M2: usize = 4;
/// M3: another hook type still runs another manager.
pub(crate) const M3: usize = 5;
/// M12: `uninstall` deleted snp's own script from `.legacy`.
pub(crate) const M12: usize = 6;

/// The note templates of [`migration_notes`], compat brand (`{prog}` = `pre-commit`).
static NOTES_COMPAT: [&str; 7] = [
    "Replaced hook script from {owner} at {hook_path}",
    " (Python: {python})",
    " (could not read its options; defaults used)",
    "Note: the replaced prek hook used prek-only options ({opts}); they are not supported and were dropped",
    "Removed {legacy_path}: it is a hook script from {owner}, not a user hook",
    "Note: {hook_path} still runs {owner}; run `pre-commit install -t {T}` to switch it to snp",
    "Removed {legacy_path}: it is a hook script from snp, not a user hook",
];

/// The note templates of [`migration_notes`], snp brand (`{prog}` = `snp`).
static NOTES_SNP: [&str; 7] = [
    "Replaced hook script from {owner} at {hook_path}",
    " (Python: {python})",
    " (could not read its options; defaults used)",
    "Note: the replaced prek hook used prek-only options ({opts}); they are not supported and were dropped",
    "Removed {legacy_path}: it is a hook script from {owner}, not a user hook",
    "Note: {hook_path} still runs {owner}; run `snp install -t {T}` to switch it to snp",
    "Removed {legacy_path}: it is a hook script from snp, not a user hook",
];

/// The templates of M1, its two suffixes, M1b, M2, M3 and M12 (11 §A.5.4) with `{prog}` already substituted.
pub(crate) fn migration_notes(brand: Brand) -> &'static [&'static str] {
    match brand {
        Brand::PreCommit => &NOTES_COMPAT,
        Brand::Snp => &NOTES_SNP,
    }
}

/// Fills `{name}` placeholders of a note template with byte values.
pub(crate) fn fill(template: &str, vars: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at_checked(open).unwrap_or((rest, ""));
        out.extend_from_slice(head.as_bytes());
        let Some(close) = tail.find('}') else {
            out.extend_from_slice(tail.as_bytes());
            return out;
        };
        let name = tail.get(1..close).unwrap_or_default();
        match vars.iter().find(|(k, _)| *k == name) {
            Some((_, v)) => out.extend_from_slice(v),
            None => out.extend_from_slice(tail.get(..=close).unwrap_or_default().as_bytes()),
        }
        rest = tail.get(close.saturating_add(1)..).unwrap_or_default();
    }
    out.extend_from_slice(rest.as_bytes());
    out
}

/// A rendered note `idx` of [`migration_notes`].
pub(crate) fn note(brand: Brand, idx: usize, vars: &[(&str, &[u8])]) -> Vec<u8> {
    migration_notes(brand)
        .get(idx)
        .map(|t| fill(t, vars))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------
// git config hooks (git ≥ 2.54, ADR-057)
// ---------------------------------------------------------------------------------------------------------------

/// `--git-config-hooks` on a git without config-based hooks.
pub(crate) fn config_hooks_need_git(brand: Brand, version: &str) -> String {
    format!(
        "{} install --git-config-hooks needs git 2.54.0 or newer (found {version})",
        brand.prog()
    )
}

/// The line printed for every written config hook.
pub(crate) fn config_hook_installed(brand: Brand, name: &str) -> String {
    format!("{} installed as git config hook hook.{name}", brand.prog())
}

/// The line printed for every removed config hook.
pub(crate) fn config_hook_uninstalled(name: &str) -> String {
    format!("hook.{name} uninstalled")
}

// ---------------------------------------------------------------------------------------------------------------
// autoupdate (`commands/autoupdate.py`; 03 §11)
// ---------------------------------------------------------------------------------------------------------------

/// `autoupdate.py:184-188`: `repos ({sorted, ", "}) were not found in {config_file}`.
pub(crate) fn repos_not_found(missing: &[&str], config_file: &OsStr) -> Vec<u8> {
    cat(&[
        b"repos (",
        missing.join(", ").as_bytes(),
        b") were not found in ",
        os_bytes(config_file),
    ])
}

/// `autoupdate.py:208-214`: `[{repo}] updating {old} -> {new}` / `… -> {tag} (frozen)`.
pub(crate) fn updating(repo: &str, old: &str, new: &str, frozen: Option<&str>) -> String {
    match frozen {
        Some(tag) => format!("[{repo}] updating {old} -> {tag} (frozen)"),
        None => format!("[{repo}] updating {old} -> {new}"),
    }
}

/// `autoupdate.py:216`: `[{repo}] already up to date!`.
pub(crate) fn up_to_date(repo: &str) -> String {
    format!("[{repo}] already up to date!")
}

/// `autoupdate.py:96-100`: the text after `[{repo}] ` of `Cannot update because the update target is missing these
/// hooks: {sorted, ", "}`.
pub(crate) fn missing_hooks(sorted: &[&str]) -> String {
    format!(
        "Cannot update because the update target is missing these hooks: {}",
        sorted.join(", ")
    )
}

/// `autoupdate.py:80`: `RepositoryCannotBeUpdatedError(f'[{self.repo}] {e}')`; D-CRASH-06 uses the same shape for a
/// failed git command of one repository.
pub(crate) fn repo_error(repo: &str, text: &[u8]) -> Vec<u8> {
    cat(&[b"[", repo.as_bytes(), b"] ", text])
}

// ---------------------------------------------------------------------------------------------------------------
// gc / clean / cache (`commands/gc.py`, `clean.py`; ARCHITECTURE §5.6, ADR-030)
// ---------------------------------------------------------------------------------------------------------------

/// `gc.py:96`: `{n} repo(s) removed.`.
pub(crate) fn repos_removed(n: usize) -> String {
    format!("{n} repo(s) removed.")
}

/// M9 (11 §A.10.3), stderr (ADR-030, D-DIAG-05).
pub(crate) fn removed_v1(store: &OsStr) -> String {
    format!(
        "Removed snp 1.x cache data from {}.",
        store.to_string_lossy()
    )
}

/// `clean.py:14`: `Cleaned {store}.`.
pub(crate) fn cleaned(store: &OsStr) -> Vec<u8> {
    cat(&[b"Cleaned ", os_bytes(store), b"."])
}

// ---------------------------------------------------------------------------------------------------------------
// alias (`snp self alias …`, 11 §A.9.2)
// ---------------------------------------------------------------------------------------------------------------

/// `Created {alias} -> {target}` (symlink).
pub(crate) fn alias_created_symlink(alias: &OsStr, target: &OsStr) -> Vec<u8> {
    cat(&[b"Created ", os_bytes(alias), b" -> ", os_bytes(target)])
}

/// `Created {alias} (hard link to {target})`.
pub(crate) fn alias_created_hardlink(alias: &OsStr, target: &OsStr) -> Vec<u8> {
    cat(&[
        b"Created ",
        os_bytes(alias),
        b" (hard link to ",
        os_bytes(target),
        b")",
    ])
}

/// `Created {alias} (copy of {target})`.
pub(crate) fn alias_created_copy(alias: &OsStr, target: &OsStr) -> Vec<u8> {
    cat(&[
        b"Created ",
        os_bytes(alias),
        b" (copy of ",
        os_bytes(target),
        b")",
    ])
}

/// The refusal (stderr, exit 1).
pub(crate) fn alias_refused(alias: &OsStr, kind: &str) -> Vec<u8> {
    cat(&[
        os_bytes(alias),
        b" already exists and is not snp (",
        kind.as_bytes(),
        b"); use --force to replace it (it will be kept as ",
        os_bytes(alias),
        b".snp-backup)",
    ])
}

/// `--force` with an existing backup (stderr, exit 1).
pub(crate) fn alias_backup_exists(backup: &OsStr) -> Vec<u8> {
    cat(&[
        os_bytes(backup),
        b" already exists; remove it first (refusing to overwrite a backup)",
    ])
}

/// The shadow warning (stderr).
pub(crate) fn alias_shadowed(other: &OsStr, dir: &OsStr) -> Vec<u8> {
    cat(&[
        b"warning: `pre-commit` resolves to ",
        os_bytes(other),
        b" on PATH, which comes before ",
        os_bytes(dir),
        b"; typing `pre-commit` will not run snp",
    ])
}

/// `Removed {alias}`.
pub(crate) fn alias_removed(alias: &OsStr) -> Vec<u8> {
    cat(&[b"Removed ", os_bytes(alias)])
}

/// `Restored {alias} from {alias}.snp-backup`.
pub(crate) fn alias_restored(alias: &OsStr) -> Vec<u8> {
    cat(&[
        b"Restored ",
        os_bytes(alias),
        b" from ",
        os_bytes(alias),
        b".snp-backup",
    ])
}

/// `{alias} is not an snp alias; leaving it alone` (stderr, exit 1).
pub(crate) fn alias_not_ours(alias: &OsStr) -> Vec<u8> {
    cat(&[os_bytes(alias), b" is not an snp alias; leaving it alone"])
}

/// `{alias} does not exist` (stdout, exit 0).
pub(crate) fn alias_absent(alias: &OsStr) -> Vec<u8> {
    cat(&[os_bytes(alias), b" does not exist"])
}

/// `pre-commit -> {resolved} ({kind})`.
pub(crate) fn alias_status(resolved: &OsStr, kind: &str) -> Vec<u8> {
    cat(&[
        b"pre-commit -> ",
        os_bytes(resolved),
        b" (",
        kind.as_bytes(),
        b")",
    ])
}

/// `status` when nothing named `pre-commit` is on PATH.
pub(crate) const ALIAS_NOT_FOUND: &str = "pre-commit -> (not found)";

/// Windows cannot replace a running `pre-commit.exe`.
pub(crate) const ALIAS_BUSY: &str = "close running pre-commit processes and retry";

// ---------------------------------------------------------------------------------------------------------------
// doctor (11 §A.15)
// ---------------------------------------------------------------------------------------------------------------

/// The summary line.
pub(crate) fn doctor_summary(errors: usize, warnings: usize) -> String {
    format!(
        "{} problem(s): {errors} error(s), {warnings} warning(s).",
        errors.saturating_add(warnings)
    )
}

/// Appended to the summary when `--fix` could repair something.
pub(crate) fn doctor_fix_hint(brand: Brand) -> String {
    format!(
        "Run `{} doctor --fix` to repair this repository's hook scripts.",
        brand.prog()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_bytes() {
        let v = note(
            Brand::Snp,
            M3,
            &[
                ("hook_path", b".git/hooks/pre-push"),
                ("owner", b"pre-commit"),
                ("T", b"pre-push"),
            ],
        );
        assert_eq!(
            String::from_utf8(v).unwrap(),
            "Note: .git/hooks/pre-push still runs pre-commit; run `snp install -t pre-push` to switch it to snp"
        );
        let v = note(
            Brand::PreCommit,
            M3,
            &[
                ("hook_path", b"h"),
                ("owner", b"prek"),
                ("T", b"pre-commit"),
            ],
        );
        assert!(
            String::from_utf8(v)
                .unwrap()
                .contains("`pre-commit install -t pre-commit`")
        );
        assert_eq!(fill("a {x} {y}", &[("x", b"1")]), b"a 1 {y}");
        assert_eq!(fill("{", &[]), b"{");
        assert_eq!(fill("}{a", &[]), b"}{a");
    }

    #[test]
    fn pre_commit_texts_both_brands() {
        let p = OsStr::new(".git/hooks/pre-commit");
        assert_eq!(
            installed_at(Brand::PreCommit, p),
            b"pre-commit installed at .git/hooks/pre-commit"
        );
        assert_eq!(
            installed_at(Brand::Snp, p),
            b"snp installed at .git/hooks/pre-commit"
        );
        assert_eq!(
            migration_mode(Brand::PreCommit, OsStr::new("x.legacy")),
            b"Running in migration mode with existing hooks at x.legacy\nUse -f to use only pre-commit."
        );
        assert_eq!(
            migration_mode(Brand::Snp, OsStr::new("x.legacy")),
            b"Running in migration mode with existing hooks at x.legacy\nUse -f to use only snp."
        );
        assert_eq!(uninstalled("pre-push"), b"pre-push uninstalled");
        assert_eq!(repos_removed(0), "0 repo(s) removed.");
        assert_eq!(cleaned(OsStr::new("/s")), b"Cleaned /s.");
        assert_eq!(
            repos_not_found(&["dne", "foo"], OsStr::new(".pre-commit-config.yaml")),
            b"repos (dne, foo) were not found in .pre-commit-config.yaml"
        );
        assert_eq!(
            updating("r", "v1", "abc", Some("v2")),
            "[r] updating v1 -> v2 (frozen)"
        );
        assert_eq!(updating("r", "v1", "v2", None), "[r] updating v1 -> v2");
        assert_eq!(up_to_date("r"), "[r] already up to date!");
        assert_eq!(
            repo_error("r", missing_hooks(&["a", "b"]).as_bytes()),
            b"[r] Cannot update because the update target is missing these hooks: a, b"
        );
        assert_eq!(
            doctor_summary(1, 2),
            "3 problem(s): 1 error(s), 2 warning(s)."
        );
    }
}
