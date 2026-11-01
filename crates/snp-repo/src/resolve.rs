//! `all_hooks()`: every repository in config order, clones (with prefetch), manifests, merge, language defaults,
//! env-less guards and the fast-path decisions (`repository.py:109-237`, `store.py:132-215`, ARCHITECTURE §5.3,
//! §7.2, ADR-025, ADR-032, ADR-064, ADR-070).
//!
//! **Transcript.** Every line pre-commit prints while building hooks is printed at pre-commit's moment and in config
//! order:
//!
//! * `[INFO] Initializing environment for {db_repo_name}.` when the store sees `(db_repo_name, rev)` for the first
//!   time (`keys-v1` marker absent; `store.py:142-171`), decided under the repository lock once the entry is
//!   confirmed missing, then the clone, the deprecated-stage warning of the new checkout (`clientlib.py:84-109`) and
//!   the marker — for the base clone of a remote repository, for every `additional_dependencies` variant (in the
//!   per-hook loop, `repository.py:193-200`) and for the local template of a `repo: local` hook with an environment
//!   (`repository.py:152-159`). A Tier A pre-commit-hooks repository prints the same lines whether or not it is
//!   cloned: after the clone a delegated hook needs, else without one.
//! * `[ERROR] … is not present in repository …` (E1) and the env-less guards (E2/E3), then exit 1.
//! * `[WARNING] Unexpected key(s) present on …` (W11) after the hook's own clone, buffered per hook so a remote
//!   repository whose later hook fails a guard prints no W11 at all (`_hook` runs for every hook before any
//!   `Hook.create`).
//!
//! Owner: WP-31.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_config::overlay::HookPatch;
use snp_config::{
    Config, ConfigHook, ExtMode, HookDef, LoadOpts, Manifest, MergeError, RepoConfig,
};
use snp_core::cancel::CancelReason;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::output::{CollectWarnings, Level, WarnSink};
use snp_git::{CloneMode, OwnedRepo};
use snp_hooks_data::{Epoch, HookClass, NativeSource, TagProfile};
use snp_proc::spawn::{Group, Stdin};
use snp_store::{MarkerKey, RepoHandle, RepoId, RepoKey, RepoKind, RepoMeta, Store};

use crate::fastpath::{self, CloneFacts, GateInput, Policy, Verdict};
use crate::prefetch::{self, Job, Prefetch};
use crate::{ExecKind, HookSrc, InstallKey, MetaHook, ResolveCtx, ResolvedHook, messages, stages};

/// `C.MANIFEST_FILE`.
pub(crate) const MANIFEST_FILE: &str = ".pre-commit-hooks.yaml";
/// `C.DEFAULT`.
const DEFAULT: &str = "default";
/// The largest manifest the deprecated-stage scan reads (pre-commit reads any size; real manifests are a few KiB).
const MANIFEST_CAP: u64 = 16 << 20;

/// The fixed data of one remote repository, computed once (prefetch planning and resolution share it).
#[derive(Debug, Clone)]
pub(crate) struct RemotePlan {
    /// `RepoId::from_config(repo, project_root)`.
    pub(crate) id: RepoId,
    /// The base clone (no dependencies).
    pub(crate) base_key: RepoKey,
    /// What `git remote add origin` receives (a local directory joined to the project root).
    pub(crate) url: String,
    /// Tier A profile (fast path on, known tag).
    pub(crate) profile: Option<&'static TagProfile>,
}

impl RemotePlan {
    /// The plan of `repo @ rev` for `cx`.
    pub(crate) fn new(cx: &ResolveCtx<'_>, policy: &Policy, repo: &str, rev: &str) -> Self {
        let root = &cx.cfg.source.project_root;
        let id = RepoId::from_config(repo, root);
        let base_key = Store::remote_key(&id, rev, &[]);
        Self {
            id,
            base_key,
            url: clone_url(repo, root),
            profile: fastpath::tier_a(policy, repo, rev),
        }
    }
}

/// pre-commit resolves a relative local `repo` against its cwd (the project root) and passes `abspath(repo)` when it
/// is a directory (`git.init_repo`); snp-git never reads the process cwd, so the join happens here.
pub(crate) fn clone_url(repo: &str, root: &Path) -> String {
    let given = Path::new(repo);
    if !repo.is_empty() && given.is_relative() && root.join(given).is_dir() {
        return root.join(given).to_string_lossy().into_owned();
    }
    repo.to_owned()
}

/// The `Locking … directory` notice (printed at most once per process by the store).
pub(crate) fn lock_notice<'c>(cx: &'c ResolveCtx<'_>) -> impl FnMut() + 'c {
    move || {
        cx.printer
            .log(Level::Info, snp_store::locking_message(cx.brand));
    }
}

/// `Failure::Interrupted` with the token's reason.
pub(crate) fn interrupted(cx: &ResolveCtx<'_>) -> Failure {
    Failure::Interrupted(cx.cancel.reason().unwrap_or(CancelReason::Interrupt))
}

/// The load options for manifests (manifest schema; extension keys never apply).
fn load_opts<'c>(cx: &'c ResolveCtx<'_>, warn: &'c dyn WarnSink) -> LoadOpts<'c> {
    LoadOpts {
        brand: cx.brand,
        ext: ExtMode::Enabled,
        regexes: cx.regexes,
        warn,
        diag: cx.printer,
    }
}

/// E1–E3: `logger.error(…)` then `exit(1)` (`repository.py:126-142,182-187`).
pub(crate) fn merge_failure(cx: &ResolveCtx<'_>, e: &MergeError) -> Failure {
    cx.printer.log(Level::Error, &e.message(cx.brand));
    Failure::Exit(1)
}

