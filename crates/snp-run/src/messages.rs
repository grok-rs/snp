//! Every user-visible text of the run pipeline, hook-impl and try-repo (ARCHITECTURE §1.3 "Messages"): compat texts
//! are pre-commit 4.6.2's bytes; snp-brand texts are the `brand_map` substitutions of COMPATIBILITY §3.3.
//!
//! Owner: WP-32.

use std::time::Duration;

use snp_core::brand::Brand;
use snp_core::profile::BUG_URL;

/// `run.py:357`: `[ERROR] Unmerged files.  Resolve before committing.` (two spaces).
pub(crate) const UNMERGED: &str = "Unmerged files.  Resolve before committing.";

/// `run.py:360`.
pub(crate) const BOTH_REFS: &str = "Specify both --from-ref and --to-ref.";

/// `run.py:363-366`: never substituted (COMPATIBILITY §3.3 "Never substituted").
pub(crate) fn config_unstaged(config: &str) -> String {
    format!("Your pre-commit configuration is unstaged.\n`git add {config}` to fix this.")
}

/// `run.py:367-368`.
pub(crate) fn commit_msg_required(stage: &str) -> String {
    format!("`--commit-msg-filename` is required for `--hook-stage {stage}`")
}

/// `run.py:431-435`: plain text, no `[ERROR]` prefix.
pub(crate) fn no_hook(hook: &str, stage: &str) -> String {
    format!("No hook with id `{hook}` in stage `{stage}`")
}

/// snp extension (ADR-080, ADR-084): a `--group`/`--require-group` name that no hook of the config carries — in the
/// style of [`no_hook`], so a typo fails instead of selecting nothing and passing.
pub(crate) fn no_group(group: &str, known: &[&str]) -> String {
    if known.is_empty() {
        format!("No hook in group `{group}`: the config defines no groups")
    } else {
        let names: Vec<String> = known.iter().map(|g| format!("`{g}`")).collect();
        format!(
            "No hook in group `{group}` (known groups: {})",
            names.join(", ")
        )
    }
}

/// `git.py:115` (`logger.info`).
pub(crate) const MERGE_CONFLICT_ONLY: &str = "Checking merge-conflict files only.";

/// `run.py:306-312`: the `--all-files` prelude of `--show-diff-on-failure` (lines 2–3 branded).
pub(crate) fn show_diff_all_files(brand: Brand) -> String {
    let p = brand.prog();
    format!(
        "pre-commit hook(s) made changes.\n\
         If you are seeing this message in CI, reproduce locally with: `{p} run --all-files`.\n\
         To run `{p}` as part of git workflow, use `{p} install`."
    )
}

/// `run.py:313`.
pub(crate) const ALL_CHANGES: &str = "All changes made by hooks:";

/// `hook_impl.py:22-29`: the migration-mode `SystemExit` text (stderr, exit 1).
pub(crate) fn migration_mode(brand: Brand, hook_type: &str) -> String {
    let (who, url) = match brand {
        Brand::PreCommit => (
            "pre-commit",
            "https://github.com/pre-commit/pre-commit/issues",
        ),
        Brand::Snp => ("snp", BUG_URL),
    };
    format!(
        "bug: {who}'s script is installed in migration mode\n\
         run `{who} install -f --hook-type {hook_type}` to fix this\n\
         \n\
         Please report this bug at {url}"
    )
}

/// `hook_impl.py:56-58`: printed with `print()` (stdout).
pub(crate) fn config_not_found_skipping(brand: Brand, config: &str) -> String {
    format!(
        "`{config}` config file not found. Skipping `{}`.",
        brand.prog()
    )
}

/// `hook_impl.py:60-66`: printed with `print()` (stdout), exit 1.
pub(crate) fn no_config_file(brand: Brand, config: &str) -> String {
    let p = brand.prog();
    format!(
        "No {config} file was found\n\
         - To temporarily silence this, run `PRE_COMMIT_ALLOW_NO_CONFIG=1 git ...`\n\
         - To permanently silence this, install {p} with the --allow-missing-config option\n\
         - To uninstall {p} run `{p} uninstall`"
    )
}

/// `hook_impl.py:189-209`: `SystemExit` texts of `_check_args_length` (the list is Python's `repr`).
pub(crate) fn arity(hook_type: &str, expected: &str, n: usize, args_repr: &str) -> String {
    format!("hook-impl for {hook_type} expected {expected} but got {n}: {args_repr}")
}

/// `hook_impl.py:211`: `AssertionError(f'unexpected hook type: {hook_type}')`.
pub(crate) fn unexpected_hook_type(hook_type: &str) -> String {
    format!("unexpected hook type: {hook_type}")
}

/// D-SEC-12 (ADR-076): the 64 MiB stdin cap.
pub(crate) const STDIN_TOO_LARGE: &str =
    "snp: hook stdin exceeds 64 MiB; refusing to run hooks on a truncated ref list";

/// `--stdin-files` over the 64 MiB cap (ADR-076): fail closed, never truncated.
pub(crate) const STDIN_FILES_TOO_LARGE: &str =
    "snp: --stdin-files input exceeds 64 MiB; refusing to run hooks on a truncated file list";

/// D-CRASH-05 (X13-EXE-03): `ArgumentTooLongError` as the hook's failure — the text pre-commit prints when exec
/// fails with `E2BIG` (`exe` is Python's `repr` of the resolved executable).
pub(crate) fn argument_list_too_long(exe: &str) -> String {
    format!("[Errno 7] Argument list too long: {exe}")
}

/// ADR-078: `SNP_VERIFY_BASELINE=1` found a non-empty baseline where the warm path assumed an empty one (stderr).
pub(crate) const BASELINE_MISMATCH: &str =
    "snp: SNP_VERIFY_BASELINE: the elided baseline diff was not empty; using the real one";

