//! snp's lefthook-inspired extension keys (ARCHITECTURE §8, COMPATIBILITY §7, 05 §18.3–§18.4): the typed values,
//! their cfgv check functions (appended after pre-commit's items, ADR-039) and the parsers that turn validated
//! values into [`HookExt`] / [`RootExt`].
//!
//! Owner: WP-21.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use cfgv::{CheckFn, Map, ValidationError, Value, validate_context};
use snp_core::path::RepoPath;

use crate::messages;

/// Hook-level extension keys, in validation order (`follow` is reserved, ARCHITECTURE §8.2).
pub(crate) const HOOK_EXT_KEYS: [&str; 13] = [
    "priority",
    "env",
    "groups",
    "stage_fixed",
    "skip",
    "only",
    "timeout",
    "fail_text",
    "root",
    "use_stdin",
    "fail_on_changes",
    "interactive",
    "follow",
];

/// Root-level extension keys, in validation order.
pub(crate) const ROOT_EXT_KEYS: [&str; 10] = [
    "minimum_snp_version",
    "priorities",
    "fail_on_changes",
    "skip",
    "only",
    "allow_disable",
    "fast_path",
    "fast_path_exclude",
    "fast_path_coalesce",
    "output",
];

/// Whether `key` is a hook-level extension key.
pub(crate) fn is_hook_ext_key(key: &Value) -> bool {
    key.as_str().is_some_and(|k| HOOK_EXT_KEYS.contains(&k))
}

/// The maximum `timeout` (24 h, COMPATIBILITY §7.1).
const MAX_TIMEOUT: Duration = Duration::from_hours(24);

/// Hook-level extension keys (§8.2).
#[derive(Debug, Clone, Default)]
pub struct HookExt {
    /// `priority`.
    pub priority: Option<Priority>,
    /// `env` (literal values, applied after language activation).
    pub env: Vec<(Arc<str>, Arc<str>)>,
    /// `groups`.
    pub groups: Vec<Arc<str>>,
    /// `stage_fixed` (§8.3).
    pub stage_fixed: Option<StageFixed>,
    /// `skip` conditions (list items OR).
    pub skip: Vec<Condition>,
    /// `only` conditions (list items OR).
    pub only: Vec<Condition>,
    /// `timeout` (Go duration).
    pub timeout: Option<Duration>,
    /// `fail_text`.
    pub fail_text: Option<Arc<str>>,
    /// `root` (relative directory).
    pub root: Option<RepoPath>,
    /// `use_stdin`.
    pub use_stdin: bool,
    /// `fail_on_changes`.
    pub fail_on_changes: Option<FailOnChanges>,
    /// `interactive`: the hook reads the terminal and writes to it directly (ADR-085).
    pub interactive: bool,
}

/// Root-level extension keys (§8.2).
#[derive(Debug, Clone)]
pub struct RootExt {
    /// `minimum_snp_version`.
    pub minimum_snp_version: Option<Arc<str>>,
    /// `priorities` (alias → value).
    pub priorities: indexmap::IndexMap<Arc<str>, u32>,
    /// `fail_on_changes`.
    pub fail_on_changes: FailOnChanges,
    /// `skip` conditions.
    pub skip: Vec<Condition>,
    /// `only` conditions.
    pub only: Vec<Condition>,
    /// `allow_disable` (`false` ⇒ `SNP=0` is ignored).
    pub allow_disable: bool,
    /// `fast_path`.
    pub fast_path: FastPath,
    /// `fast_path_exclude` (hook ids).
    pub fast_path_exclude: Vec<Arc<str>>,
    /// `fast_path_coalesce`.
    pub fast_path_coalesce: bool,
    /// `output`.
    pub output: OutputMode,
}

impl Default for RootExt {
    /// `allow_disable: true`, `fast_path: Auto`, `fail_on_changes: Always`, `output: Default`, everything else empty.
    fn default() -> Self {
        Self {
            minimum_snp_version: None,
            priorities: indexmap::IndexMap::new(),
            fail_on_changes: FailOnChanges::Always,
            skip: Vec::new(),
            only: Vec::new(),
            allow_disable: true,
            fast_path: FastPath::Auto,
            fast_path_exclude: Vec::new(),
            fast_path_coalesce: false,
            output: OutputMode::Default,
        }
    }
}

/// `priority`: a number or an alias from `priorities`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Priority {
    /// A literal priority (lower runs first).
    Value(u32),
    /// A name from the root `priorities` map.
    Alias(Arc<str>),
}

/// `stage_fixed` (§8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StageFixed {
    /// `true` / `verify`: re-run once before staging a failed fixer's changes.
    Verify,
    /// `trust`: stage without the re-run.
    Trust,
}

/// `fail_on_changes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailOnChanges {
    /// pre-commit's behaviour.
    Always,
    /// Modifications never fail the hook.
    Never,
    /// Fail only when `CI` is truthy.
    Ci,
    /// Fail only when `CI` is not truthy.
    NonCi,
}

/// `fast_path` (§7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FastPath {
    /// Never run pre-commit-hooks natively.
    Off,
    /// NATIVE hooks only.
    Exact,
    /// NATIVE plus gated NATIVE-WITH-CAVEATS hooks (default).
    Auto,
}