/// The meta hook of a `repo: meta` id.
fn meta_hook(id: &str) -> Option<MetaHook> {
    match id {
        "check-hooks-apply" => Some(MetaHook::CheckHooksApply),
        "check-useless-excludes" => Some(MetaHook::CheckUselessExcludes),
        "identity" => Some(MetaHook::Identity),
        _ => None,
    }
}

/// The `alias` the merged hook will have (the config's, else the manifest's), for the overlay lookup.
fn merged_alias<'h>(c: &'h ConfigHook, m: &'h HookDef) -> &'h str {
    c.raw
        .iter()
        .find(|(k, _)| k.as_str() == Some("alias"))
        .and_then(|(_, v)| v.as_str())
        .unwrap_or(&m.alias)
}

/// Flushes a per-hook warning buffer to the real sink.
fn flush(buf: &CollectWarnings, warn: &dyn WarnSink) {
    for w in buf.take() {
        warn.warn(&w);
    }
}

/// The `language_version` after `languages[lang].get_default_version()` (`repository.py:120-123`), memoised by the
/// backend through `ToolProbe`; env-less languages keep `default` (their `get_default_version` is
/// `lang_base.basic_get_default_version`).
pub(crate) fn default_version(
    cx: &ResolveCtx<'_>,
    lang: LanguageId,
    version: &Arc<str>,
) -> Result<Arc<str>, Failure> {
    if &**version != DEFAULT || lang.env_dir_base().is_none() {
        return Ok(Arc::clone(version));
    }
    // The backends memoise through `ToolProbe::default_version` themselves; wrapping the call in the same memo
    // again would wait on the slot this very call holds.
    snp_lang::backend(lang)
        .default_version(&cx.host)
        .map_err(Failure::from)
}

/// The `InstallKey` of a hook with an environment on `repo` (`hook.install_key`, `hook.py:39-46`).
pub(crate) fn install_key(repo: RepoKey, def: &HookDef) -> Option<InstallKey> {
    def.language.env_dir_base()?;
    Some(InstallKey {
        repo,
        lang: def.language,
        version: Arc::clone(&def.language_version),
        deps: Arc::clone(&def.additional_dependencies),
    })
}

/// docker's image tag (ADR-035) for hooks of a docker repository.
fn docker_tag(key: RepoKey, lang: LanguageId) -> Option<Arc<str>> {
    (lang == LanguageId::Docker).then(|| Arc::from(snp_lang::common::docker_tag(&key.hex())))
}

/// `git config --get core.sshCommand` in the project (ADR-064: an ssh URL is prefetched only when no ssh command
/// is configured). A failure to ask counts as "set" (no prefetch).
fn core_ssh_command_set(cx: &ResolveCtx<'_>) -> bool {
    let mut argv: Vec<OsString> = cx.git_exe.argv0.clone();
    argv.extend(["config", "--get", "core.sshCommand"].map(OsString::from));
    match cx.git_backend.capture(
        &argv,
        cx.env,
        &cx.cfg.source.project_root,
        Stdin::Null,
        Group::Inherit,
    ) {
        Ok(out) => out.status.0 == 0,
        Err(_) => true,
    }
}

/// Whether a URL is an ssh/scp one (the only kind whose eligibility needs the git config).
fn needs_ssh_probe(url: &str) -> bool {
    let probe_env = snp_core::env::Env::from_pairs::<OsString, OsString>([]);
    snp_git::prefetch_eligible(url, &probe_env, false)
        && !snp_git::prefetch_eligible(url, &probe_env, true)
}

/// The prefetch jobs of `cfg` (ADR-064): missing, `prefetch_eligible` base clones of non-Tier-A remote
/// repositories, deduplicated, in config order; none with `SNP_PREFETCH=0` or fewer than two clones to make.
pub(crate) fn prefetch_jobs(cx: &ResolveCtx<'_>, plans: &[Option<RemotePlan>]) -> Vec<Job> {
    if cx
        .env
        .get("SNP_PREFETCH")
        .is_some_and(|v| v == OsStr::new("0"))
    {
        return Vec::new();
    }
    let mut missing: Vec<(&RemotePlan, &Arc<str>)> = Vec::new();
    for (plan, repo) in plans.iter().zip(&cx.cfg.repos) {
        let (Some(plan), RepoConfig::Remote { rev, .. }) = (plan, repo) else {
            continue;
        };
        if plan.profile.is_some() || missing.iter().any(|(p, _)| p.base_key == plan.base_key) {
            continue;
        }
        if cx.store.lookup(&plan.base_key).is_none() {
            missing.push((plan, rev));
        }
    }
    if missing.len() < 2 {
        return Vec::new();
    }
    let mut ssh_set: Option<bool> = None;
    let mut jobs = Vec::new();
    for (plan, rev) in missing {
        let ssh = if needs_ssh_probe(&plan.url) {
            *ssh_set.get_or_insert_with(|| core_ssh_command_set(cx))
        } else {
            false
        };
        if snp_git::prefetch_eligible(&plan.url, cx.env, ssh) {
            jobs.push(Job::new(plan.base_key, plan.url.clone(), Arc::clone(rev)));
        }
    }
    jobs
}

