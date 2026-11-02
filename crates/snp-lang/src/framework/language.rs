//! The `Language` trait, the registry and the support checks (04b §4, §5.1).
//!
//! Owner: WP-16.

use std::sync::Arc;

use snp_core::brand::Brand;
use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::messages;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, PlanCtx, RunPlan,
};

/// How a language treats `language_version`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VersionSupport {
    /// Only `default` (`assert_version_default`).
    DefaultOnly,
    /// Any version.
    Any,
    /// The value is ignored.
    Ignored,
}

/// What a language accepts.
#[derive(Copy, Clone, Debug)]
pub struct Support {
    /// `language_version` handling.
    pub version: VersionSupport,
    /// `additional_dependencies` allowed (else `assert_no_additional_deps`).
    pub deps: bool,
}

/// One pre-commit language (`pre_commit/languages/<lang>.py`).
///
/// Implemented by the zero-sized backends of [`crate::langs`]; [`backend`] is the registry. The lifecycle is driven by
/// snp-repo (ARCHITECTURE §6.1): version resolution → env-less guards → [`check_support`] (before any filesystem
/// mutation) → installed? → [`Language::install`] → [`Language::health`] → [`Language::activate`] →
/// [`Language::plan`] → executor (snp-run). Backends never run hooks; setup and probe commands go through
/// [`Host::runner`](crate::Host).
pub trait Language: Send + Sync + 'static {
    /// The language.
    fn id(&self) -> LanguageId;
    /// Version and dependency support.
    fn support(&self) -> Support;
    /// Memoised by `ToolProbe`.
    fn default_version(&self, h: &Host<'_>) -> Result<Arc<str>, LangError>;
    /// `env_dir` does NOT exist on entry.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError>;
    /// `health_check`.
    fn health(&self, cx: &EnvCtx<'_>, recorded: Option<&InstallState>)
    -> Result<Health, LangError>;
    /// `in_env` as data.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError>;
    /// What to run.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError>;
}

/// Static match over the ZSTs in `langs` (`all_languages.py:27-50`).
pub fn backend(id: LanguageId) -> &'static dyn Language {
    crate::langs::registry(id)
}

/// What `install_environment` of `id` asserts in pre-commit 4.7.0 (04b §3, §4.5):
///
/// * `assert_version_default`: conda, coursier, dart, docker, dotnet, haskell, lua, perl, r, swift;
/// * `assert_no_additional_deps`: docker, dotnet, swift;
/// * julia accepts and ignores `language_version`; python, node, golang, rust, ruby accept any version and deps;
/// * the env-less languages (`docker_image`, `fail`, `pygrep`, `unsupported`, `unsupported_script`) are never installed: a
///   non-default version or any dependency is the config-time `[ERROR] The hook … does not install an environment`
///   guard of snp-repo (`repository.py:126-142`), reported as `DefaultOnly` without deps.
///
/// The environment backends' [`Language::support`] return exactly this (crate-internal, so WP-24/WP-25 share it).
pub(crate) const fn default_support(id: LanguageId) -> Support {
    use LanguageId as L;
    let (version, deps) = match id {
        L::Conda | L::Coursier | L::Dart | L::Haskell | L::Lua | L::Perl | L::R => {
            (VersionSupport::DefaultOnly, true)
        }
        L::Docker | L::Dotnet | L::Swift => (VersionSupport::DefaultOnly, false),
        L::Julia => (VersionSupport::Ignored, true),
        L::Python | L::Node | L::Golang | L::Rust | L::Ruby => (VersionSupport::Any, true),
        L::DockerImage | L::Fail | L::Pygrep | L::Unsupported | L::UnsupportedScript => {
            (VersionSupport::DefaultOnly, false)
        }
    };
    Support { version, deps }
}

/// Verbatim `assert_version_default` / `assert_no_additional_deps` texts (brand-aware); run BEFORE any FS mutation
/// (04b §5.1).
///
/// pre-commit runs these assertions inside `install_environment`, after `_hook_install` has already `rmtree`d the
/// environment directory (`repository.py:79-82`), which is how a hostile `language_version` deletes directories
/// (04b §5.1); snp checks first and touches nothing. The order is pre-commit's (version, then deps); the binary named
/// in the version text is the language name for every language (`conda.py:64`, …, `swift.py:39`). Env-less languages
/// pass: their guard (with the hook id) belongs to hook resolution. Errors are `AssertionError` (exit 3).
pub fn check_support(
    id: LanguageId,
    version: &str,
    deps: &[Arc<str>],
    brand: Brand,
) -> Result<(), LangError> {
    if id.env_dir_base().is_none() {
        return Ok(());
    }
    let s = default_support(id);
    if s.version == VersionSupport::DefaultOnly && version != "default" {
        return Err(LangError::Assertion(messages::assert_version_default(
            brand,
            id.as_str(),
            version,
        )));
    }
    if !s.deps && !deps.is_empty() {
        let repr = snp_pycompat::repr::repr_str_list(deps);
        return Err(LangError::Assertion(messages::assert_no_additional_deps(
            brand,
            id.as_str(),
            &repr,
        )));
    }
    Ok(())
}

/// 4.7.0 `empty_template_*` bytes (03 §7.7), in `store._LOCAL_RESOURCES` order: `(path relative to the local repo,
/// bytes)`, `/`-separated (`renv/activate.R`).
///
/// pre-commit writes them with `open(target, 'w')` from `resource_text(...)` (`store.py:48-56`), i.e. in text mode:
/// LF on POSIX, CRLF on Windows (`snp_pycompat::textio::windows_text_write`); the bytes here are the LF resources.
pub const LOCAL_TEMPLATE: &[(&str, &[u8])] = &[
    (
        "Cargo.toml",
        include_bytes!("template/empty_template_Cargo.toml"),
    ),
    ("main.go", include_bytes!("template/empty_template_main.go")),
    ("go.mod", include_bytes!("template/empty_template_go.mod")),
    ("main.rs", include_bytes!("template/empty_template_main.rs")),
    (
        "pre-commit-package-dev-1.rockspec",
        include_bytes!("template/empty_template_pre-commit-package-dev-1.rockspec"),
    ),
    (
        "pre_commit_placeholder_package.gemspec",
        include_bytes!("template/empty_template_pre_commit_placeholder_package.gemspec"),
    ),
    (
        "setup.py",
        include_bytes!("template/empty_template_setup.py"),
    ),
    (
        "environment.yml",
        include_bytes!("template/empty_template_environment.yml"),
    ),
    (
        "Makefile.PL",
        include_bytes!("template/empty_template_Makefile.PL"),
    ),
    (
        "pubspec.yaml",
        include_bytes!("template/empty_template_pubspec.yaml"),
    ),
    (
        "renv.lock",
        include_bytes!("template/empty_template_renv.lock"),
    ),
    (
        "renv/activate.R",
        include_bytes!("template/empty_template_activate.R"),
    ),
    (
        "renv/LICENSE.renv",
        include_bytes!("template/empty_template_LICENSE.renv"),
    ),
];
