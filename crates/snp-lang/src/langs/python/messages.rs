//! Every user-visible text of the python backend (ARCHITECTURE §1.3 "Messages"). The health texts are pre-commit
//! 4.7.0's (`python.py:175-211`); they reach the user only inside the `BUG: expected environment … healthy` assertion
//! of an environment that is unhealthy right after its install. None names the tool, so both brands share them.
//!
//! Owner: WP-24.

/// `health_check`: no `pyvenv.cfg`.
pub(crate) const PYVENV_MISSING: &str = "pyvenv.cfg does not exist (old virtualenv?)";

/// `health_check`: no `version_info` key.
pub(crate) const VERSION_INFO_MISSING: &str =
    "created virtualenv's pyvenv.cfg is missing `version_info`";

/// `_version_info(exe)` when the probe fails.
pub(crate) fn version_error(exe: &str) -> String {
    format!("<<error retrieving version from {exe}>>")
}

/// `health_check`: the environment's interpreter reports another version.
pub(crate) fn venv_mismatch(actual: &str, expected: &str) -> String {
    format!(
        "virtualenv python version did not match created version:\n\
         - actual version: {actual}\n\
         - expected version: {expected}\n"
    )
}

/// `health_check`: the base executable reports another version.
pub(crate) fn base_mismatch(actual: &str, expected: &str) -> String {
    format!(
        "base executable python version does not match created version:\n\
         - base-executable version: {actual}\n\
         - expected version: {expected}\n"
    )
}

/// snp only (silent reinstall reason): the environment was built by another installer than the one requested.
pub(crate) fn installer_changed(recorded: &str, requested: &str) -> String {
    format!(
        "python environment was built with {recorded}, SNP_PYTHON_INSTALLER={requested} requires a rebuild"
    )
}

/// snp only: an unknown `SNP_PYTHON_INSTALLER` value (`FatalError`).
pub(crate) fn bad_installer(value: &str) -> String {
    format!(
        "SNP_PYTHON_INSTALLER must be one of `auto`, `uv`, `pip` (got {})",
        snp_pycompat::repr::repr_str(value)
    )
}

/// snp only: `SNP_PYTHON_INSTALLER=uv` and the environment cannot be built with uv (`FatalError`).
pub(crate) fn uv_required(reason: &str) -> String {
    format!(
        "SNP_PYTHON_INSTALLER=uv: {reason}; set SNP_PYTHON_INSTALLER=auto or pip to build it with pip"
    )
}

/// Fallback reason: no uv.
pub(crate) const NO_UV: &str = "uv is not available (no $SNP_UV, no uv next to snp or on PATH, and the pinned release cannot be downloaded)";

/// Fallback reason: a pip setting without a uv equivalent.
pub(crate) fn untranslatable(origin: &str) -> String {
    format!("the pip setting `{origin}` has no uv equivalent")
}

/// Fallback reason: a pip option among `additional_dependencies` that uv does not accept.
pub(crate) fn dash_dependency(dep: &str) -> String {
    format!("the additional dependency `{dep}` is a pip option uv does not accept")
}

/// No interpreter for `language_version` (`ValueError`, exit 3, like the `CalledProcessError` of virtualenv's
/// `RuntimeError: failed to find interpreter for Builtin discover of python_spec=…` in pre-commit).
pub(crate) fn no_interpreter(version: &str) -> String {
    format!(
        "failed to find interpreter for python_spec={}",
        snp_pycompat::repr::repr_str(version)
    )
}
