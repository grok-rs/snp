//! pre-commit's custom cfgv validators as `cfgv::MapItem`s (`clientlib.py:42-404`; 01 §5) and the check
//! functions they share.
//!
//! Every item reproduces its Python counterpart's `check` / `apply_default` exactly: the same contexts (`At key:`
//! added manually where 4.7.0 adds it), the same messages, the same warning order through `Cx`. Where pre-commit
//! raises a non-`ValidationError` (unhashable stage/language/tag values, `int()` failures in `parse_version`), the
//! Rust port produces the validation error that pre-commit's intent implies (D-CRASH-01, D-CRASH-03).
//!
//! Owner: WP-21.

use cfgv::{
    CheckFn, ConditionValue, Cx, Map, MapItem, ValidationError, Validator, Value, validate_context,
};
use file_identify::Tag;
use snp_core::brand::Brand;
use snp_core::profile;
use snp_core::stage::Stage;

use crate::messages;

/// `C.DEFAULT`.
pub(crate) const DEFAULT: &str = "default";
/// `LOCAL`.
pub(crate) const LOCAL: &str = "local";
/// `META`.
pub(crate) const META: &str = "meta";
/// `repo: builtin` (snp extension, ARCHITECTURE §7.3).
pub(crate) const BUILTIN: &str = "builtin";

/// `_STAGES` keys (`clientlib.py:64-68`).
pub(crate) const LEGACY_STAGES: [&str; 3] = ["commit", "merge-commit", "push"];

/// `parse_version(C.VERSION)`.
const PRE_COMMIT_VERSION: [i128; 3] = [4, 7, 0];

/// `transform_stage` over a value: strings are looked up, every other value passes through unchanged (where
/// Python's dict lookup would raise `TypeError` for an unhashable value, the value is kept and fails the one-of
/// check that follows; D-CRASH-03).
pub(crate) fn transform_stage(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::from(Stage::transform_legacy(s)),
        other => other.clone(),
    }
}

/// `_translate_language` over a value (same unhashable rule as [`transform_stage`]).
pub(crate) fn translate_language(v: &Value) -> Value {
    match v.as_str() {
        Some("system") => Value::from("unsupported"),
        Some("script") => Value::from("unsupported_script"),
        _ => v.clone(),
    }
}

/// `STAGES` as cfgv values (`check_one_of` sorts them for the message).
pub(crate) fn stage_values() -> Vec<Value> {
    Stage::ALL.iter().map(|s| Value::from(s.as_str())).collect()
}

/// `language_names` as cfgv values.
pub(crate) fn language_values() -> Vec<Value> {
    snp_core::lang::LanguageId::ALL
        .iter()
        .map(|l| Value::from(l.as_str()))
        .collect()
}

/// `check_string_regex = check_and(check_string, check_regex)` with snp-pyre's CPython-3.12 acceptance
/// (ADR-003; every rejection, OverflowError/ValueError/RecursionError included, is the plain cfgv leaf,
/// D-CRASH-04).
pub(crate) fn check_string_regex() -> CheckFn {
    CheckFn::check_and([
        CheckFn::check_string(),
        CheckFn::check_regex_with(snp_pyre::cfgv_is_valid),
    ])
}

/// The `Cx` warning prefix of [`Compiled`] (no real warning starts with NUL); `load::validate_bytes` routes these
/// to its pattern list instead of the warning sink.
pub(crate) const COMPILED: &str = "\0snp-compiled\0";

/// A `files` / `exclude` item whose check compiles the pattern (`check_regex` → `re.compile`, `cfgv.py:386`):
/// after a successful check it reports the pattern through `Cx` (prefix [`COMPILED`]), so the loader prints the
/// pattern's `FutureWarning`s in the order pre-commit compiles them, also when a later item fails (D-MISC-04).
#[derive(Debug)]
pub(crate) struct Compiled(pub(crate) Validator);

