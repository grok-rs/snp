//! The typed config model (`clientlib.py` schemas, `hook.py` fields; 01 §2–§6) and its errors.
//!
//! Owner: WP-21.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-21")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-21")]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use file_identify::TagSet;
use snp_core::brand::Brand;
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_core::output::{DiagSink, WarnSink};
use snp_core::stage::{HookType, StageSet};

use crate::ext::{HookExt, RootExt};

/// Where a config was found: the file, its display name (`-c` as given) and the project root its relative
/// `repo:` paths resolve against (= the repository root in v1, §1.6).
#[derive(Clone, Debug)]
pub struct ConfigSource {
    /// The config file.
    pub path: PathBuf,
    /// The name printed in messages (`-c` verbatim).
    pub display: OsString,
    /// The project root (v1: the git work-tree root).
    pub project_root: PathBuf,
}

/// Whether snp's extension keys are accepted.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtMode {
    /// Extension keys are validated and applied (§8.1).
    Enabled,
    /// `SNP_COMPAT_STRICT=1`: pre-commit's schemas only.
    Strict,
}

/// Everything a load needs besides the file.
#[derive(Debug)]
pub struct LoadOpts<'a> {
    /// The brand (message texts).
    pub brand: Brand,
    /// Extension-key policy.
    pub ext: ExtMode,
    /// The run's regex cache (`files`/`exclude` validation).
    pub regexes: &'a snp_pyre::RegexCache,
    /// `[WARNING] …` on stdout, in cfgv item order.
    pub warn: &'a dyn WarnSink,
    /// Regex `FutureWarning`s and snp notes on stderr.
    pub diag: &'a dyn DiagSink,
}

/// `InvalidConfigError` / `InvalidManifestError` (`clientlib.py`, cfgv's `ValidationError` re-raised as `exc_tp`).
#[derive(Debug, Clone)]
pub struct ConfigError {
    /// `InvalidManifestError` (true) or `InvalidConfigError` (false).
    pub manifest: bool,
    /// The exception text: `"\n==> File …\n=====> leaf"`.
    pub rendered: String,
}

impl From<ConfigError> for Failure {
    /// `Exc { Fatal, InvalidConfigError | InvalidManifestError }` (exit 1, 13 X.1).
    fn from(e: ConfigError) -> Self {
        todo!()
    }
}

/// The validated, defaulted `.pre-commit-config.yaml` (`CONFIG_SCHEMA`, 01 §3).
#[derive(Debug, Clone)]
pub struct Config {
    /// Where it was loaded from.
    pub source: ConfigSource,
    /// `repos`, in config order.
    pub repos: Vec<RepoConfig>,
    /// `default_install_hook_types`.
    pub default_install_hook_types: Vec<HookType>,
    /// `default_language_version`.
    pub default_language_version: indexmap::IndexMap<LanguageId, Arc<str>>,
    /// `default_stages` (legacy names transformed).
    pub default_stages: StageSet,
    /// Top-level `files`.
    pub files: Arc<str>,
    /// Top-level `exclude`.
    pub exclude: Arc<str>,
    /// `fail_fast`.
    pub fail_fast: bool,
    /// `minimum_pre_commit_version`.
    pub minimum_pre_commit_version: Arc<str>,
    /// Root extension keys (§8).
    pub root_ext: RootExt,
    /// The validated + defaulted value (meta hooks, `config dump`).
    pub raw: cfgv::Value,
}

/// One entry of `repos` (`CONFIG_REPO_DICT`, 01 §3.2).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RepoConfig {
    /// A remote repository.
    Remote {
        /// `repo` verbatim.
        repo: Arc<str>,
        /// `rev`.
        rev: Arc<str>,
        /// The config-level hook dicts.
        hooks: Vec<ConfigHook>,
    },
    /// `repo: local`: fully defaulted manifest-style hooks.
    Local {
        /// The hooks.
        hooks: Vec<HookDef>,
    },
    /// `repo: meta`.
    Meta {
        /// The hooks (meta manifest ∪ config).
        hooks: Vec<HookDef>,
    },
    /// `repo: builtin` (snp extension, §7.3).
    Builtin {
        /// The config-level hook dicts.
        hooks: Vec<ConfigHook>,
    },
}