/// `output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputMode {
    /// pre-commit's output.
    Default,
    /// Footer with wall time, durations and counts.
    Summary,
    /// Failures only.
    Quiet,
}

/// One `skip` / `only` condition; keys of one map AND (`All`), list items OR.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Condition {
    /// A merge is in progress.
    Merge,
    /// A rebase is in progress.
    Rebase,
    /// The commit being created is a merge commit.
    MergeCommit,
    /// A cherry-pick is in progress.
    CherryPick,
    /// A revert is in progress.
    Revert,
    /// A bisect is in progress.
    Bisect,
    /// `{ref: glob}` against the current branch.
    Ref(Arc<str>),
    /// `{env: NAME}`: the variable is non-empty.
    Env(Arc<str>),
    /// `{ci: bool}`.
    Ci(bool),
    /// `{run: cmd}`: `sh -c` exits 0 (30 s, cached per run).
    Run(Arc<str>),
    /// Every condition holds (keys of one map).
    All(Vec<Self>),
}

// --- validation ------------------------------------------------------------------------------------------------

/// `priority` (hook) and the values of `priorities` (root): a `u32`; `alias` additionally allows a name.
fn check_priority_value(v: &Value, alias: bool) -> Result<(), ValidationError> {
    match v {
        Value::Int(i) => {
            if u32::try_from(*i).is_ok() {
                Ok(())
            } else {
                Err(ValidationError::new(messages::priority_range(
                    &v.try_repr()?,
                )))
            }
        }
        Value::BigInt(_) => Err(ValidationError::new(messages::priority_range(
            &v.try_repr()?,
        ))),
        Value::String(s) if alias && !s.is_empty() => Ok(()),
        _ => Err(ValidationError::new(messages::priority_type(v.type_name()))),
    }
}

/// `priority`: a `u32` or a name of the root `priorities` (the name is checked at the root,
/// [`crate::items::PriorityAliases`]).
pub(crate) fn check_priority() -> CheckFn {
    CheckFn::custom(|v| check_priority_value(v, true))
}

/// `priorities`: a map of names to `u32`.
pub(crate) fn check_priorities() -> CheckFn {
    CheckFn::custom(|v| {
        let map = expect_map(v)?;
        for (k, val) in map {
            validate_context(
                || format!("For key: {k}"),
                || CheckFn::check_string().check(k),
            )?;
            validate_context(
                || format!("At key: {k}"),
                || check_priority_value(val, false),
            )?;
        }
        Ok(())
    })
}

/// `check_type(dict)` (`Expected dict got list`).
fn expect_map(v: &Value) -> Result<&Map, ValidationError> {
    CheckFn::check_type(cfgv::ValueType::Map, None).check(v)?;
    v.as_map()
        .ok_or_else(|| ValidationError::new(format!("Expected dict got {}", v.type_name())))
}

/// `env`: a map of variable names (non-empty, no `=`, no NUL) to string values (no NUL).
pub(crate) fn check_env() -> CheckFn {
    CheckFn::custom(|v| {
        let map = expect_map(v)?;
        for (k, val) in map {
            validate_context(
                || format!("For key: {k}"),
                || {
                    CheckFn::check_string().check(k)?;
                    let name = k.as_str().unwrap_or_default();
                    if name.is_empty() || name.contains('=') || name.contains('\0') {
                        return Err(ValidationError::new(messages::env_name(&k.try_repr()?)));
                    }
                    Ok(())
                },
            )?;
            validate_context(
                || format!("At key: {k}"),
                || {
                    CheckFn::check_string().check(val)?;
                    if val.as_str().is_some_and(|s| s.contains('\0')) {
                        return Err(ValidationError::new(messages::env_value(&val.try_repr()?)));
                    }
                    Ok(())
                },
            )?;
        }
        Ok(())
    })
}

/// A group name (ADR-080): non-empty, no whitespace, not starting with `@`.
pub(crate) fn is_valid_group(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('@') && !name.chars().any(char::is_whitespace)
}

/// `groups` / overlay `skip_groups`: a list of group names.
pub(crate) fn check_groups() -> CheckFn {
    CheckFn::check_array(CheckFn::custom(|v| {
        CheckFn::check_string().check(v)?;
        if v.as_str().is_some_and(is_valid_group) {
            Ok(())
        } else {
            Err(ValidationError::new(messages::group_name(&v.try_repr()?)))
        }
    }))
}

/// `stage_fixed`: a bool, `verify` or `trust`.
pub(crate) fn check_stage_fixed() -> CheckFn {
    CheckFn::custom(|v| match v {
        Value::Bool(_) => Ok(()),
        _ => CheckFn::check_one_of([
            Value::Bool(false),
            Value::Bool(true),
            Value::from("trust"),
            Value::from("verify"),
        ])
        .check(v),
    })
}

/// `fail_on_changes`: `always`, `never`, `ci`, `non-ci`, or a bool (`true` = `always`, `false` = `never`).
pub(crate) fn check_fail_on_changes() -> CheckFn {
    CheckFn::custom(|v| match v {
        Value::Bool(_) => Ok(()),
        _ => CheckFn::check_one_of(["always", "ci", "never", "non-ci"]).check(v),
    })
}