impl MapItem for Compiled {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.0.check(dct, cx)?;
        if let Some(p) = self
            .0
            .key()
            .and_then(|k| dct.get(k))
            .and_then(Value::as_str)
        {
            cx.warn(format!("{COMPILED}{p}"));
        }
        Ok(())
    }

    fn apply_default(&self, dct: &mut Map) {
        MapItem::apply_default(&self.0, dct);
    }

    fn remove_default(&self, dct: &mut Map) {
        MapItem::remove_default(&self.0, dct);
    }

    fn key(&self) -> Option<&Value> {
        MapItem::key(&self.0)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        MapItem::check_fn(&self.0)
    }
}

/// `check_type_tag` (`clientlib.py:42-47`): `tag not in ALL_TAGS` (file-identify's tags = identify 2.6.20).
pub(crate) fn check_type_tag(brand: Brand) -> CheckFn {
    CheckFn::custom(move |v| {
        if v.as_str().and_then(Tag::from_name).is_some() {
            Ok(())
        } else {
            Err(ValidationError::new(messages::type_tag(
                brand,
                &v.try_repr()?,
            )))
        }
    })
}

/// `check_min_version` (`clientlib.py:55-61`) after `check_string`: a component that `int()` rejects is the
/// validation error `invalid literal for int() with base 10: '…'` (D-CRASH-01; pre-commit crashes with exit 3).
pub(crate) fn min_version_error(brand: Brand, version: &str) -> Option<ValidationError> {
    match snp_pycompat::int::parse_version(version) {
        Err(e) => Some(ValidationError::new(e.to_string())),
        Ok(parts) if parts.as_slice() > PRE_COMMIT_VERSION.as_slice() => {
            Some(ValidationError::new(messages::min_version(brand, version)))
        }
        Ok(_) => None,
    }
}

/// `check_and(check_string, check_min_version)`.
pub(crate) fn check_min_version(brand: Brand) -> CheckFn {
    CheckFn::check_and([
        CheckFn::check_string(),
        CheckFn::custom(
            move |v| match v.as_str().and_then(|s| min_version_error(brand, s)) {
                Some(e) => Err(e),
                None => Ok(()),
            },
        ),
    ])
}

/// `StagesMigrationNoDefault` / `StagesMigration` (`clientlib.py:112-139`).
#[derive(Debug)]
pub(crate) struct StagesMigration {
    key: Value,
    /// `None`: the `NoDefault` variant.
    default: Option<Value>,
}

impl StagesMigration {
    pub(crate) fn new(key: &str, default: Option<Value>) -> Self {
        Self {
            key: Value::from(key),
            default,
        }
    }
}

impl MapItem for StagesMigration {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let Some(val) = dct.get(&self.key) else {
            return Ok(());
        };
        validate_context(
            || format!("At key: {}", self.key),
            || {
                CheckFn::check_array(CheckFn::check_any()).check(val)?;
                let transformed: Vec<Value> = val
                    .as_array()
                    .unwrap_or_default()
                    .iter()
                    .map(transform_stage)
                    .collect();
                CheckFn::check_array(CheckFn::check_one_of(stage_values()))
                    .check(&Value::List(transformed))
            },
        )
    }

    fn apply_default(&self, dct: &mut Map) {
        if let Some(default) = &self.default
            && !dct.contains_key(&self.key)
        {
            dct.insert(self.key.clone(), default.clone());
        }
        if let Some(slot) = dct.get_mut(&self.key)
            && let Some(items) = slot.as_array()
        {
            *slot = Value::List(items.iter().map(transform_stage).collect());
        }
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }
}

/// `DeprecatedStagesWarning('stages')` / `DeprecatedDefaultStagesWarning('default_stages')`
/// (`clientlib.py:142-191`): legacy names in order, duplicates kept.
#[derive(Debug)]
pub(crate) struct DeprecatedStagesWarning {
    key: Value,
    /// `true`: the hook variant (W8), `false`: the top-level variant (W9).
    hook: bool,
    brand: Brand,
}

impl DeprecatedStagesWarning {
    pub(crate) fn hook(brand: Brand) -> Self {
        Self {
            key: Value::from("stages"),
            hook: true,
            brand,
        }
    }

    pub(crate) fn top(brand: Brand) -> Self {
        Self {
            key: Value::from("default_stages"),
            hook: false,
            brand,
        }
    }
}