/// One `all_hooks()` pass.
#[derive(Debug)]
pub(crate) struct Resolution<'c, 'a> {
    /// The context.
    pub(crate) cx: &'c ResolveCtx<'a>,
    /// G1 inputs.
    pub(crate) policy: Policy,
    /// The remote plans, aligned with `cfg.repos`.
    pub(crate) plans: Vec<Option<RemotePlan>>,
    /// The background clones.
    pub(crate) prefetch: Option<&'c Prefetch>,
    /// Whether `ensure_created` succeeded.
    created: bool,
    /// The hooks so far.
    out: Vec<ResolvedHook>,
    /// The next flattened config index.
    index: u32,
}

impl<'c, 'a> Resolution<'c, 'a> {
    /// A pass over `cx.cfg`.
    pub(crate) fn new(
        cx: &'c ResolveCtx<'a>,
        policy: Policy,
        plans: Vec<Option<RemotePlan>>,
        prefetch: Option<&'c Prefetch>,
    ) -> Self {
        Self {
            cx,
            policy,
            plans,
            prefetch,
            created: false,
            out: Vec::new(),
            index: 0,
        }
    }

    /// Creates the store on first need (pre-commit's `Store()` always exists; snp's is lazy, X13-ST-01).
    pub(crate) fn ensure_store(&mut self) -> Result<(), Failure> {
        if !self.created {
            self.cx
                .store
                .ensure_created(self.cx.brand, self.cx.printer)?;
            self.created = true;
        }
        Ok(())
    }

    /// Every repository in config order (`all_hooks`, `repository.py:232-237`).
    pub(crate) fn run(mut self) -> Result<Vec<ResolvedHook>, Failure> {
        let cfg = self.cx.cfg;
        for (i, repo) in cfg.repos.iter().enumerate() {
            if self.cx.cancel.is_cancelled() {
                return Err(interrupted(self.cx));
            }
            match repo {
                RepoConfig::Local { hooks } => {
                    for h in hooks {
                        self.local(h)?;
                    }
                }
                RepoConfig::Meta { hooks } => {
                    for h in hooks {
                        self.meta(h)?;
                    }
                }
                RepoConfig::Builtin { hooks } => self.builtin(hooks)?,
                RepoConfig::Remote { repo, rev, hooks } => {
                    let plan = self
                        .plans
                        .get(i)
                        .and_then(Option::clone)
                        .unwrap_or_else(|| RemotePlan::new(self.cx, &self.policy, repo, rev));
                    self.remote(&plan, repo, rev, hooks)?;
                }
                _ => {}
            }
        }
        self.report_unmatched();
        Ok(self.out)
    }

    /// The overlay keys no resolved hook matched (`snp: local config overrides unknown hook '<id>'`, stderr).
    fn report_unmatched(&self) {
        let Some(ov) = self.cx.overlay else {
            return;
        };
        let mut names: Vec<&str> = Vec::new();
        for h in &self.out {
            names.push(&h.def.id);
            if !h.def.alias.is_empty() {
                names.push(&h.def.alias);
            }
        }
        for id in ov.unmatched(&names) {
            self.cx
                .printer
                .diag(&snp_config::overlay::unknown_hook_note(id));
        }
    }

