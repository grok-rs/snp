//! The `dotnet` language backend (§6.2; `pre_commit/languages/dotnet.py`).
//!
//! `install_environment` (`dotnet.py:55-111`): `dotnet pack --configuration Release --property
//! PackageOutputPath=<prefix>/pre-commit-build` in the repository, then for every `*.nupkg` of that directory the tool
//! id is read from the package's single `*.nuspec` (the first `id` element in the root element's namespace) and
//! installed with `dotnet tool install --configfile <tmp>/nuget.config --tool-path <env>/bin --add-source
//! <build dir> <id>`, the temporary `nuget.config` clearing every package source. `additional_dependencies` are not
//! supported. Hooks run with `<env>/bin` first on `PATH`.
//!
//! snp differences: the packages are processed in sorted order (pre-commit: directory order); the nuspec entry is read
//! with a 1 MiB cap and parsed with `roxmltree` (no entity expansion), 04b §5.7: a nuspec with a DTD (`<!DOCTYPE …>`,
//! internal entities) is refused, where `ElementTree` would expand them. COMPATIBILITY §5 has no register ID for the DTD refusal yet (allocation by the integration lead is pending).
//!
//! Owner: WP-25.

use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use snp_core::env::EnvPatch;
use snp_core::lang::LanguageId;

use super::docker::group2::{
    assert_support, basic_plan, default_version, io_err, listdir, outcome, tempdir, write_text,
};
use crate::common::{prefix_path, setup_cmd};
use crate::framework::default_support;
use crate::{
    EnvCtx, Health, Host, InstallCtx, InstallOutcome, InstallState, LangError, Language, PlanCtx,
    RunPlan, Support,
};

/// `language: dotnet`.
#[derive(Debug)]
pub struct Dotnet;

/// `dotnet.BIN_DIR`.
const BIN_DIR: &str = "bin";

/// The size cap on a `.nuspec` entry (zip-bomb protection, 04b §5.7).
pub const MAX_NUSPEC: u64 = 1 << 20;

/// `_nuget_config_no_sources()`'s file content (`dotnet.py:37-52`), one line, no newline.
pub const NUGET_CONFIG: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><configuration>  <packageSources>    \
                                <clear />  </packageSources></configuration>";

/// `AssertionError` texts of `install_environment`.
pub const NO_BUILD_OUTPUTS: &str = "could not find any build outputs to install";
/// See [`NO_BUILD_OUTPUTS`].
pub const NO_NAMESPACE: &str = "could not parse namespace from nuspec";
/// See [`NO_BUILD_OUTPUTS`].
pub const NO_ID_ELEMENT: &str = "expected to find an \"id\" element";
/// See [`NO_BUILD_OUTPUTS`].
pub const EMPTY_ID: &str = "\"id\" element missing tool name";

/// The tool id of a `.nuspec` document (`dotnet.py:84-98`): the root element must be in a namespace
/// (`re.match(r'{.*}', root.tag)`), the first descendant `id` element in that namespace (document order) must have
/// non-empty text.
pub fn nuspec_tool_id(xml: &str) -> Result<String, LangError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| LangError::Value(e.to_string()))?;
    let root = doc.root_element();
    let Some(ns) = root.tag_name().namespace().filter(|ns| !ns.is_empty()) else {
        return Err(LangError::Assertion(NO_NAMESPACE.to_owned()));
    };
    let id = root
        .descendants()
        .skip(1)
        .find(|n| {
            n.is_element() && n.tag_name().name() == "id" && n.tag_name().namespace() == Some(ns)
        })
        .ok_or_else(|| LangError::Assertion(NO_ID_ELEMENT.to_owned()))?;
    // ElementTree's `.text`: the character data before the first child element.
    let mut text = String::new();
    for child in id.children() {
        if child.is_element() {
            break;
        }
        if let Some(t) = child.text().filter(|_| child.is_text()) {
            text.push_str(t);
        }
    }
    if text.is_empty() {
        return Err(LangError::Assertion(EMPTY_ID.to_owned()));
    }
    Ok(text)
}