impl MapItem for DeprecatedStagesWarning {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let Some(val) = dct.get(&self.key) else {
            return Ok(());
        };
        CheckFn::check_array(CheckFn::check_any()).check(val)?;
        let legacy: Vec<&str> = val
            .as_array()
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .filter(|s| LEGACY_STAGES.contains(s))
            .collect();
        if !legacy.is_empty() {
            let list = messages::join(&legacy);
            if self.hook {
                let id = dct.get(&Value::from("id")).map_or_default(Value::to_string);
                cx.warn(messages::hook_deprecated_stages(self.brand, &id, &list));
            } else {
                cx.warn(messages::top_deprecated_stages(self.brand, &list));
            }
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }
}

/// `LanguageMigration` / `LanguageMigrationRequired` (`clientlib.py:194-227`).
#[derive(Debug)]
pub(crate) struct LanguageMigration {
    key: Value,
    check_fn: CheckFn,
    required: bool,
}

impl LanguageMigration {
    pub(crate) fn new(check_fn: CheckFn, required: bool) -> Self {
        Self {
            key: Value::from("language"),
            check_fn,
            required,
        }
    }
}

impl MapItem for LanguageMigration {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        match dct.get(&self.key) {
            None if self.required => Err(ValidationError::new(format!(
                "Missing required key: {}",
                self.key
            ))),
            None => Ok(()),
            Some(v) => validate_context(
                || format!("At key: {}", self.key),
                || self.check_fn.check(&translate_language(v)),
            ),
        }
    }

    fn apply_default(&self, dct: &mut Map) {
        if let Some(slot) = dct.get_mut(&self.key) {
            *slot = translate_language(slot);
        }
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        Some(&self.check_fn)
    }
}

/// `NotAllowed(key, check_any)` (`clientlib.py:401-404`): no `At key:` context.
#[derive(Debug)]
pub(crate) struct NotAllowed(pub(crate) Value);

impl MapItem for NotAllowed {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if dct.contains_key(&self.0) {
            return Err(ValidationError::new(messages::cannot_be_overridden(
                &self.0.try_repr()?,
            )));
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.0)
    }
}

/// `WarnMutableRev('rev', check_string, condition_key='repo', condition_value=NotIn(LOCAL, META),
/// ensure_absent=True)` (`clientlib.py:294-310`).
#[derive(Debug)]
pub(crate) struct WarnMutableRev {
    inner: Validator,
    key: Value,
    brand: Brand,
}

impl WarnMutableRev {
    pub(crate) fn new(brand: Brand) -> Self {
        Self {
            inner: Validator::conditional_with_absent(
                "rev",
                CheckFn::check_string(),
                "repo",
                ConditionValue::NotIn(vec![Value::from(LOCAL), Value::from(META)]),
                true,
            ),
            key: Value::from("rev"),
            brand,
        }
    }
}

/// `re.match(r'^[a-fA-F0-9]+$', rev)`: Python's `$` also matches before a final `\n`.
fn is_hex_rev(rev: &str) -> bool {
    let body = rev.trim_suffix('\n');
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_hexdigit())
}

impl MapItem for WarnMutableRev {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.inner.check(dct, cx)?;
        if let Some(rev) = dct.get(&self.key).and_then(Value::as_str)
            && !rev.contains('.')
            && !is_hex_rev(rev)
        {
            let repo = dct
                .get(&Value::from("repo"))
                .map(Value::try_repr)
                .transpose()?
                .unwrap_or_default();
            cx.warn(messages::mutable_rev(
                self.brand,
                &self.key.try_repr()?,
                &repo,
            ));
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.inner.check_fn()
    }
}

/// `OptionalSensibleRegexAtHook` / `OptionalSensibleRegexAtTop` (`clientlib.py:313-347`).
#[derive(Debug)]
pub(crate) struct SensibleRegex {
    inner: Validator,
    key: Value,
    hook: bool,
    brand: Brand,
}

impl SensibleRegex {
    pub(crate) fn new(key: &str, hook: bool, brand: Brand) -> Self {
        Self {
            inner: Validator::optional_no_default(key, CheckFn::check_string()),
            key: Value::from(key),
            hook,
            brand,
        }
    }
}