    /// The overlay patch of a hook.
    fn patch(&self, id: &str, alias: &str) -> Option<&'c HookPatch> {
        self.cx.overlay.and_then(|ov| ov.hook_patch(id, alias))
    }

    /// The next flattened index.
    fn next_index(&mut self) -> u32 {
        let i = self.index;
        self.index = self.index.saturating_add(1);
        i
    }

    /// `store._new_repo` (`store.py:132-171`) around [`Store::materialize`]: under the repository lock, once the
    /// entry is confirmed missing, `[INFO] Initializing environment for {name}.` when the store has never seen
    /// `(name, rev)` (`keys-v1` marker absent), then `fill`, then the deprecated-stage warning of the new checkout,
    /// then the marker — so a failing clone records nothing and a process that waited for another one's clone
    /// prints only the `Locking` line. An entry that is already present prints nothing (pre-commit's row exists).
    fn materialize_announced(
        &mut self,
        key: &RepoKey,
        meta: RepoMeta,
        a: Announce<'_>,
        fill: &mut dyn FnMut(&Path) -> Result<(), Failure>,
    ) -> Result<RepoHandle, Failure> {
        self.ensure_store()?;
        let cx = self.cx;
        let mut ran = false;
        let mut marked = false;
        let r = {
            let mut wrapped = |dest: &Path| -> Result<(), Failure> {
                ran = true;
                let new = !initialized(cx, a.name, a.rev);
                if new {
                    cx.printer.log(Level::Info, &messages::initializing(a.name));
                }
                fill(dest)?;
                if new {
                    if let Some(repo) = a.stages_of {
                        warn_stages_of(cx, repo, dest);
                    }
                    cx.store.set_marker(&init_key(a.name, a.rev));
                    marked = true;
                }
                Ok(())
            };
            cx.store
                .materialize(key, meta, &mut wrapped, &mut lock_notice(cx), cx.cancel)
        };
        match &r {
            // The entry never reached the store (a failed rename or marker write): neither did the db row.
            Err(_) if marked => forget(cx, a.name, a.rev),
            Ok(_) if !ran && !initialized(cx, a.name, a.rev) => {
                cx.store.set_marker(&init_key(a.name, a.rev));
            }
            _ => {}
        }
        r
    }

    /// The `Initializing` line and the deprecated-stage warning of a Tier A repository that is not cloned
    /// (ADR-032): printed exactly when pre-commit's clone would print them, under the repository lock like
    /// `_new_repo`'s second query.
    fn announce_uncloned(
        &mut self,
        plan: &RemotePlan,
        repo: &str,
        rev: &str,
        manifest: &[u8],
    ) -> Result<(), Failure> {
        let cx = self.cx;
        if initialized(cx, repo, rev) {
            return Ok(());
        }
        self.ensure_store()?;
        let _lock = cx
            .store
            .repo_lock(&plan.base_key, &mut lock_notice(cx), cx.cancel)?;
        if initialized(cx, repo, rev) {
            return Ok(());
        }
        cx.printer.log(Level::Info, &messages::initializing(repo));
        warn_stages(cx, repo, manifest);
        cx.store.set_marker(&init_key(repo, rev));
        Ok(())
    }

    /// The base clone of a remote repository (`store.clone(repo, rev)`): the store entry, the finished prefetch, or
    /// pre-commit's foreground clone.
    fn materialize_remote(
        &mut self,
        plan: &RemotePlan,
        repo: &str,
        rev: &str,
    ) -> Result<RepoHandle, Failure> {
        let cx = self.cx;
        let meta = RepoMeta {
            kind: RepoKind::Remote {
                id: plan.id.clone(),
                rev: Arc::from(rev),
            },
            deps: Arc::from([]),
        };
        let prefetch = self.prefetch;
        let scratch = cx.store.scratch_dir();
        let mut fill = |dest: &Path| -> Result<(), Failure> {
            if let Some(dir) = prefetch.and_then(|p| p.take(&plan.base_key, cx))
                && prefetch::adopt(&dir, dest, &scratch)
            {
                return Ok(());
            }
            OwnedRepo::clone_rev(
                Arc::clone(cx.git_exe),
                cx.env,
                dest,
                &plan.url,
                rev,
                CloneMode::Foreground,
                Arc::clone(&cx.git_backend),
            )
            .map(drop)
        };
        let a = Announce {
            name: repo,
            rev,
            stages_of: Some(repo),
        };
        self.materialize_announced(&plan.base_key, meta, a, &mut fill)
    }

    /// A deps variant (ADR-025): `git clone --local` of the base, falling back to the network clone.
    fn materialize_variant(
        &mut self,
        plan: &RemotePlan,
        base: &RepoHandle,
        repo: &str,
        rev: &str,
        deps: &Arc<[Arc<str>]>,
    ) -> Result<RepoHandle, Failure> {
        let cx = self.cx;
        let key = Store::remote_key(&plan.id, rev, deps);
        let meta = RepoMeta {
            kind: RepoKind::Remote {
                id: plan.id.clone(),
                rev: Arc::from(rev),
            },
            deps: Arc::clone(deps),
        };
        let mut fill = |dest: &Path| -> Result<(), Failure> {
            OwnedRepo::clone_local(
                Arc::clone(cx.git_exe),
                cx.env,
                &base.src,
                dest,
                rev,
                Arc::clone(&cx.git_backend),
            )
            .map(drop)
        };
        let name = snp_store::db_repo_name(repo, deps);
        let a = Announce {
            name: &name,
            rev,
            stages_of: Some(repo),
        };
        self.materialize_announced(&key, meta, a, &mut fill)
    }

    /// `store.make_local(deps)` (`store.py:212-215`): the 4.6.2 local template.
    fn materialize_local(&mut self, deps: &Arc<[Arc<str>]>) -> Result<RepoHandle, Failure> {
        let key = Store::local_key(deps);
        let meta = RepoMeta {
            kind: RepoKind::Local,
            deps: Arc::clone(deps),
        };
        let name = snp_store::db_repo_name("local", deps);
        let rev = snp_core::profile::LOCAL_TEMPLATE_VERSION.to_string();
        let a = Announce {
            name: &name,
            rev: &rev,
            // `warn_for_stages_on_repo_init('local', …)` finds no manifest there.
            stages_of: None,
        };
        self.materialize_announced(&key, meta, a, &mut |dest| write_local_template(dest))
    }

    /// A `repo: local` hook (`_non_cloned_repository_hooks`, `repository.py:147-168`).
    fn local(&mut self, h: &HookDef) -> Result<(), Failure> {
        let cx = self.cx;
        let buf = CollectWarnings::new();
        let patch = self.patch(&h.id, &h.alias);
        let mut def = snp_config::finalize_local(h, cx.cfg, "local", patch, &buf)
            .map_err(|e| merge_failure(cx, &e))?;
        def.language_version = default_version(cx, def.language, &def.language_version)?;
        let (prefix, key, repo) = if def.language.env_dir_base().is_some() {
            let handle = self.materialize_local(&def.additional_dependencies)?;
            (handle.src.clone(), Some(handle.key), Some(handle))
        } else {
            (cx.cfg.source.project_root.clone(), None, None)
        };
        flush(&buf, cx.warn);
        let index = self.next_index();
        let install_key = key.and_then(|k| install_key(k, &def));
        self.out.push(ResolvedHook {
            index,
            src: HookSrc::Local,
            docker_tag: key.and_then(|k| docker_tag(k, def.language)),
            language_version: Arc::clone(&def.language_version),
            def: Arc::new(def),
            prefix,
            repo,
            exec: ExecKind::Language,
            install_key,
            virtual_install: None,
        });
        Ok(())
    }

    /// A `repo: meta` hook: language `unsupported`, prefix = the project root.
    fn meta(&mut self, h: &HookDef) -> Result<(), Failure> {
        let cx = self.cx;
        let patch = self.patch(&h.id, &h.alias);
        let mut def = snp_config::finalize_local(h, cx.cfg, "meta", patch, cx.warn)
            .map_err(|e| merge_failure(cx, &e))?;
        def.language_version = default_version(cx, def.language, &def.language_version)?;
        let exec = meta_hook(&h.id).map_or(ExecKind::Language, ExecKind::Meta);
        let index = self.next_index();
        self.out.push(ResolvedHook {
            index,
            src: HookSrc::Meta,
            language_version: Arc::clone(&def.language_version),
            def: Arc::new(def),
            prefix: cx.cfg.source.project_root.clone(),
            repo: None,
            exec,
            install_key: None,
            virtual_install: None,
            docker_tag: None,
        });
        Ok(())
    }

    /// `repo: builtin` (ARCHITECTURE §7.3): the embedded builtin manifest, native hooks, no clone.
    fn builtin(&mut self, hooks: &[ConfigHook]) -> Result<(), Failure> {
        let cx = self.cx;
        let manifest = builtin_manifest(cx)?;
        for c in hooks {
            if manifest.get(&c.id).is_none() {
                return Err(merge_failure(
                    cx,
                    &MergeError::UnknownId {
                        id: Arc::clone(&c.id),
                        repo: Arc::from("builtin"),
                    },
                ));
            }
        }
        let mut merged = Vec::new();
        for c in hooks {
            let Some(m) = manifest.get(&c.id) else {
                continue;
            };
            let buf = CollectWarnings::new();
            let patch = self.patch(&c.id, merged_alias(c, m));
            let def = snp_config::merge_hook(m, c, cx.cfg, "builtin", patch, &buf)
                .map_err(|e| merge_failure(cx, &e))?;
            merged.push((def, buf));
        }
        for (def, buf) in merged {
            flush(&buf, cx.warn);
            let exec = builtin_exec(&def);
            let index = self.next_index();
            self.out.push(ResolvedHook {
                index,
                src: HookSrc::Builtin,
                language_version: Arc::clone(&def.language_version),
                def: Arc::new(def),
                prefix: cx.cfg.source.project_root.clone(),
                repo: None,
                exec,
                install_key: None,
                virtual_install: None,
                docker_tag: None,
            });
        }
        Ok(())
    }

    /// A cloned repository (`_cloned_repository_hooks`, `repository.py:171-200`), with the fast path.
    ///
    /// pre-commit's order is: `Initializing` → clone → deprecated-stage warning → manifest → unknown-id check →
    /// per-hook merge errors. A Tier A repository reads its manifest from the profile, so the ids, merges and
    /// fast-path verdicts are known before any clone: when a hook delegates (or `--with-fallback`), the clone comes
    /// first and the line, the warning and the marker follow it as in pre-commit; when every hook runs natively, they
    /// are printed without one (ADR-032). Errors found before that point are held back and reported after it.
    fn remote(
        &mut self,
        plan: &RemotePlan,
        repo: &Arc<str>,
        rev: &Arc<str>,
        hooks: &[ConfigHook],
    ) -> Result<(), Failure> {
        let cx = self.cx;
        self.ensure_store()?;
        let mut base: Option<RepoHandle> = None;
        let manifest = if let Some(profile) = plan.profile {
            let display = would_be_manifest_path(cx, &plan.base_key);
            snp_config::load_manifest_bytes(
                profile.manifest_yaml.as_bytes(),
                display.as_os_str(),
                &load_opts(cx, cx.warn),
            )?
        } else {
            let handle = self.materialize_remote(plan, repo, rev)?;
            let path = handle.src.join(MANIFEST_FILE);
            let m = snp_config::load_manifest(&path, path.as_os_str(), &load_opts(cx, cx.warn))?;
            base = Some(handle);
            m
        };
        let mut held: Option<Held> =
            hooks
                .iter()
                .find(|c| manifest.get(&c.id).is_none())
                .map(|c| {
                    Held::Merge(MergeError::UnknownId {
                        id: Arc::clone(&c.id),
                        repo: Arc::clone(repo),
                    })
                });
        let mut merged = Vec::new();
        if held.is_none() {
            for c in hooks {
                let Some(m) = manifest.get(&c.id) else {
                    continue;
                };
                let buf = CollectWarnings::new();
                let patch = self.patch(&c.id, merged_alias(c, m));
                let mut def = match snp_config::merge_hook(m, c, cx.cfg, repo, patch, &buf) {
                    Ok(d) => d,
                    Err(e) => {
                        held = Some(Held::Merge(e));
                        break;
                    }
                };
                match default_version(cx, def.language, &def.language_version) {
                    Ok(v) => def.language_version = v,
                    Err(f) => {
                        held = Some(Held::Failure(f));
                        break;
                    }
                }
                merged.push((c, m, patch, def, buf));
            }
        }
        let mut facts: Option<Result<CloneFacts, String>> = None;
        let mut decided = Vec::new();
        for (c, m, patch, def, buf) in merged {
            let verdict = self.verdict(plan, repo, base.as_ref(), &mut facts, c, m, patch, &def);
            decided.push((verdict, def, buf));
        }
        if let Some(profile) = plan.profile {
            let with_fallback = self.policy.with_fallback;
            if decided
                .iter()
                .any(|(v, _, _)| clones(v.as_ref(), with_fallback))
            {
                base = Some(self.materialize_remote(plan, repo, rev)?);
            } else {
                self.announce_uncloned(plan, repo, rev, profile.manifest_yaml.as_bytes())?;
            }
        }
        match held {
            Some(Held::Merge(e)) => return Err(merge_failure(cx, &e)),
            Some(Held::Failure(f)) => return Err(f),
            None => {}
        }
        for (verdict, def, buf) in decided {
            self.push_remote(plan, repo, rev, &mut base, verdict, def)?;
            flush(&buf, cx.warn);
        }
        Ok(())
    }

    /// The fast-path verdict of one merged hook; `None` = not a pre-commit-hooks hook at all.
    #[expect(clippy::too_many_arguments, reason = "the gate inputs of one hook")]
    fn verdict(
        &self,
        plan: &RemotePlan,
        repo: &str,
        base: Option<&RepoHandle>,
        facts: &mut Option<Result<CloneFacts, String>>,
        c: &ConfigHook,
        m: &HookDef,
        patch: Option<&HookPatch>,
        def: &HookDef,
    ) -> Option<Verdict> {
        let cand = candidate(self.cx, &self.policy, plan, repo, base, facts, m)?;
        let v = fastpath::decide(
            &GateInput {
                policy: &self.policy,
                user_id: &c.id,
                config: Some(c),
                patch,
                args: &def.args,
                preflight: self.cx.preflight,
            },
            cand,
        );
        let (decision, reason) = v.row();
        tracing::debug!("fast-path {}: {decision} ({reason})", c.id);
        Some(v)
    }

    /// Builds the resolved hook for `verdict`, cloning what pre-commit would have cloned for it.
    fn push_remote(
        &mut self,
        plan: &RemotePlan,
        repo: &Arc<str>,
        rev: &Arc<str>,
        base: &mut Option<RepoHandle>,
        verdict: Option<Verdict>,
        def: HookDef,
    ) -> Result<(), Failure> {
        let cx = self.cx;
        let src = HookSrc::Remote {
            repo: Arc::clone(repo),
            rev: Arc::clone(rev),
        };
        let root = cx.cfg.source.project_root.clone();
        let native = match verdict {
            Some(Verdict::Native { id, epoch, .. }) => Some(ExecKind::Native {
                source: NativeSource::PreCommitHooks,
                id,
                epoch,
            }),
            Some(Verdict::FailLanguage) => {
                let index = self.next_index();
                self.out.push(ResolvedHook {
                    index,
                    src,
                    language_version: Arc::clone(&def.language_version),
                    prefix: base.as_ref().map_or(root, |b| b.src.clone()),
                    repo: base.clone(),
                    def: Arc::new(def),
                    exec: ExecKind::Language,
                    install_key: None,
                    virtual_install: None,
                    docker_tag: None,
                });
                return Ok(());
            }
            _ => None,
        };
        if let Some(exec) = &native
            && !self.policy.with_fallback
        {
            let key = Store::remote_key(&plan.id, rev, &def.additional_dependencies);
            let index = self.next_index();
            self.out.push(ResolvedHook {
                index,
                src,
                language_version: Arc::clone(&def.language_version),
                prefix: base.as_ref().map_or(root, |b| b.src.clone()),
                repo: base.clone(),
                virtual_install: install_key(key, &def),
                def: Arc::new(def),
                exec: exec.clone(),
                install_key: None,
                docker_tag: None,
            });
            return Ok(());
        }
        // Delegated, regular, or `--with-fallback`: the clone pre-commit makes for this hook.
        let base_handle = if let Some(b) = base.clone() {
            b
        } else {
            let b = self.materialize_remote(plan, repo, rev)?;
            *base = Some(b.clone());
            b
        };
        let handle = if def.additional_dependencies.is_empty() {
            base_handle
        } else {
            self.materialize_variant(plan, &base_handle, repo, rev, &def.additional_dependencies)?
        };
        let index = self.next_index();
        self.out.push(ResolvedHook {
            index,
            src,
            language_version: Arc::clone(&def.language_version),
            prefix: handle.src.clone(),
            install_key: install_key(handle.key, &def),
            docker_tag: docker_tag(handle.key, def.language),
            repo: Some(handle),
            def: Arc::new(def),
            exec: native.unwrap_or(ExecKind::Language),
            virtual_install: None,
        });
        Ok(())
    }
}

