//! The `docker` language backend (§6.2; `pre_commit/languages/docker.py`).
//!
//! pre-commit 4.6.2 builds the hook repository's `Dockerfile` into an image tagged `pre-commit-<md5>` at install time
//! (`docker build --tag <tag> --label PRE_COMMIT --pull .`, cwd = the repository, `docker.py:78-106`), creates the
//! empty environment directory for its state files, and **rebuilds** the image (without `--pull`) before every hook
//! run (`docker.py:159-181`). The hook then runs as
//! `docker run --rm [--tty] [-u uid:gid] -v <cwd>:/src:rw,Z --workdir /src --entrypoint <entry[0]> <tag> <entry[1:]>
//! <args> <files>` (the shared prefix of `common/docker.rs`, also used by `docker_image`).
//!
//! snp differences, all invisible in the hook's output:
//!
//! * the tag hashes the store's repository key instead of the prefix basename, because every snp prefix is named
//!   `src` (ADR-035, [`tag_for_prefix`]);
//! * the per-run rebuild is a [`PreStep`] skipped when `docker image inspect <tag>` succeeds — the image exists, its
//!   context is the immutable checkout — unless `SNP_DOCKER_ALWAYS_BUILD` is set (D-PERF-04);
//! * a failing rootless probe (`docker system info`) means "not rootless" instead of a `CalledProcessError` crash
//!   (D-CRASH-21, `common/docker.rs`).
//!
//! Owner: WP-25.

pub(crate) mod group2;

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use self::group2::{assert_support, entry_error, mkdir_new, os_args, outcome};
use crate::common::{docker as shared, docker_tag, hook_cmd, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    PreStep, RunPlan, Support,
};

/// `docker.PRE_COMMIT_LABEL`.
pub const PRE_COMMIT_LABEL: &str = "PRE_COMMIT";

/// The variable that restores pre-commit's unconditional per-run `docker build` (D-PERF-04).
pub const ALWAYS_BUILD_VAR: &str = "SNP_DOCKER_ALWAYS_BUILD";

/// `language: docker`.
#[derive(Debug)]
pub struct Docker;

/// The image tag of the hook repository checked out at `prefix` (ADR-035).
///
/// pre-commit's `docker_tag(prefix)` is `'pre-commit-' + md5(basename(prefix_dir))` (`docker.py:73-75`). snp's
/// checkouts live at `repos-v1/<repo key hex>/src`, so the basename `src` is replaced by the name of its parent — the
/// repository key — which makes the tag `docker_tag(<key hex>)` ([`crate::common::docker_tag`]): two repositories
/// never share a tag. Any other prefix (tests, foreign layouts) keeps pre-commit's basename rule.
pub fn tag_for_prefix(prefix: &Path) -> String {
    let base = prefix.file_name().unwrap_or_default();
    let key = if base == OsStr::new("src") {
        prefix.parent().and_then(Path::file_name).unwrap_or(base)
    } else {
        base
    };
    docker_tag(&key.to_string_lossy())
}

/// `build_docker_image(prefix, pull=…)` argv (`docker.py:78-92`): `docker build --tag <tag> --label PRE_COMMIT
/// [--pull] .` — the `.` last for old docker versions (#477).
pub fn build_argv(tag: &str, pull: bool) -> Vec<OsString> {
    let mut argv = os_args(["docker", "build", "--tag", tag, "--label", PRE_COMMIT_LABEL]);
    if pull {
        argv.push("--pull".into());
    }
    argv.push(".".into());
    argv
}

/// `_get_container_id()`'s parse (`docker.py:34-42`): the 64-hex container id of the first
/// `/containers[/overlay-containers]/<id>[/userdata]/hostname` mount in a `/proc/1/mountinfo` dump, if any.
///
/// Total on arbitrary bytes (fuzz target `lang_docker_mountinfo`).
pub fn container_id_from_mountinfo(mountinfo: &[u8]) -> Option<String> {
    shared::container_id(mountinfo)
}

/// `container, = json.loads(out); container['Mounts']` of `docker inspect <id>` as `(Destination, Source)` pairs;
/// `None` for anything pre-commit would crash on (fuzz target `lang_docker_mountinfo`).
pub fn mounts_from_inspect(stdout: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    shared::mounts_from_inspect(stdout)
}

/// `_is_rootless()`'s decision over the output of `docker system info --format '{{ json . }}'`; unparsable output is
/// "not rootless" (D-CRASH-21; fuzz target `lang_docker_mountinfo`).
pub fn rootless_from_info(stdout: &[u8]) -> bool {
    shared::rootless_from_info(stdout)
}

impl Language for Docker {
    fn id(&self) -> LanguageId {
        LanguageId::Docker
    }

    /// `assert_version_default('docker', …)` and `assert_no_additional_deps('docker', …)`.
    fn support(&self) -> Support {
        default_support(LanguageId::Docker)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(group2::default_version())
    }

    /// `install_environment` (`docker.py:95-106`): `docker build --tag <tag> --label PRE_COMMIT --pull .` in the
    /// repository (format-A `CalledProcessError` on failure), then `os.mkdir(<env>)` for the state files.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Docker, cx)?;
        let host = cx.cx.host;
        let tag = tag_for_prefix(cx.cx.prefix);
        setup_cmd(
            host.runner,
            &build_argv(&tag, true),
            host.base_env,
            cx.cx.prefix,
        )?;
        mkdir_new(cx.cx.env_dir)?;
        Ok(outcome("docker"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `in_env = lang_base.no_env`.
    fn activate(&self, _cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default())
    }

    /// `run_hook` (`docker.py:159-181`): the rebuild as a [`PreStep`] (`docker build --tag <tag> --label PRE_COMMIT
    /// .` in the repository, skipped when `docker image inspect <tag>` succeeds unless `SNP_DOCKER_ALWAYS_BUILD` is
    /// set), then `docker_cmd(color) + ['--entrypoint', entry[0], tag] + entry[1:] + args`.
    ///
    /// The tag is [`PlanCtx::docker_tag`] when snp-repo provides it (ADR-035), else [`tag_for_prefix`]. An entry that
    /// does not split into a command fails only this hook (C-hook, X13-EXE-01/02).
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        let tag = cx
            .docker_tag
            .map_or_else(|| tag_for_prefix(cx.prefix), str::to_owned);
        let (entry_exe, rest) = match hook_cmd(cx.entry, cx.args, cx.host.self_exe) {
            Ok(cmd) => match cmd.split_first() {
                Some((first, rest)) => (first.clone(), rest.to_vec()),
                None => return Ok(entry_error(&snp_proc::hookcmd::HookCmdError::Empty)),
            },
            Err(e) => return Ok(entry_error(&e)),
        };
        let mut cmd = shared::docker_cmd(cx.host, cx.color)?;
        cmd.push("--entrypoint".into());
        cmd.push(entry_exe);
        cmd.push(OsString::from(&tag));
        cmd.extend(rest);
        let skip_if_ok = (!cx.host.base_env.is_truthy_flag(ALWAYS_BUILD_VAR))
            .then(|| os_args(["docker", "image", "inspect", &tag]));
        let pre = vec![PreStep {
            argv: build_argv(&tag, false),
            env: cx.env.clone(),
            cwd: cx.prefix.to_path_buf(),
            skip_if_ok,
        }];
        Ok(RunPlan::Xargs {
            cmd: Arc::from(cmd),
            env: cx.env.clone(),
            shuffle: true,
            serial: false,
            pre,
        })
    }
}