/// The three slash-class spellings, in pre-commit's order (raw strings of 5, 4 and 5 characters).
const FWD_SLASH_RES: [&str; 3] = [r"[\\/]", r"[\/]", r"[/\\]"];

impl MapItem for SensibleRegex {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        self.inner.check(dct, cx)?;
        let Some(val) = dct.get(&self.key).and_then(Value::as_str) else {
            return Ok(());
        };
        let key = self.key.try_repr()?;
        let id = || {
            dct.get(&Value::from("id"))
                .map_or_else(|| Ok("None".to_owned()), Value::try_repr)
        };
        if val.contains("/*") {
            let msg = if self.hook {
                messages::hook_regex_glob(&key, &id()?)
            } else {
                messages::top_regex_glob(&key)
            };
            cx.warn(msg);
        }
        for fwd in FWD_SLASH_RES {
            if val.contains(fwd) {
                let msg = if self.hook {
                    messages::hook_regex_slash(self.brand, &key, &id()?, fwd)
                } else {
                    messages::top_regex_slash(self.brand, &key, fwd)
                };
                cx.warn(msg);
            }
        }
        Ok(())
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.inner.check_fn()
    }
}

/// Runs `inner` unless the map is a `repo: builtin` entry (ADR-070: pre-commit's remote-repo items must not apply
/// to builtin repos, whose own items follow; the texts of every other repo kind stay pre-commit's).
#[derive(Debug)]
pub(crate) struct UnlessBuiltin(pub(crate) Validator);

fn is_builtin(dct: &Map) -> bool {
    dct.get(&Value::from("repo")).and_then(Value::as_str) == Some(BUILTIN)
}

impl MapItem for UnlessBuiltin {
    fn check(&self, dct: &Map, cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if is_builtin(dct) {
            Ok(())
        } else {
            self.0.check(dct, cx)
        }
    }

    fn apply_default(&self, dct: &mut Map) {
        if !is_builtin(dct) {
            self.0.apply_default(dct);
        }
    }

    fn key(&self) -> Option<&Value> {
        self.0.key()
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        self.0.check_fn()
    }
}

/// `repo: builtin` has no `rev` (ARCHITECTURE §7.3), reported in cfgv's ensure-absent style.
#[derive(Debug)]
pub(crate) struct BuiltinNoRev;

impl MapItem for BuiltinNoRev {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if is_builtin(dct)
            && let Some(rev) = dct.get(&Value::from("rev"))
        {
            return Err(ValidationError::new(messages::builtin_rev(
                &rev.try_repr()?,
            )));
        }
        Ok(())
    }
}

/// An extension key: checked inside `At key: {key}` when present (snp leaves, pre-commit contexts; ARCHITECTURE
/// §8.1).
pub(crate) struct ExtKey {
    key: Value,
    check: CheckFn,
}

impl std::fmt::Debug for ExtKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtKey")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl ExtKey {
    pub(crate) fn item(key: &str, check: CheckFn) -> Validator {
        Validator::custom(Self {
            key: Value::from(key),
            check,
        })
    }
}

impl MapItem for ExtKey {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        match dct.get(&self.key) {
            None => Ok(()),
            Some(v) => validate_context(|| format!("At key: {}", self.key), || self.check.check(v)),
        }
    }

    fn key(&self) -> Option<&Value> {
        Some(&self.key)
    }

    fn check_fn(&self) -> Option<&CheckFn> {
        Some(&self.check)
    }
}

/// `interactive: true` with `language: fail` or `pygrep`: those hooks never spawn a process (ADR-085). Reported at
/// `interactive` after its own bool check.
#[derive(Debug)]
pub(crate) struct InteractiveLanguage;

impl MapItem for InteractiveLanguage {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        if dct
            .get(&Value::from("interactive"))
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Ok(());
        }
        match dct.get(&Value::from("language")).and_then(Value::as_str) {
            Some(lang @ ("fail" | "pygrep")) => validate_context(
                || "At key: interactive".to_owned(),
                || {
                    Err(ValidationError::new(messages::interactive_in_process(
                        &format!("language: {lang}"),
                    )))
                },
            ),
            _ => Ok(()),
        }
    }
}

