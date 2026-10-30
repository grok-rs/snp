//! A scenario fixture: a temporary root with the repository (the run's cwd), a hook-repository prefix, an `envs`
//! directory, a tool directory on `PATH`, a temporary directory and a home, and a `TestHost` recording every command.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::Env;
use snp_core::lang::LanguageId;
use snp_lang::harness::TestHost;
use snp_lang::{
    EnvCtx, Health, InstallCtx, InstallOutcome, InstallState, LOCAL_TEMPLATE, LangError, PlanCtx,
    RunPlan, ToolProbe, backend,
};

/// What `docker system info --format '{{ json . }}'` answers in the scenarios (not rootless).
pub(crate) const DOCKER_INFO: &str = r#"{"SecurityOptions": ["name=seccomp,profile=builtin"]}"#;

/// The fixture.
pub(crate) struct Fx {
    /// Keeps the root alive.
    _dir: tempfile::TempDir,
    /// The repository (cwd).
    pub(crate) repo: PathBuf,
    /// The hook repository.
    pub(crate) prefix: PathBuf,
    /// Where the environments go.
    pub(crate) envs: PathBuf,
    /// The tool directory first on `PATH`.
    pub(crate) shims: PathBuf,
    /// `TMPDIR`.
    pub(crate) tmp: PathBuf,
    /// `HOME`.
    pub(crate) home: PathBuf,
    /// The host.
    pub(crate) t: TestHost,
}

/// `(name, value)` pairs as owned strings.
pub(crate) fn pairs(xs: &[(&str, &str)]) -> Vec<(String, String)> {
    xs.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// Deps as `Arc<str>`s.
pub(crate) fn arcs(xs: &[&str]) -> Vec<Arc<str>> {
    xs.iter().map(|d| Arc::from(*d)).collect()
}

impl Fx {
    /// A fixture whose prefix holds pre-commit's local template; `extra` is added to the base environment.
    pub(crate) fn new(extra: &[(&str, &str)]) -> Self {
        Self::with_template(extra, true)
    }

    /// A fixture with an empty or templated prefix.
    pub(crate) fn with_template(extra: &[(&str, &str)], template: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = root.join("repo");
        let prefix = root.join("store").join("src");
        let envs = root.join("store").join("envs");
        let shims = root.join("shims");
        let tmp = root.join("tmp");
        let home = root.join("home");
        for d in [&repo, &prefix, &envs, &shims, &tmp, &home] {
            std::fs::create_dir_all(d).unwrap();
        }
        if template {
            for (rel, bytes) in LOCAL_TEMPLATE {
                let p = prefix.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, bytes).unwrap();
            }
        }
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path = format!("{}{sep}/usr/bin{sep}/bin", shims.display());
        let mut base = vec![
            ("PATH".to_owned(), path),
            ("HOME".to_owned(), home.display().to_string()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TMPDIR".to_owned(), tmp.display().to_string()),
            ("PRE_COMMIT".to_owned(), "1".to_owned()),
        ];
        base.extend(pairs(extra));
        let mut t = TestHost::new(&repo, Env::from_pairs(base));
        let empty = root.join(".no-mountinfo");
        std::fs::write(&empty, b"").unwrap();
        t.probe = ToolProbe::new(None).with_mountinfo(empty);
        Self {
            _dir: dir,
            repo,
            prefix,
            envs,
            shims,
            tmp,
            home,
            t,
        }
    }

    /// `envs/<ENV_DIR>-<version>`.
    pub(crate) fn env_dir(&self, id: LanguageId, version: &str) -> PathBuf {
        self.envs
            .join(format!("{}-{version}", id.env_dir_base().unwrap()))
    }

    /// Writes `bytes` to `rel` in the prefix.
    pub(crate) fn put(&self, rel: &str, bytes: &[u8]) {
        let p = self.prefix.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// An executable file `name` in the tool directory.
    pub(crate) fn tool(&self, name: &str) -> PathBuf {
        let p = self.shims.join(name);
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    /// `backend(id).install(…)` of version `version` with `deps`.
    pub(crate) fn install_v(
        &self,
        id: LanguageId,
        version: &str,
        deps: &[&str],
    ) -> Result<InstallOutcome, LangError> {
        let host = self.t.host();
        let env_dir = self.env_dir(id, version);
        let deps = arcs(deps);
        backend(id).install(&InstallCtx {
            cx: EnvCtx {
                host: &host,
                prefix: &self.prefix,
                version,
                env_dir: &env_dir,
            },
            deps: &deps,
            is_local: true,
        })
    }

    /// `install` of the default version.
    pub(crate) fn install(&self, id: LanguageId, deps: &[&str]) -> Result<InstallOutcome, LangError> {
        self.install_v(id, "default", deps)
    }

    /// `health(recorded)` of the default environment.
    pub(crate) fn health(
        &self,
        id: LanguageId,
        recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        let host = self.t.host();
        let env_dir = self.env_dir(id, "default");
        backend(id).health(
            &EnvCtx {
                host: &host,
                prefix: &self.prefix,
                version: "default",
                env_dir: &env_dir,
            },
            recorded,
        )
    }

    /// The activated environment of the default env dir.
    pub(crate) fn activated(&self, id: LanguageId) -> Result<Env, LangError> {
        let host = self.t.host();
        let env_dir = self.env_dir(id, "default");
        let patch = backend(id).activate(&EnvCtx {
            host: &host,
            prefix: &self.prefix,
            version: "default",
            env_dir: &env_dir,
        })?;
        Ok(self.t.env.patched(&patch))
    }

    /// `plan` of a hook (`env` = the activated environment).
    pub(crate) fn plan(
        &self,
        id: LanguageId,
        entry: &str,
        args: &[&str],
        is_local: bool,
        env: &Env,
    ) -> Result<RunPlan, LangError> {
        self.plan_full(id, entry, args, is_local, false, None, env)
    }

    /// `plan` with every knob.
    #[expect(clippy::too_many_arguments, reason = "test helper mirroring PlanCtx")]
    pub(crate) fn plan_full(
        &self,
        id: LanguageId,
        entry: &str,
        args: &[&str],
        is_local: bool,
        color: bool,
        docker_tag: Option<&str>,
        env: &Env,
    ) -> Result<RunPlan, LangError> {
        let host = self.t.host();
        let env_dir = self.env_dir(id, "default");
        let args = arcs(args);
        backend(id).plan(&PlanCtx {
            host: &host,
            prefix: &self.prefix,
            version: "default",
            env_dir: Some(&env_dir),
            env,
            entry,
            args: &args,
            is_local,
            color,
            docker_tag,
        })
    }
}

/// The argv of an `Xargs` plan as strings.
pub(crate) fn xargs_argv(plan: &RunPlan) -> Vec<String> {
    match plan {
        RunPlan::Xargs { cmd, .. } => cmd
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        other => panic!("not an xargs plan: {other:?}"),
    }
}

/// The argv of recorded calls as strings (tool = basename).
pub(crate) fn call_argvs(t: &TestHost) -> Vec<Vec<String>> {
    t.runner
        .calls()
        .iter()
        .map(|c| {
            let mut v: Vec<String> = c
                .argv
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            if let Some(first) = v.first_mut() {
                *first = snp_lang::harness::tool_name(&OsString::from(first.clone()));
            }
            v
        })
        .collect()
}

/// `p` as a string.
pub(crate) fn s(p: &Path) -> String {
    p.display().to_string()
}
