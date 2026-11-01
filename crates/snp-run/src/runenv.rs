//! `run_env` (ARCHITECTURE §2.5, §3.1 step 8): the startup environment plus the variables `run` exports to hooks
//! (`run.py:375-417`, 02 §6.1).
//!
//! Owner: WP-32.

use std::ffi::{OsStr, OsString};

use snp_cli::RunArgs;
use snp_core::env::{Env, EnvPatch};

/// Python truthiness of an optional CLI string (`None` and `''` are false).
fn truthy(v: Option<&OsString>) -> Option<&OsStr> {
    v.map(OsString::as_os_str).filter(|s| !s.is_empty())
}

/// The patch `run` applies to `os.environ` before the stash (`run.py:375-417`): every variable only when its value
/// is truthy, the from/to pair only together, the pre-rebase pair only together, the four push variables only when
/// all four are given, and `PRE_COMMIT=1` always.
///
/// The naming trap is kept: the legacy `PRE_COMMIT_ORIGIN` carries **`from_ref`** and `PRE_COMMIT_SOURCE` **`to_ref`**.
pub(crate) fn run_vars(a: &RunArgs) -> EnvPatch {
    let mut p = EnvPatch::default();
    if let Some(v) = truthy(a.prepare_commit_message_source.as_ref()) {
        p = p.set("PRE_COMMIT_COMMIT_MSG_SOURCE", v);
    }
    if let Some(v) = truthy(a.commit_object_name.as_ref()) {
        p = p.set("PRE_COMMIT_COMMIT_OBJECT_NAME", v);
    }
    if let (Some(from), Some(to)) = (truthy(a.from_ref.as_ref()), truthy(a.to_ref.as_ref())) {
        p = p
            .set("PRE_COMMIT_ORIGIN", from)
            .set("PRE_COMMIT_SOURCE", to)
            .set("PRE_COMMIT_FROM_REF", from)
            .set("PRE_COMMIT_TO_REF", to);
    }
    if let (Some(up), Some(branch)) = (
        truthy(a.pre_rebase_upstream.as_ref()),
        truthy(a.pre_rebase_branch.as_ref()),
    ) {
        p = p
            .set("PRE_COMMIT_PRE_REBASE_UPSTREAM", up)
            .set("PRE_COMMIT_PRE_REBASE_BRANCH", branch);
    }
    if let (Some(name), Some(url), Some(remote_branch), Some(local_branch)) = (
        truthy(a.remote_name.as_ref()),
        truthy(a.remote_url.as_ref()),
        truthy(a.remote_branch.as_ref()),
        truthy(a.local_branch.as_ref()),
    ) {
        p = p
            .set("PRE_COMMIT_LOCAL_BRANCH", local_branch)
            .set("PRE_COMMIT_REMOTE_BRANCH", remote_branch)
            .set("PRE_COMMIT_REMOTE_NAME", name)
            .set("PRE_COMMIT_REMOTE_URL", url);
    }
    if let Some(v) = truthy(a.checkout_type.as_ref()) {
        p = p.set("PRE_COMMIT_CHECKOUT_TYPE", v);
    }
    if let Some(v) = truthy(a.is_squash_merge.as_ref()) {
        p = p.set("PRE_COMMIT_IS_SQUASH_MERGE", v);
    }
    if let Some(v) = truthy(a.rewrite_command.as_ref()) {
        p = p.set("PRE_COMMIT_REWRITE_COMMAND", v);
    }
    p.set("PRE_COMMIT", "1")
}

/// `startup ⊕ run vars` (§2.5).
pub(crate) fn run_env(startup: &Env, a: &RunArgs) -> Env {
    startup.patched(&run_vars(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snp_core::output::ColorSetting;
    use snp_core::stage::{HookType, Stage};

    pub(crate) fn args() -> RunArgs {
        RunArgs {
            color: ColorSetting::Never,
            config: OsString::from(".pre-commit-config.yaml"),
            hooks: Vec::new(),
            verbose: false,
            all_files: false,
            files: Vec::new(),
            show_diff_on_failure: false,
            fail_fast: false,
            hook_stage: Stage::Hook(HookType::PreCommit),
            remote_branch: None,
            local_branch: None,
            from_ref: None,
            to_ref: None,
            pre_rebase_upstream: None,
            pre_rebase_branch: None,
            commit_msg_filename: None,
            prepare_commit_message_source: None,
            commit_object_name: None,
            remote_name: None,
            remote_url: None,
            checkout_type: None,
            is_squash_merge: None,
            rewrite_command: None,
            ext: snp_cli::RunExt::default(),
        }
    }

    fn get(e: &Env, k: &str) -> Option<String> {
        e.get(k).map(|v| v.to_string_lossy().into_owned())
    }

    #[test]
    fn t10_run_21_exported_variables() {
        let base = Env::from_pairs([("PATH", "/usr/bin")]);
        let mut a = args();
        a.is_squash_merge = Some("1".into());
        a.rewrite_command = Some("amend".into());
        a.checkout_type = Some("0".into());
        a.pre_rebase_upstream = Some("master".into());
        let e = run_env(&base, &a);
        assert_eq!(get(&e, "PRE_COMMIT").as_deref(), Some("1"));
        assert_eq!(get(&e, "PRE_COMMIT_IS_SQUASH_MERGE").as_deref(), Some("1"));
        assert_eq!(
            get(&e, "PRE_COMMIT_REWRITE_COMMAND").as_deref(),
            Some("amend")
        );
        // "0" is a non-empty string: truthy
        assert_eq!(get(&e, "PRE_COMMIT_CHECKOUT_TYPE").as_deref(), Some("0"));
        // only one of the pre-rebase pair: neither is exported
        assert_eq!(get(&e, "PRE_COMMIT_PRE_REBASE_UPSTREAM"), None);
        a.pre_rebase_branch = Some("topic".into());
        a.from_ref = Some("A".into());
        a.to_ref = Some("B".into());
        a.remote_name = Some("origin".into());
        a.remote_url = Some("u".into());
        a.remote_branch = Some("refs/heads/main".into());
        let e = run_env(&base, &a);
        assert_eq!(
            get(&e, "PRE_COMMIT_PRE_REBASE_BRANCH").as_deref(),
            Some("topic")
        );
        assert_eq!(get(&e, "PRE_COMMIT_ORIGIN").as_deref(), Some("A"));
        assert_eq!(get(&e, "PRE_COMMIT_SOURCE").as_deref(), Some("B"));
        assert_eq!(get(&e, "PRE_COMMIT_FROM_REF").as_deref(), Some("A"));
        assert_eq!(get(&e, "PRE_COMMIT_TO_REF").as_deref(), Some("B"));
        // three of the four push variables: none exported
        assert_eq!(get(&e, "PRE_COMMIT_REMOTE_NAME"), None);
        a.local_branch = Some("refs/heads/main".into());
        let e = run_env(&base, &a);
        assert_eq!(get(&e, "PRE_COMMIT_REMOTE_NAME").as_deref(), Some("origin"));
        assert_eq!(
            get(&e, "PRE_COMMIT_LOCAL_BRANCH").as_deref(),
            Some("refs/heads/main")
        );
        // an empty string is falsy
        a.commit_object_name = Some(OsString::new());
        assert_eq!(
            get(&run_env(&base, &a), "PRE_COMMIT_COMMIT_OBJECT_NAME"),
            None
        );
        assert_eq!(get(&e, "PATH").as_deref(), Some("/usr/bin"));
    }
}