/// `priority: <name>` must name an entry of the root `priorities` (checked once at the root, after every
/// pre-commit item, with the contexts cfgv would have produced: `At key: repos` / `At Repository(repo=…)` /
/// `At key: hooks` / `At Hook(id=…)` / `At key: priority`).
#[derive(Debug)]
pub(crate) struct PriorityAliases;

impl MapItem for PriorityAliases {
    fn check(&self, dct: &Map, _cx: &mut Cx<'_>) -> Result<(), ValidationError> {
        let defined = dct.get(&Value::from("priorities")).and_then(Value::as_map);
        let Some(repos) = dct.get(&Value::from("repos")).and_then(Value::as_array) else {
            return Ok(());
        };
        for repo in repos {
            let Some(repo) = repo.as_map() else { continue };
            let Some(hooks) = repo.get(&Value::from("hooks")).and_then(Value::as_array) else {
                continue;
            };
            for hook in hooks {
                let Some(hook) = hook.as_map() else { continue };
                let Some(name) = hook.get(&Value::from("priority")).and_then(Value::as_str) else {
                    continue;
                };
                if defined.is_some_and(|d| d.contains_key(&Value::from(name))) {
                    continue;
                }
                let repo_id = repo
                    .get(&Value::from("repo"))
                    .map_or_else(|| Ok("MISSING".to_owned()), Value::try_repr)?;
                let hook_id = hook
                    .get(&Value::from("id"))
                    .map_or_else(|| Ok("MISSING".to_owned()), Value::try_repr)?;
                return Err(ValidationError::new(messages::priority_unknown(
                    &snp_pycompat::repr::repr_str(name),
                ))
                .with_context("At key: priority")
                .with_context(format!("At Hook(id={hook_id})"))
                .with_context("At key: hooks")
                .with_context(format!("At Repository(repo={repo_id})"))
                .with_context("At key: repos"));
            }
        }
        Ok(())
    }
}

/// `minimum_snp_version` (COMPATIBILITY §7.2): `MAJOR[.MINOR[.PATCH]]` compared with snp's own version.
pub(crate) fn check_min_snp_version() -> CheckFn {
    CheckFn::check_and([
        CheckFn::check_string(),
        CheckFn::custom(|v| {
            let s = v.as_str().unwrap_or_default();
            let Some(want) = parse_semver(s) else {
                return Err(ValidationError::new(messages::bad_snp_version(
                    &v.try_repr()?,
                )));
            };
            let have = parse_semver(profile::SNP_VERSION).unwrap_or([0, 0, 0]);
            if want > have {
                return Err(ValidationError::new(messages::min_snp_version(s)));
            }
            Ok(())
        }),
    ])
}

/// `MAJOR[.MINOR[.PATCH]]` with ASCII digits only.
fn parse_semver(s: &str) -> Option<[u64; 3]> {
    let mut out = [0_u64; 3];
    let mut n = 0_usize;
    for part in s.split('.') {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let slot = out.get_mut(n)?;
        *slot = part.parse().ok()?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rev_matches_python() {
        assert!(is_hex_rev("deadbeef"));
        assert!(is_hex_rev("abc\n"));
        assert!(!is_hex_rev("\n"));
        assert!(!is_hex_rev(""));
        assert!(!is_hex_rev("HEAD"));
        assert!(!is_hex_rev("abc\n\n"));
    }

    #[test]
    fn semver() {
        assert_eq!(parse_semver("2"), Some([2, 0, 0]));
        assert_eq!(parse_semver("2.1.3"), Some([2, 1, 3]));
        assert_eq!(parse_semver("2.1.3.4"), None);
        assert_eq!(parse_semver("v2"), None);
        assert_eq!(parse_semver(""), None);
        assert_eq!(parse_semver("1..2"), None);
    }

    #[test]
    fn min_version_errors() {
        assert!(min_version_error(Brand::PreCommit, "4.7.0").is_none());
        assert!(min_version_error(Brand::PreCommit, "0").is_none());
        assert!(min_version_error(Brand::PreCommit, "4.7.0.0").is_some());
        assert_eq!(
            min_version_error(Brand::PreCommit, "abc").map(|e| e.message().to_owned()),
            Some("invalid literal for int() with base 10: 'abc'".to_owned())
        );
    }
}