/// The repository states of a condition.
const CONDITION_STATES: [&str; 6] = [
    "bisect",
    "cherry-pick",
    "merge",
    "merge-commit",
    "rebase",
    "revert",
];
/// The keys of a condition map.
const CONDITION_KEYS: [&str; 4] = ["ci", "env", "ref", "run"];

/// One `skip` / `only` list item: a state name or a map of `ref` / `env` / `ci` / `run`.
fn check_condition_item(v: &Value) -> Result<(), ValidationError> {
    match v {
        Value::Map(m) => {
            CheckFn::check_type(cfgv::ValueType::Map, None).check(v)?;
            cfgv::Validator::no_additional_keys(
                CONDITION_KEYS.iter().map(|k| Value::from(*k)).collect(),
            )
            .check(m, &mut cfgv::Cx::discard())?;
            if m.is_empty() {
                return Err(ValidationError::new(messages::EMPTY_CONDITION));
            }
            for (k, val) in m {
                validate_context(
                    || format!("At key: {k}"),
                    || match k.as_str().unwrap_or_default() {
                        "ci" => CheckFn::check_bool().check(val),
                        key @ ("env" | "run") => {
                            CheckFn::check_string().check(val)?;
                            if val.as_str().is_some_and(str::is_empty) {
                                return Err(ValidationError::new(messages::empty_condition_value(
                                    key,
                                )));
                            }
                            Ok(())
                        }
                        _ => {
                            CheckFn::check_string().check(val)?;
                            let glob = val.as_str().unwrap_or_default();
                            match globset::Glob::new(glob) {
                                Ok(_) => Ok(()),
                                Err(e) => Err(ValidationError::new(messages::bad_glob(
                                    &val.try_repr()?,
                                    &e.kind().to_string(),
                                ))),
                            }
                        }
                    },
                )?;
            }
            Ok(())
        }
        _ => CheckFn::check_one_of(CONDITION_STATES).check(v),
    }
}

/// `skip` / `only`: a bool, one condition, or a list of conditions (list items OR, keys of one map AND).
pub(crate) fn check_conditions() -> CheckFn {
    CheckFn::custom(|v| match v {
        Value::Bool(_) => Ok(()),
        Value::List(_) | Value::Tuple(_) => {
            CheckFn::check_array(CheckFn::custom(check_condition_item)).check(v)
        }
        _ => check_condition_item(v),
    })
}

/// `timeout`: a Go duration in `(0, 24h]`.
pub(crate) fn check_timeout() -> CheckFn {
    CheckFn::custom(|v| {
        CheckFn::check_string().check(v)?;
        let d = parse_go_duration(v.as_str().unwrap_or_default()).map_err(ValidationError::new)?;
        if d.is_zero() || d > MAX_TIMEOUT {
            return Err(ValidationError::new(messages::timeout_range(
                &v.try_repr()?,
            )));
        }
        Ok(())
    })
}

/// `root`, normalized: `./a//b/` → `a/b`; `None` for an absolute path, a `..` component, NUL or nothing left.
pub(crate) fn normalize_root(s: &str) -> Option<RepoPath> {
    if s.starts_with('/') || s.starts_with('\\') || s.contains('\0') {
        return None;
    }
    let parts: Vec<&str> = s
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.contains(&"..") {
        return None;
    }
    // A `:` in the first component is a Windows drive (`C:/x`, `C:x`), a `\\?\` / `//?/` device path or an NTFS
    // stream: on Windows it escapes the repository or resolves against a drive's cwd. Rejected everywhere, so a
    // config means the same on every platform.
    if parts.first().is_some_and(|p| p.contains(':')) {
        return None;
    }
    let mut joined = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            joined.push('/');
        }
        joined.push_str(p);
    }
    RepoPath::new(joined.as_bytes()).filter(RepoPath::is_contained)
}

/// `root`: a relative directory inside the repository.
pub(crate) fn check_root() -> CheckFn {
    CheckFn::custom(|v| {
        CheckFn::check_string().check(v)?;
        if v.as_str().and_then(normalize_root).is_some() {
            Ok(())
        } else {
            Err(ValidationError::new(messages::root_dir(&v.try_repr()?)))
        }
    })
}

/// `follow`: reserved (ARCHITECTURE §8.2).
pub(crate) fn check_reserved(key: &'static str) -> CheckFn {
    CheckFn::custom(move |_| {
        Err(ValidationError::new(messages::reserved_key(
            &snp_pycompat::repr::repr_str(key),
        )))
    })
}

/// `interactive`: a bool; `true` is refused for a hook that never spawns a process (`in_process` names its kind:
/// meta and `repo: builtin` hooks, `language: fail` / `pygrep`; ADR-085).
pub(crate) fn check_interactive(in_process: Option<&'static str>) -> CheckFn {
    CheckFn::custom(move |v| {
        CheckFn::check_bool().check(v)?;
        match in_process {
            Some(kind) if v.as_bool() == Some(true) => {
                Err(ValidationError::new(messages::interactive_in_process(kind)))
            }
            _ => Ok(()),
        }
    })
}