/// D-EXT-05 / ADR-038: `SNP=0` while the config sets `allow_disable: false` (stderr).
pub(crate) fn kill_switch_refused(config: &str) -> String {
    format!("snp: SNP=0 ignored: {config} sets allow_disable: false")
}

/// D-CRASH-15 (X13-EXE-05): an unwritable `log_file` (logged as `[WARNING] …`).
pub(crate) fn log_file_failed(path: &str, err: &str) -> String {
    format!("Could not write to log_file {path}: {err}")
}

/// `try_repo.py:30` (`logger.warning`).
pub(crate) const TRY_REPO_SHADOW: &str = "Creating temporary repo with uncommitted changes...";

/// `try_repo.py:72-76`: the 79-`=` rule around the generated config.
pub(crate) fn rule() -> String {
    core::iter::repeat_n('=', 79).collect()
}

/// `try_repo.py:73`.
pub(crate) const USING_CONFIG: &str = "Using config:";

/// `store.py:137` through snp's own clone in try-repo (`logger.info`).
pub(crate) fn initializing(db_repo_name: &str) -> String {
    format!("Initializing environment for {db_repo_name}.")
}

/// D-DIAG-03 (11 §A.14): the TTY-only `run` hint (stderr).
pub(crate) fn run_hint(brand: Brand, hook_path: &str, owner: &str) -> String {
    format!(
        "hint: {hook_path} runs {owner}, not snp; run `{} install` to switch",
        brand.prog()
    )
}

/// ARCHITECTURE §8.2 `root`: the hook's `root` directory is missing.
pub(crate) fn root_missing(root: &str) -> String {
    format!("- root directory '{root}' does not exist")
}

/// ARCHITECTURE §8.2 `timeout`.
pub(crate) fn timed_out(d: Duration) -> String {
    format!("- hook timed out after {}", go_duration(d))
}

/// ARCHITECTURE §8.3 `stage_fixed`: the accepted fixer's detail line.
/// It states the intent only: the staging runs once after every hook, and its refusal (`STAGE_FIXED_REFUSED`) or a
/// withdrawal (`stage_fixed_withdrawn`) is reported then (ADR-084).
pub(crate) const STAGED_FIXES: &str =
    "- files were modified by this hook (stage_fixed: staged after the last hook)";

/// `fail_text` (ARCHITECTURE §8.2): `- <fail_text>`.
pub(crate) fn fail_text(text: &str) -> String {
    format!("- {text}")
}

/// `stage_fixed`: the stash patch would not apply over the staged fixes, so nothing was staged (stderr; the run
/// fails closed).
pub(crate) const STAGE_FIXED_REFUSED: &str =
    "snp: stage_fixed: the unstaged changes would not apply over the fixes; nothing was staged";

/// `stage_fixed`: an accepted fix that a later hook changed again is withdrawn from the staging (stderr, one line
/// per path; ARCHITECTURE §8.3 "files also modified by a non-`stage_fixed` hook are never staged").
pub(crate) fn stage_fixed_withdrawn(path: &str) -> String {
    format!("snp: stage_fixed: {path} was changed by a later hook; not staged")
}

/// Go's `time.Duration.String()` for the durations the `timeout` key accepts (`1m30s`, `500ms`, `2h0m0s`).
pub(crate) fn go_duration(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos == 0 {
        return String::from("0s");
    }
    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return format!("{}µs", trim_frac(nanos, 1_000, 3));
    }
    if nanos < 1_000_000_000 {
        return format!("{}ms", trim_frac(nanos, 1_000_000, 6));
    }
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut out = String::new();
    if h > 0 {
        out.push_str(&h.to_string());
        out.push('h');
        out.push_str(&m.to_string());
        out.push('m');
    } else if m > 0 {
        out.push_str(&m.to_string());
        out.push('m');
    }
    let sub = u128::from(s)
        .saturating_mul(1_000_000_000)
        .saturating_add(u128::from(d.subsec_nanos()));
    out.push_str(&trim_frac(sub, 1_000_000_000, 9));
    out.push('s');
    out
}

/// `n / unit` (`unit` = 10^`digits`) with the fraction printed without trailing zeros (`1.5`, `2`).
fn trim_frac(n: u128, unit: u128, digits: usize) -> String {
    let whole = n.checked_div(unit).unwrap_or(0);
    let rest = n.checked_rem(unit).unwrap_or(0);
    if rest == 0 {
        return whole.to_string();
    }
    let frac = format!("{rest:0digits$}");
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_durations() {
        assert_eq!(go_duration(Duration::from_secs(1)), "1s");
        assert_eq!(go_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(go_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(go_duration(Duration::from_secs(7200)), "2h0m0s");
        assert_eq!(go_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(go_duration(Duration::ZERO), "0s");
    }

    #[test]
    fn branded_texts() {
        assert_eq!(
            migration_mode(Brand::PreCommit, "pre-commit"),
            "bug: pre-commit's script is installed in migration mode\nrun `pre-commit install -f --hook-type \
             pre-commit` to fix this\n\nPlease report this bug at https://github.com/pre-commit/pre-commit/issues"
        );
        assert!(migration_mode(Brand::Snp, "pre-push").starts_with("bug: snp's script"));
        assert_eq!(
            config_not_found_skipping(Brand::PreCommit, ".pre-commit-config.yaml"),
            "`.pre-commit-config.yaml` config file not found. Skipping `pre-commit`."
        );
        assert!(
            no_config_file(Brand::Snp, "x.yaml")
                .ends_with("- To uninstall snp run `snp uninstall`")
        );
        assert!(show_diff_all_files(Brand::Snp).contains("`snp run --all-files`"));
        assert_eq!(rule().len(), 79);
    }
}