/// The `.nuspec` text of the package at `path`: exactly one member must end with `.nuspec` (`nuspec, = (…)`, else
/// pre-commit's `ValueError` unpack texts), read with the [`MAX_NUSPEC`] cap.
pub fn read_nuspec(path: &Path) -> Result<String, LangError> {
    let file = std::fs::File::open(path).map_err(io_err("open", path))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| LangError::Value(format!("{}: {e}", path.display())))?;
    let mut names: Vec<String> = Vec::new();
    for n in archive.file_names() {
        let n = n.map_err(|e| LangError::Value(format!("{}: {e}", path.display())))?;
        if n.ends_with(".nuspec") {
            names.push(n.into_owned());
        }
    }
    let name = match names.as_slice() {
        [one] => one.clone(),
        [] => {
            return Err(LangError::Value(
                "not enough values to unpack (expected 1, got 0)".to_owned(),
            ));
        }
        _ => {
            return Err(LangError::Value(
                "too many values to unpack (expected 1)".to_owned(),
            ));
        }
    };
    let entry = archive
        .by_name(&name)
        .map_err(|e| LangError::Value(format!("{}: {e}", path.display())))?;
    let mut bytes = Vec::new();
    entry
        .take(MAX_NUSPEC.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_err("read", path))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_NUSPEC {
        return Err(LangError::Value(format!(
            "{}: {name} is larger than {MAX_NUSPEC} bytes",
            path.display()
        )));
    }
    let bytes = bytes.trim_prefix(b"\xef\xbb\xbf");
    String::from_utf8(bytes.to_vec())
        .map_err(|_| LangError::Value(format!("{}: {name} is not UTF-8", path.display())))
}

impl Language for Dotnet {
    fn id(&self) -> LanguageId {
        LanguageId::Dotnet
    }

    /// `assert_version_default('dotnet', …)` and `assert_no_additional_deps('dotnet', …)`.
    fn support(&self) -> Support {
        default_support(LanguageId::Dotnet)
    }

    /// `lang_base.basic_get_default_version`.
    fn default_version(&self, _h: &Host<'_>) -> Result<Arc<str>, LangError> {
        Ok(default_version())
    }

    /// `install_environment` (`dotnet.py:55-111`), see the module docs.
    fn install(&self, cx: &InstallCtx<'_>) -> Result<InstallOutcome, LangError> {
        assert_support(LanguageId::Dotnet, cx)?;
        let host = cx.cx.host;
        let prefix = cx.cx.prefix;
        let build_dir = PathBuf::from(prefix_path(prefix, OsStr::new("pre-commit-build")));
        let mut prop = OsString::from("PackageOutputPath=");
        prop.push(build_dir.as_os_str());
        let pack: Vec<OsString> = vec![
            "dotnet".into(),
            "pack".into(),
            "--configuration".into(),
            "Release".into(),
            "--property".into(),
            prop,
        ];
        setup_cmd(host.runner, &pack, host.base_env, prefix)?;
        let mut nupkgs = listdir(&build_dir)?;
        nupkgs.retain(|n| n.as_encoded_bytes().ends_with(b".nupkg"));
        if nupkgs.is_empty() {
            return Err(LangError::Assertion(NO_BUILD_OUTPUTS.to_owned()));
        }
        let tool_path = cx.cx.env_dir.join(BIN_DIR);
        for nupkg in nupkgs {
            let xml = read_nuspec(&build_dir.join(&nupkg))?;
            let tool_id = nuspec_tool_id(&xml)?;
            let tmp = tempdir(host.base_env)?;
            let config = tmp.path().join("nuget.config");
            write_text(&config, NUGET_CONFIG)?;
            let install: Vec<OsString> = vec![
                "dotnet".into(),
                "tool".into(),
                "install".into(),
                "--configfile".into(),
                config.into_os_string(),
                "--tool-path".into(),
                tool_path.clone().into_os_string(),
                "--add-source".into(),
                build_dir.clone().into_os_string(),
                tool_id.into(),
            ];
            setup_cmd(host.runner, &install, host.base_env, prefix)?;
        }
        Ok(outcome("dotnet"))
    }

    /// `lang_base.basic_health_check`.
    fn health(
        &self,
        _cx: &EnvCtx<'_>,
        _recorded: Option<&InstallState>,
    ) -> Result<Health, LangError> {
        Ok(Health::Healthy)
    }

    /// `get_env_patch(env)`: `<env>/bin` first on `PATH`.
    fn activate(&self, cx: &EnvCtx<'_>) -> Result<EnvPatch, LangError> {
        Ok(EnvPatch::default().prepend_path("PATH", [cx.env_dir.join(BIN_DIR)]))
    }

    /// `lang_base.basic_run_hook`.
    fn plan(&self, cx: &PlanCtx<'_>) -> Result<RunPlan, LangError> {
        Ok(basic_plan(cx))
    }
}