/// The check of one hook-level extension key.
pub(crate) fn hook_check(key: &'static str) -> CheckFn {
    match key {
        "priority" => check_priority(),
        "env" => check_env(),
        "groups" => check_groups(),
        "stage_fixed" => check_stage_fixed(),
        "skip" | "only" => check_conditions(),
        "timeout" => check_timeout(),
        "fail_text" => CheckFn::check_string(),
        "root" => check_root(),
        "use_stdin" => CheckFn::check_bool(),
        "fail_on_changes" => check_fail_on_changes(),
        "interactive" => check_interactive(None),
        _ => check_reserved(key),
    }
}

/// The check of one root-level extension key.
pub(crate) fn root_check(key: &'static str) -> CheckFn {
    match key {
        "minimum_snp_version" => crate::items::check_min_snp_version(),
        "priorities" => check_priorities(),
        "fail_on_changes" => check_fail_on_changes(),
        "skip" | "only" => check_conditions(),
        "fast_path" => CheckFn::check_one_of(["auto", "exact", "off"]),
        "fast_path_exclude" => CheckFn::check_array(CheckFn::check_string()),
        "output" => CheckFn::check_one_of(["default", "quiet", "summary"]),
        _ => CheckFn::check_bool(),
    }
}

// --- parsing (validated values only; never fails) --------------------------------------------------------------

fn arc(s: &str) -> Arc<str> {
    Arc::from(s)
}

/// `fail_on_changes` value.
pub(crate) fn parse_fail_on_changes(v: &Value) -> Option<FailOnChanges> {
    match v {
        Value::Bool(true) => Some(FailOnChanges::Always),
        Value::Bool(false) => Some(FailOnChanges::Never),
        _ => match v.as_str()? {
            "always" => Some(FailOnChanges::Always),
            "never" => Some(FailOnChanges::Never),
            "ci" => Some(FailOnChanges::Ci),
            "non-ci" => Some(FailOnChanges::NonCi),
            _ => None,
        },
    }
}

fn parse_condition_item(v: &Value) -> Option<Condition> {
    match v {
        Value::Map(m) => {
            let mut all = Vec::new();
            for (k, val) in m {
                let c = match k.as_str()? {
                    "ref" => Condition::Ref(arc(val.as_str()?)),
                    "env" => Condition::Env(arc(val.as_str()?)),
                    "ci" => Condition::Ci(val.as_bool()?),
                    "run" => Condition::Run(arc(val.as_str()?)),
                    _ => return None,
                };
                all.push(c);
            }
            if all.len() == 1 {
                all.pop()
            } else {
                Some(Condition::All(all))
            }
        }
        _ => Some(match v.as_str()? {
            "merge" => Condition::Merge,
            "rebase" => Condition::Rebase,
            "merge-commit" => Condition::MergeCommit,
            "cherry-pick" => Condition::CherryPick,
            "revert" => Condition::Revert,
            "bisect" => Condition::Bisect,
            _ => return None,
        }),
    }
}

/// `skip` / `only` value → the OR-list of conditions (`true` = always, `false` = never).
pub(crate) fn parse_conditions(v: &Value) -> Vec<Condition> {
    match v {
        Value::Bool(true) => vec![Condition::All(Vec::new())],
        Value::Bool(false) => Vec::new(),
        Value::List(items) | Value::Tuple(items) => {
            items.iter().filter_map(parse_condition_item).collect()
        }
        other => parse_condition_item(other).into_iter().collect(),
    }
}

/// Applies one validated hook-level extension key to `ext` (later keys override earlier ones; `env` merges with
/// the new values winning, ARCHITECTURE §8.4).
pub(crate) fn apply_hook_key(ext: &mut HookExt, key: &str, v: &Value) {
    match key {
        "priority" => {
            ext.priority = match v {
                Value::String(s) => Some(Priority::Alias(arc(s))),
                _ => v
                    .as_int()
                    .and_then(|i| u32::try_from(i).ok())
                    .map(Priority::Value),
            };
        }
        "env" => {
            if let Some(m) = v.as_map() {
                for (k, val) in m {
                    if let (Some(k), Some(val)) = (k.as_str(), val.as_str()) {
                        match ext.env.iter_mut().find(|(name, _)| &**name == k) {
                            Some(slot) => slot.1 = arc(val),
                            None => ext.env.push((arc(k), arc(val))),
                        }
                    }
                }
            }
        }
        "groups" => {
            ext.groups = v
                .as_array()
                .unwrap_or_default()
                .iter()
                .filter_map(Value::as_str)
                .map(arc)
                .collect();
        }
        "stage_fixed" => {
            ext.stage_fixed = match v {
                Value::Bool(true) => Some(StageFixed::Verify),
                Value::String(s) if s == "verify" => Some(StageFixed::Verify),
                Value::String(s) if s == "trust" => Some(StageFixed::Trust),
                _ => None,
            };
        }
        "skip" => ext.skip = parse_conditions(v),
        "only" => ext.only = parse_conditions(v),
        "timeout" => ext.timeout = v.as_str().and_then(|s| parse_go_duration(s).ok()),
        "fail_text" => ext.fail_text = v.as_str().map(arc),
        "root" => ext.root = v.as_str().and_then(normalize_root),
        "use_stdin" => ext.use_stdin = v.as_bool().unwrap_or(false),
        "fail_on_changes" => ext.fail_on_changes = parse_fail_on_changes(v),
        "interactive" => ext.interactive = v.as_bool().unwrap_or(false),
        _ => {}
    }
}