/// A config hook of a remote/builtin repo: the config-level dict (stage/language transforms applied, NO defaults),
/// kept raw because check-useless-excludes reads it unmerged (01 §9.2) and merging is whole-key override.
#[derive(Debug, Clone)]
pub struct ConfigHook {
    /// `id`.
    pub id: Arc<str>,
    /// The dict as written (transforms applied).
    pub raw: cfgv::Map,
    /// Parsed extension keys.
    pub ext: HookExt,
}

/// Fully defaulted manifest-style hook (`hook.py`, 22 fields). Strings `Arc<str>`; lists `Arc<[…]>`.
#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per hook.py key (01 §6)"
)]
pub struct HookDef {
    /// `id`.
    pub id: Arc<str>,
    /// `name`.
    pub name: Arc<str>,
    /// `entry`.
    pub entry: Arc<str>,
    /// `language` (legacy names transformed).
    pub language: LanguageId,
    /// `alias`.
    pub alias: Arc<str>,
    /// `files`.
    pub files: Arc<str>,
    /// `exclude`.
    pub exclude: Arc<str>,
    /// `types`.
    pub types: TagSet,
    /// `types_or`.
    pub types_or: TagSet,
    /// `exclude_types`.
    pub exclude_types: TagSet,
    /// `additional_dependencies`.
    pub additional_dependencies: Arc<[Arc<str>]>,
    /// `args`.
    pub args: Arc<[Arc<str>]>,
    /// `always_run`.
    pub always_run: bool,
    /// `fail_fast`.
    pub fail_fast: bool,
    /// `pass_filenames`.
    pub pass_filenames: bool,
    /// `description`.
    pub description: Arc<str>,
    /// `language_version`.
    pub language_version: Arc<str>,
    /// `log_file`.
    pub log_file: Arc<str>,
    /// `minimum_pre_commit_version`.
    pub minimum_pre_commit_version: Arc<str>,
    /// `require_serial`.
    pub require_serial: bool,
    /// `stages` (legacy names transformed; empty ⇒ `default_stages` after merging).
    pub stages: StageSet,
    /// Whether `stages` was given explicitly.
    pub stages_set: bool,
    /// `verbose`.
    pub verbose: bool,
    /// Unknown keys (W11 warning, sorted).
    pub extra_keys: Vec<Arc<str>>,
    /// Parsed extension keys.
    pub ext: HookExt,
}

/// A loaded `.pre-commit-hooks.yaml` (`MANIFEST_SCHEMA`).
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The hooks in file order.
    pub hooks: Vec<HookDef>,
}

impl Manifest {
    /// By-id lookup; the last duplicate wins (`{hook['id']: hook for hook in manifest}`).
    pub fn get(&self, id: &str) -> Option<&HookDef> {
        todo!()
    }
}

/// `[ERROR] …` + exit 1 from `repository.py:_hook` (01 §6.4).
#[derive(Debug)]
#[non_exhaustive]
pub enum MergeError {
    /// `{id} is not present in repository {repo}.  Typo? …`.
    UnknownId {
        /// The hook id.
        id: Arc<str>,
        /// The repository.
        repo: Arc<str>,
    },
    /// `language_version` set on an env-less language.
    LanguageVersionOnEnvless {
        /// The hook id.
        id: Arc<str>,
        /// The language.
        lang: LanguageId,
    },
    /// `additional_dependencies` set on an env-less language.
    DepsOnEnvless {
        /// The hook id.
        id: Arc<str>,
        /// The language.
        lang: LanguageId,
    },
}