/// What a new store entry announces (`_new_repo`).
#[derive(Debug, Clone, Copy)]
struct Announce<'n> {
    /// `db_repo_name`.
    name: &'n str,
    /// The revision (`C.LOCAL_REPO_VERSION` for the local template).
    rev: &'n str,
    /// `warn_for_stages_on_repo_init(repo, …)` of the new checkout (`None` for the local template).
    stages_of: Option<&'n str>,
}

/// An error found before a Tier A repository's clone decision, reported after it (pre-commit's order).
#[derive(Debug)]
enum Held {
    /// E1–E3.
    Merge(MergeError),
    /// `get_default_version` and the like.
    Failure(Failure),
}

/// Whether a hook with `verdict` needs the clone pre-commit makes (see `push_remote`).
const fn clones(verdict: Option<&Verdict>, with_fallback: bool) -> bool {
    match verdict {
        Some(Verdict::FailLanguage) => false,
        Some(Verdict::Native { .. }) => with_fallback,
        _ => true,
    }
}

/// The `keys-v1` marker of `(db_repo_name, rev)`.
fn init_key(name: &str, rev: &str) -> MarkerKey {
    MarkerKey::Initialized {
        db_repo_name: Arc::from(name),
        rev: Arc::from(rev),
    }
}

/// Whether the store has seen `(db_repo_name, rev)` (pre-commit's `repos` row).
fn initialized(cx: &ResolveCtx<'_>, name: &str, rev: &str) -> bool {
    cx.store.marker_exists(&init_key(name, rev))
}