/// The hook-level extension keys of a validated hook dict, in one pass over the dict. Each key sets only its own
/// field and a dict holds a key once, so the dict's order gives the result `HOOK_EXT_KEYS` order would; every other
/// key (pre-commit's, unknown, `follow`, non-string) falls to [`apply_hook_key`]'s no-op arm.
pub(crate) fn parse_hook_ext(m: &Map) -> HookExt {
    let mut ext = HookExt::default();
    for (k, v) in m {
        if let Some(key) = k.as_str() {
            apply_hook_key(&mut ext, key, v);
        }
    }
    ext
}

/// Applies one validated root-level extension key to `ext` (other keys are ignored).
fn apply_root_key(ext: &mut RootExt, key: &str, v: &Value) {
    match key {
        "minimum_snp_version" => {
            if let Some(s) = v.as_str() {
                ext.minimum_snp_version = Some(arc(s));
            }
        }
        "priorities" => {
            if let Some(p) = v.as_map() {
                for (name, value) in p {
                    if let (Some(name), Some(value)) = (
                        name.as_str(),
                        value.as_int().and_then(|i| u32::try_from(i).ok()),
                    ) {
                        ext.priorities.insert(arc(name), value);
                    }
                }
            }
        }
        "fail_on_changes" => {
            if let Some(f) = parse_fail_on_changes(v) {
                ext.fail_on_changes = f;
            }
        }
        "skip" => ext.skip = parse_conditions(v),
        "only" => ext.only = parse_conditions(v),
        "allow_disable" => {
            if let Some(b) = v.as_bool() {
                ext.allow_disable = b;
            }
        }
        "fast_path" => {
            ext.fast_path = match v.as_str() {
                Some("off") => FastPath::Off,
                Some("exact") => FastPath::Exact,
                _ => FastPath::Auto,
            };
        }
        "fast_path_exclude" => {
            if let Some(ids) = v.as_array() {
                ext.fast_path_exclude = ids.iter().filter_map(Value::as_str).map(arc).collect();
            }
        }
        "fast_path_coalesce" => {
            if let Some(b) = v.as_bool() {
                ext.fast_path_coalesce = b;
            }
        }
        "output" => {
            ext.output = match v.as_str() {
                Some("summary") => OutputMode::Summary,
                Some("quiet") => OutputMode::Quiet,
                _ => OutputMode::Default,
            };
        }
        _ => {}
    }
}

/// The root-level extension keys of a validated config, in one pass over the dict (each key sets only its own
/// field, as in [`parse_hook_ext`]).
pub(crate) fn parse_root_ext(m: &Map) -> RootExt {
    let mut ext = RootExt::default();
    for (k, v) in m {
        if let Some(key) = k.as_str() {
            apply_root_key(&mut ext, key, v);
        }
    }
    ext
}

// --- Go durations ----------------------------------------------------------------------------------------------

/// `1 << 63` (Go's `Duration` bound).
const LIMIT: u64 = 1 << 63;

/// Go's `time` package `quote`: `"…"` with `"` and `\` escaped and every byte of a control or non-ASCII
/// character written as `\xNN`.
fn go_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if c < ' ' || !c.is_ascii() {
            let mut buf = [0_u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                write!(out, "\\x{b:02x}").unwrap_or_default();
            }
        } else {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out.push('"');
    out
}

/// Go's `leadingInt`: the leading decimal digits (`None` on overflow).
fn leading_int(s: &str) -> Option<(u64, &str)> {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = s.split_at_checked(end)?;
    let mut x: u64 = 0;
    for b in digits.bytes() {
        if x > LIMIT / 10 {
            return None;
        }
        x = x
            .checked_mul(10)?
            .checked_add(u64::from(b.wrapping_sub(b'0')))?;
        if x > LIMIT {
            return None;
        }
    }
    Some((x, rest))
}

/// Go's `leadingFraction`: digits after the point as `(value, scale)`; digits past `u64` precision are dropped.
fn leading_fraction(s: &str) -> (u64, f64, &str) {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = s.split_at_checked(end).unwrap_or((s, ""));
    let mut x: u64 = 0;
    let mut scale = 1.0_f64;
    let mut overflow = false;
    for b in digits.bytes() {
        if overflow {
            continue;
        }
        if x > (LIMIT - 1) / 10 {
            overflow = true;
            continue;
        }
        let Some(y) = x
            .checked_mul(10)
            .and_then(|y| y.checked_add(u64::from(b.wrapping_sub(b'0'))))
        else {
            overflow = true;
            continue;
        };
        if y > LIMIT {
            overflow = true;
            continue;
        }
        x = y;
        scale *= 10.0;
    }
    (x, scale, rest)
}

/// The nanoseconds of a Go duration unit.
fn unit_nanos(u: &str) -> Option<u64> {
    Some(match u {
        "ns" => 1,
        "us" | "\u{b5}s" | "\u{3bc}s" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        _ => return None,
    })
}

