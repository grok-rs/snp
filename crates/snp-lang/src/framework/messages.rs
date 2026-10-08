//! Every user-visible text of the framework (ARCHITECTURE §1.3 "Messages"): brand-aware where it names the tool,
//! golden-tested in both brands (`tests/framework`).
//!
//! The snp brand substitutes `pre-commit` → `snp` exactly as `tests/differential/brand_map.yaml` does for these lines
//! (`for now, pre-commit requires `/`for now, pre-commit does not support ` → `for now, snp …`).
//!
//! Owner: WP-16.

use snp_core::brand::Brand;

/// `lang_base.assert_version_default` (`lang_base.py:94-99`):
/// ``for now, pre-commit requires system-installed {binary} -- you selected `language_version: {version}` ``.
pub(crate) fn assert_version_default(brand: Brand, binary: &str, version: &str) -> String {
    format!(
        "for now, {} requires system-installed {binary} -- you selected `language_version: {version}`",
        brand.prog()
    )
}

/// `lang_base.assert_no_additional_deps` (`lang_base.py:102-111`); `deps` is the Python list repr (`['x']`):
/// ``for now, pre-commit does not support additional_dependencies for {lang} -- you selected
/// `additional_dependencies: {deps!r}` ``.
pub(crate) fn assert_no_additional_deps(brand: Brand, lang: &str, deps_repr: &str) -> String {
    format!(
        "for now, {} does not support additional_dependencies for {lang} -- you selected \
         `additional_dependencies: {deps_repr}`",
        brand.prog()
    )
}

/// `lang_base.no_install` (`lang_base.py:122-127`): `AssertionError('This language is not installable')`.
pub(crate) const NOT_INSTALLABLE: &str = "This language is not installable";

/// The C-hook text of an `entry` that splits to nothing (X13-EXE-01; `normalize_cmd(())` in pre-commit is an
/// `IndexError`, exit 3): the hook fails like any `ExecutableNotFoundError` (`parse_shebang.py:48-62`).
pub(crate) const EMPTY_ENTRY: &str = "Executable `` not found";

/// The C-hook text of an `entry` that `shlex.split` rejects (X13-EXE-02): the Python exception's last line,
/// `ValueError: No closing quotation` / `ValueError: No escaped character`.
pub(crate) fn shlex_error(py_type: &str, text: &str) -> String {
    format!("{py_type}: {text}")
}