/// Removes the marker of `(db_repo_name, rev)`.
fn forget(cx: &ResolveCtx<'_>, name: &str, rev: &str) {
    let _removed = cx.store.retain_markers(&mut |k| {
        !matches!(k, MarkerKey::Initialized { db_repo_name, rev: r } if **db_repo_name == *name && **r == *rev)
    });
}

/// `warn_for_stages_on_repo_init` for a new clone of `repo` with manifest bytes `manifest`.
fn warn_stages(cx: &ResolveCtx<'_>, repo: &str, manifest: &[u8]) {
    let legacy = stages::legacy_stages(manifest);
    if !legacy.is_empty() {
        cx.warn
            .warn(&messages::deprecated_repo_stages(cx.brand, repo, &legacy));
    }
}

/// The deprecated-stage warning from a checkout's manifest file (unreadable: nothing, like pre-commit's swallowed
/// load error).
fn warn_stages_of(cx: &ResolveCtx<'_>, repo: &str, src: &Path) {
    if let Ok(bytes) = snp_os::fs::read_capped(&src.join(MANIFEST_FILE), MANIFEST_CAP) {
        warn_stages(cx, repo, &bytes);
    }
}

/// G2's input for one hook: the Tier A entry, the Tier B verification of the clone, or `None` when the hook is not
/// a pre-commit-hooks hook at all (neither the repository nor the manifest entry is upstream's).
fn candidate(
    cx: &ResolveCtx<'_>,
    policy: &Policy,
    plan: &RemotePlan,
    repo: &str,
    base: Option<&RepoHandle>,
    facts: &mut Option<Result<CloneFacts, String>>,
    m: &HookDef,
) -> Option<Result<fastpath::Candidate, String>> {
    if let Some(profile) = plan.profile {
        return Some(
            fastpath::tier_a_candidate(profile, &m.id)
                .ok_or_else(|| format!("`{}` has no profile entry", m.id)),
        );
    }
    let upstream_url = snp_hooks_data::is_pre_commit_hooks_url(repo);
    let possible = fastpath::tier_b_possible(&m.entry, m.language);
    if !upstream_url && !possible {
        return None;
    }
    if !policy.enabled() {
        return Some(Err("fast path disabled".to_owned()));
    }
    if !possible {
        return Some(Err("not an upstream pre-commit-hooks entry".to_owned()));
    }
    let Some(base) = base else {
        return Some(Err("not cloned".to_owned()));
    };
    let f = facts.get_or_insert_with(|| read_facts(cx, base));
    Some(match f {
        Ok(f) => f.candidate(&m.entry, m.language),
        Err(e) => Err(e.clone()),
    })
}