/// Go's `time.ParseDuration` (lefthook `timeout` values, 05 §18): `[-+]?([0-9]*(\.[0-9]*)?[a-z]+)+` with the
/// units `ns`, `us` (`µs`, `μs`), `ms`, `s`, `m`, `h`, or `0`; the error texts are Go's (`time: invalid duration
/// "x"`, `time: missing unit in duration "1"`, `time: unknown unit "d" in duration "1d"`). A negative duration
/// (valid in Go) is rejected: a timeout cannot be negative.
pub fn parse_go_duration(s: &str) -> Result<Duration, String> {
    let orig = s;
    let invalid = || format!("time: invalid duration {}", go_quote(orig));
    let mut rest = s;
    let mut neg = false;
    if let Some(r) = rest.strip_prefix('-') {
        neg = true;
        rest = r;
    } else if let Some(r) = rest.strip_prefix('+') {
        rest = r;
    }
    if rest == "0" {
        return Ok(Duration::ZERO);
    }
    if rest.is_empty() {
        return Err(invalid());
    }
    let mut total: u64 = 0;
    while !rest.is_empty() {
        let first = rest.as_bytes().first().copied().unwrap_or(0);
        if !(first == b'.' || first.is_ascii_digit()) {
            return Err(invalid());
        }
        let before = rest.len();
        let (mut v, after_int) = leading_int(rest).ok_or_else(invalid)?;
        rest = after_int;
        let pre = before != rest.len();
        let mut f = 0_u64;
        let mut scale = 1.0_f64;
        let mut post = false;
        if let Some(r) = rest.strip_prefix('.') {
            let before = r.len();
            let (fv, fs, after) = leading_fraction(r);
            f = fv;
            scale = fs;
            post = before != after.len();
            rest = after;
        }
        if !pre && !post {
            return Err(invalid());
        }
        let unit_len = rest
            .bytes()
            .take_while(|b| *b != b'.' && !b.is_ascii_digit())
            .count();
        if unit_len == 0 {
            return Err(format!("time: missing unit in duration {}", go_quote(orig)));
        }
        let (u, after_unit) = rest.split_at_checked(unit_len).ok_or_else(invalid)?;
        rest = after_unit;
        let Some(unit) = unit_nanos(u) else {
            return Err(format!(
                "time: unknown unit {} in duration {}",
                go_quote(u),
                go_quote(orig)
            ));
        };
        if v > LIMIT.checked_div(unit).unwrap_or(0) {
            return Err(invalid());
        }
        v = v.checked_mul(unit).ok_or_else(invalid)?;
        if f > 0 {
            #[expect(
                clippy::as_conversions,
                clippy::cast_precision_loss,
                reason = "Go's float64(f) * (float64(unit) / scale)"
            )]
            let frac = (f as f64) * ((unit as f64) / scale);
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "Go's uint64(…) of a non-negative finite value below 2^63 (saturating in Rust)"
            )]
            let frac = frac as u64;
            v = v.checked_add(frac).ok_or_else(invalid)?;
            if v > LIMIT {
                return Err(invalid());
            }
        }
        total = total.checked_add(v).ok_or_else(invalid)?;
        if total > LIMIT {
            return Err(invalid());
        }
    }
    if neg {
        if total == 0 {
            return Ok(Duration::ZERO);
        }
        return Err(format!(
            "{} (negative durations are not supported)",
            invalid()
        ));
    }
    if total > LIMIT - 1 {
        return Err(invalid());
    }
    Ok(Duration::from_nanos(total))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// The hashed probe the parsers used before their single pass.
    fn probe<'a>(m: &'a Map, k: &str) -> Option<&'a Value> {
        m.get(&Value::from(k))
    }

    /// Reference model of [`parse_hook_ext`]: one hashed probe per `HOOK_EXT_KEYS` entry, in that order.
    fn parse_hook_ext_by_probes(m: &Map) -> HookExt {
        let mut ext = HookExt::default();
        for key in HOOK_EXT_KEYS {
            if let Some(v) = probe(m, key) {
                apply_hook_key(&mut ext, key, v);
            }
        }
        ext
    }

    /// Reference model of [`parse_root_ext`]: one hashed probe per root extension key, in `ROOT_EXT_KEYS` order.
    fn parse_root_ext_by_probes(m: &Map) -> RootExt {
        let mut ext = RootExt::default();
        if let Some(v) = probe(m, "minimum_snp_version").and_then(Value::as_str) {
            ext.minimum_snp_version = Some(arc(v));
        }
        if let Some(p) = probe(m, "priorities").and_then(Value::as_map) {
            for (k, v) in p {
                if let (Some(k), Some(v)) =
                    (k.as_str(), v.as_int().and_then(|i| u32::try_from(i).ok()))
                {
                    ext.priorities.insert(arc(k), v);
                }
            }
        }
        if let Some(f) = probe(m, "fail_on_changes").and_then(parse_fail_on_changes) {
            ext.fail_on_changes = f;
        }
        if let Some(v) = probe(m, "skip") {
            ext.skip = parse_conditions(v);
        }
        if let Some(v) = probe(m, "only") {
            ext.only = parse_conditions(v);
        }
        if let Some(b) = probe(m, "allow_disable").and_then(Value::as_bool) {
            ext.allow_disable = b;
        }
        ext.fast_path = match probe(m, "fast_path").and_then(Value::as_str) {
            Some("off") => FastPath::Off,
            Some("exact") => FastPath::Exact,
            _ => FastPath::Auto,
        };
        if let Some(ids) = probe(m, "fast_path_exclude").and_then(Value::as_array) {
            ext.fast_path_exclude = ids.iter().filter_map(Value::as_str).map(arc).collect();
        }
        if let Some(b) = probe(m, "fast_path_coalesce").and_then(Value::as_bool) {
            ext.fast_path_coalesce = b;
        }
        ext.output = match probe(m, "output").and_then(Value::as_str) {
            Some("summary") => OutputMode::Summary,
            Some("quiet") => OutputMode::Quiet,
            _ => OutputMode::Default,
        };
        ext
    }

    /// Values for any key: the valid spellings of every extension key and values of the wrong type or range (the
    /// parsers never fail and must ignore what they cannot use exactly as before).
    fn values() -> Vec<Value> {
        let list = |items: Vec<Value>| Value::List(items);
        vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(3),
            Value::Int(-1),
            Value::Int(1 << 40),
            Value::Float(2.5),
            Value::from(""),
            Value::from("early"),
            Value::from("1.2"),
            Value::from("trust"),
            Value::from("verify"),
            Value::from("ci"),
            Value::from("non-ci"),
            Value::from("never"),
            Value::from("always"),
            Value::from("merge"),
            Value::from("1m30s"),
            Value::from("./sub//dir/"),
            Value::from("/abs"),
            Value::from("off"),
            Value::from("exact"),
            Value::from("summary"),
            Value::from("quiet"),
            list(vec![Value::from("g1"), Value::Int(2), Value::from("g2")]),
            list(vec![
                Value::from("rebase"),
                cfgv::value_map! {"ref" => "main", "ci" => true},
            ]),
            cfgv::value_map! {"env" => "CI"},
            cfgv::value_map! {"A" => "1", "B" => "x"},
            cfgv::value_map! {"early" => 1, "late" => 9, "bad" => -1},
            Value::Tuple(vec![Value::from("bisect")]),
        ]
    }

    /// Keys of a dict under test: every extension key (hook and root), pre-commit keys, unknown keys and non-string
    /// keys that spell an extension key.
    fn keys() -> Vec<Value> {
        let mut keys: Vec<Value> = HOOK_EXT_KEYS
            .iter()
            .chain(&ROOT_EXT_KEYS)
            .chain(&crate::schema::HOOK_KEYS)
            .chain(&["repos", "zzz", ""])
            .map(|k| Value::from(*k))
            .collect();
        keys.push(Value::Bytes(b"env".to_vec()));
        keys.push(Value::Tuple(vec![Value::from("skip")]));
        keys.push(Value::Int(1));
        keys.push(Value::Null);
        keys
    }

    fn dict() -> impl Strategy<Value = Map> {
        let entry = (prop::sample::select(keys()), prop::sample::select(values()));
        prop::collection::vec(entry, 0..40)
            .prop_shuffle()
            .prop_map(|entries| entries.into_iter().collect())
    }

    proptest! {
        /// The single pass parses exactly what the ordered hashed probes parsed, for any key order and mix.
        #[test]
        fn parse_hook_ext_matches_the_ordered_probes(m in dict()) {
            prop_assert_eq!(
                format!("{:?}", parse_hook_ext(&m)),
                format!("{:?}", parse_hook_ext_by_probes(&m))
            );
        }

        #[test]
        fn parse_root_ext_matches_the_ordered_probes(m in dict()) {
            prop_assert_eq!(
                format!("{:?}", parse_root_ext(&m)),
                format!("{:?}", parse_root_ext_by_probes(&m))
            );
        }
    }

    /// The single passes hand every string key to the `apply_*_key` matches, so those must act on their extension
    /// keys only: every other key leaves the default, and every extension key but `follow` changes something.
    #[test]
    fn only_extension_keys_apply() {
        let hook_default = format!("{:?}", HookExt::default());
        let root_default = format!("{:?}", RootExt::default());
        let others = crate::schema::HOOK_KEYS
            .iter()
            .chain(&["repos", "follow", "zzz", "", "Priority", "env "]);
        for key in others.clone().chain(&ROOT_EXT_KEYS) {
            if HOOK_EXT_KEYS.contains(key) && *key != "follow" {
                continue;
            }
            for v in values() {
                let mut ext = HookExt::default();
                apply_hook_key(&mut ext, key, &v);
                assert_eq!(format!("{ext:?}"), hook_default, "hook {key:?} {v:?}");
            }
        }
        for key in others.chain(&HOOK_EXT_KEYS) {
            if ROOT_EXT_KEYS.contains(key) {
                continue;
            }
            for v in values() {
                let mut ext = RootExt::default();
                apply_root_key(&mut ext, key, &v);
                assert_eq!(format!("{ext:?}"), root_default, "root {key:?} {v:?}");
            }
        }
        let changes_hook = |key: &str| {
            values().iter().any(|v| {
                let mut ext = HookExt::default();
                apply_hook_key(&mut ext, key, v);
                format!("{ext:?}") != hook_default
            })
        };
        let changes_root = |key: &str| {
            values().iter().any(|v| {
                let mut ext = RootExt::default();
                apply_root_key(&mut ext, key, v);
                format!("{ext:?}") != root_default
            })
        };
        for key in HOOK_EXT_KEYS.iter().filter(|k| **k != "follow") {
            assert!(changes_hook(key), "hook {key:?}");
        }
        for key in ROOT_EXT_KEYS {
            assert!(changes_root(key), "root {key:?}");
        }
    }

    #[test]
    fn go_durations() {
        let ok = |s: &str| parse_go_duration(s).unwrap();
        assert_eq!(ok("0"), Duration::ZERO);
        assert_eq!(ok("30s"), Duration::from_secs(30));
        assert_eq!(ok("1m30s"), Duration::from_secs(90));
        assert_eq!(ok("500ms"), Duration::from_millis(500));
        assert_eq!(ok("1h2m3.5s"), Duration::from_millis(3_723_500));
        assert_eq!(ok("1.5h"), Duration::from_mins(90));
        assert_eq!(ok(".5s"), Duration::from_millis(500));
        assert_eq!(ok("1.s"), Duration::from_secs(1));
        assert_eq!(ok("+5s"), Duration::from_secs(5));
        assert_eq!(ok("2\u{b5}s"), Duration::from_micros(2));
        assert_eq!(ok("2\u{3bc}s"), Duration::from_micros(2));
        assert_eq!(ok("3us"), Duration::from_micros(3));
        assert_eq!(ok("7ns"), Duration::from_nanos(7));
        assert_eq!(ok("-0"), Duration::ZERO);
        assert_eq!(
            ok("2562047h47m16.854775807s"),
            Duration::from_nanos((1 << 63) - 1)
        );
        let err = |s: &str| parse_go_duration(s).unwrap_err();
        assert_eq!(err(""), "time: invalid duration \"\"");
        assert_eq!(err("1"), "time: missing unit in duration \"1\"");
        assert_eq!(err("1d"), "time: unknown unit \"d\" in duration \"1d\"");
        assert_eq!(err("."), "time: invalid duration \".\"");
        assert_eq!(err("s"), "time: invalid duration \"s\"");
        assert_eq!(
            err("1é"),
            "time: unknown unit \"\\xc3\\xa9\" in duration \"1\\xc3\\xa9\""
        );
        assert_eq!(
            err("9223372036854775808ns"),
            "time: invalid duration \"9223372036854775808ns\""
        );
        assert_eq!(
            err("2562047h47m16.854775808s"),
            "time: invalid duration \"2562047h47m16.854775808s\""
        );
        assert!(err("-1s").starts_with("time: invalid duration \"-1s\""));
        assert_eq!(
            err("1s\""),
            "time: unknown unit \"s\\\"\" in duration \"1s\\\"\""
        );
    }

    #[test]
    fn roots() {
        let n = |s: &str| normalize_root(s).map(|p| p.lossy().into_owned());
        assert_eq!(n("a"), Some("a".to_owned()));
        assert_eq!(n("./a//b/"), Some("a/b".to_owned()));
        assert_eq!(n("a, b/c"), Some("a, b/c".to_owned()));
        assert_eq!(n(".."), None);
        assert_eq!(n("a/../b"), None);
        assert_eq!(n("/abs"), None);
        assert_eq!(n("."), None);
        assert_eq!(n(""), None);
        for drive in [
            "C:/Windows",
            "C:x",
            "c:\\x",
            "//?/C:/x",
            "\\\\?\\C:\\x",
            "./C:/x",
            "a:b",
            "x::$DATA",
        ] {
            assert_eq!(n(drive), None, "{drive}");
        }
        assert_eq!(n("a/b:c"), Some("a/b:c".to_owned()));
    }

    #[test]
    fn groups() {
        assert!(is_valid_group("lint"));
        assert!(!is_valid_group(""));
        assert!(!is_valid_group("@x"));
        assert!(!is_valid_group("a b"));
        assert!(!is_valid_group("a\u{a0}b"));
    }

    #[test]
    fn conditions() {
        let v = Value::List(vec![
            Value::from("merge"),
            cfgv::value_map! {"ref" => "main", "ci" => true},
        ]);
        let c = parse_conditions(&v);
        assert!(matches!(c.as_slice(), [Condition::Merge, Condition::All(all)] if all.len() == 2));
        assert!(
            matches!(parse_conditions(&Value::Bool(true)).as_slice(), [Condition::All(a)] if a.is_empty())
        );
        assert!(parse_conditions(&Value::Bool(false)).is_empty());
        assert!(check_conditions().check(&Value::from("nope")).is_err());
        assert!(
            check_conditions()
                .check(&cfgv::value_map! {"ref" => "[a"})
                .is_err()
        );
        assert!(
            check_conditions()
                .check(&cfgv::value_map! {"bogus" => 1})
                .is_err()
        );
    }
}