/// The Tier B facts of a clone (`git ls-tree` of `pre_commit_hooks/`, `setup.cfg`), cached next to it.
fn read_facts(cx: &ResolveCtx<'_>, base: &RepoHandle) -> Result<CloneFacts, String> {
    let owned = OwnedRepo::open(
        Arc::clone(cx.git_exe),
        cx.env,
        &base.src,
        Arc::clone(&cx.git_backend),
    );
    CloneFacts::load(&base.envs, &mut || {
        let blobs = owned.ls_tree_blobs("HEAD", "pre_commit_hooks")?;
        let setup_cfg = owned.read_blob("HEAD", "setup.cfg").ok();
        Ok((blobs, setup_cfg))
    })
    .map_err(|_| "the clone's tree could not be read".to_owned())
}

/// Where the manifest of a never-cloned Tier A repository would be (only shown in a manifest error, which the
/// embedded manifests never produce).
fn would_be_manifest_path(cx: &ResolveCtx<'_>, key: &RepoKey) -> PathBuf {
    cx.store
        .location()
        .path
        .join("repos-v1")
        .join(key.hex())
        .join("src")
        .join(MANIFEST_FILE)
}

/// The builtin manifest (`snp_hooks_data::BUILTIN_MANIFEST_YAML`).
fn builtin_manifest(cx: &ResolveCtx<'_>) -> Result<Manifest, Failure> {
    Ok(snp_config::load_manifest_bytes(
        snp_hooks_data::BUILTIN_MANIFEST_YAML.as_bytes(),
        OsStr::new("builtin"),
        &load_opts(cx, cx.warn),
    )?)
}

/// How a builtin hook runs: its `language: fail` entries through the language, every other id natively with the
/// v6.0.0 epoch (the builtin manifest equals v6.0.0 for the shared ids).
fn builtin_exec(def: &HookDef) -> ExecKind {
    if def.language == LanguageId::Fail {
        return ExecKind::Language;
    }
    let profile = snp_hooks_data::profile_for_rev("v6.0.0");
    let entry = profile.and_then(|p| p.hooks.iter().find(|h| *h.0 == *def.id));
    match entry {
        Some(&(_, HookClass::FailLanguage, _)) => ExecKind::Language,
        Some(&(_, _, epoch)) if epoch != Epoch(0) => ExecKind::Native {
            source: NativeSource::Builtin,
            id: Arc::clone(&def.id),
            epoch,
        },
        _ => ExecKind::Native {
            source: NativeSource::Builtin,
            id: Arc::clone(&def.id),
            epoch: Epoch(1),
        },
    }
}

/// `_make_local_repo` (`store.py:48-56`): the `empty_template_*` resources, written in text mode (CRLF on Windows).
fn write_local_template(dest: &Path) -> Result<(), Failure> {
    for (rel, bytes) in snp_lang::LOCAL_TEMPLATE {
        let path = rel.split('/').fold(dest.to_path_buf(), |p, c| p.join(c));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Failure::from_io(&e, Some(parent.as_os_str()), None))?;
        }
        let text = text_mode(bytes);
        snp_os::fs::write_new_durable(&path, &text)
            .map_err(|e| Failure::from_io(&e, Some(path.as_os_str()), None))?;
    }
    Ok(())
}

/// `open(target, 'w')`: `\n` becomes `\r\n` on Windows.
fn text_mode(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if cfg!(windows) {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                out.push(b'\r');
            }
            out.push(b);
        }
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(bytes)
    }
}

/// The decision-table rows of `cfg` (`snp doctor --fast-path`): no clone, no output; a repository that is not
/// cloned yet and not Tier A is reported as undecided.
pub(crate) fn table(cfg: &Config, cx: &ResolveCtx<'_>) -> Vec<(Arc<str>, Arc<str>, String)> {
    let policy = Policy::new(&cx.fast_path, cfg);
    let sink = CollectWarnings::new();
    let mut rows = Vec::new();
    let row = |id: &Arc<str>, d: &str, r: String| (Arc::clone(id), Arc::from(d), r);
    for repo in &cfg.repos {
        match repo {
            RepoConfig::Builtin { hooks } => {
                for c in hooks {
                    rows.push(row(&c.id, "native", "repo: builtin".to_owned()));
                }
            }
            RepoConfig::Remote { repo, rev, hooks } => {
                let plan = RemotePlan::new(cx, &policy, repo, rev);
                let opts = load_opts(cx, &sink);
                let (manifest, base) = if let Some(p) = plan.profile {
                    let m = snp_config::load_manifest_bytes(
                        p.manifest_yaml.as_bytes(),
                        OsStr::new(MANIFEST_FILE),
                        &opts,
                    );
                    (m, None)
                } else if let Some(h) = cx.store.lookup(&plan.base_key) {
                    let path = h.src.join(MANIFEST_FILE);
                    (
                        snp_config::load_manifest(&path, path.as_os_str(), &opts),
                        Some(h),
                    )
                } else {
                    for c in hooks {
                        let r = if snp_hooks_data::is_pre_commit_hooks_url(repo) || policy.enabled()
                        {
                            "not cloned yet: decided after the clone (tier B)"
                        } else {
                            "fast path disabled"
                        };
                        rows.push(row(&c.id, "undecided", r.to_owned()));
                    }
                    continue;
                };
                let manifest = match manifest {
                    Ok(m) => m,
                    Err(e) => {
                        for c in hooks {
                            rows.push(row(&c.id, "error", e.rendered.trim().to_owned()));
                        }
                        continue;
                    }
                };
                let mut facts = None;
                for c in hooks {
                    let Some(m) = manifest.get(&c.id) else {
                        rows.push(row(
                            &c.id,
                            "error",
                            format!("`{}` is not present in repository {repo}", c.id),
                        ));
                        continue;
                    };
                    let patch = cx
                        .overlay
                        .and_then(|ov| ov.hook_patch(&c.id, merged_alias(c, m)));
                    let def = match snp_config::merge_hook(m, c, cfg, repo, patch, &sink) {
                        Ok(d) => d,
                        Err(e) => {
                            rows.push(row(&c.id, "error", e.message(cx.brand)));
                            continue;
                        }
                    };
                    let Some(cand) =
                        candidate(cx, &policy, &plan, repo, base.as_ref(), &mut facts, m)
                    else {
                        rows.push(row(
                            &c.id,
                            "language",
                            "not a pre-commit-hooks hook".to_owned(),
                        ));
                        continue;
                    };
                    let v = fastpath::decide(
                        &GateInput {
                            policy: &policy,
                            user_id: &c.id,
                            config: Some(c),
                            patch,
                            args: &def.args,
                            preflight: cx.preflight,
                        },
                        cand,
                    );
                    let (d, r) = v.row();
                    rows.push(row(&c.id, d, r));
                }
            }
            _ => {}
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_url_joins_local_directories_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        assert_eq!(
            clone_url("hooks", root),
            root.join("hooks").to_string_lossy()
        );
        assert_eq!(clone_url("missing", root), "missing");
        assert_eq!(
            clone_url("https://example.com/r", root),
            "https://example.com/r"
        );
        let abs = root.join("hooks").to_string_lossy().into_owned();
        assert_eq!(clone_url(&abs, root), abs);
    }

    #[test]
    fn ssh_probe_only_for_ssh_urls() {
        assert!(needs_ssh_probe("git@github.com:o/r"));
        assert!(needs_ssh_probe("ssh://git@github.com/o/r"));
        assert!(!needs_ssh_probe("https://github.com/o/r"));
        assert!(!needs_ssh_probe("/local/path"));
    }

    #[test]
    fn meta_ids() {
        assert_eq!(meta_hook("identity"), Some(MetaHook::Identity));
        assert_eq!(
            meta_hook("check-hooks-apply"),
            Some(MetaHook::CheckHooksApply)
        );
        assert_eq!(
            meta_hook("check-useless-excludes"),
            Some(MetaHook::CheckUselessExcludes)
        );
        assert_eq!(meta_hook("nope"), None);
    }

    #[test]
    fn local_template_is_written_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        write_local_template(tmp.path()).unwrap();
        for (rel, bytes) in snp_lang::LOCAL_TEMPLATE {
            let got = std::fs::read(tmp.path().join(rel)).unwrap();
            assert_eq!(got, &*text_mode(bytes), "{rel}");
        }
    }
}
