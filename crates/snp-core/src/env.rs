//! The immutable environment snapshot and pre-commit's `envcontext` patches (ARCHITECTURE §2.5).
//!
//! snp never mutates the process environment (S6): `snp::entry` captures it once with
//! [`Env::capture_process`], every child gets `env_clear()` + a computed map ([`Env::apply_to`]),
//! and per-command changes are [`EnvPatch`]es applied with [`Env::patched`].

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// `os.pathsep`.
const PATHSEP: &str = if cfg!(windows) { ";" } else { ":" };

/// Variables pre-commit drops at import (`main.py:39-42`).
const DROPPED_AT_STARTUP: [&str; 2] = ["__PYVENV_LAUNCHER__", "PYTHONEXECUTABLE"];

/// Immutable environment snapshot (cheap to clone). Windows: keys compare ASCII-case-insensitively
/// and the first spelling is kept.
#[derive(Clone, Debug)]
pub struct Env(Arc<EnvInner>);

/// Variables in insertion order (a Python `dict`: overwriting keeps the position).
#[derive(Clone, Default)]
struct EnvInner {
    vars: Vec<(OsString, OsString)>,
}

impl fmt::Debug for EnvInner {
    /// Values are never printed (they may hold secrets, ADR-077): `NAME: <N bytes>`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct Len(usize);
        impl fmt::Debug for Len {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "<{} bytes>", self.0)
            }
        }
        f.debug_map()
            .entries(self.vars.iter().map(|(k, v)| (k, Len(v.len()))))
            .finish()
    }
}

fn key_eq(a: &OsStr, b: &OsStr) -> bool {
    if cfg!(windows) {
        a.as_encoded_bytes()
            .eq_ignore_ascii_case(b.as_encoded_bytes())
    } else {
        a == b
    }
}

impl EnvInner {
    fn position(&self, key: &OsStr) -> Option<usize> {
        self.vars.iter().position(|(k, _)| key_eq(k, key))
    }

    /// `env[key] = value`: an existing key keeps its position and spelling.
    fn set(&mut self, key: OsString, value: OsString) {
        match self.position(&key) {
            Some(i) => {
                if let Some(slot) = self.vars.get_mut(i) {
                    slot.1 = value;
                }
            }
            None => self.vars.push((key, value)),
        }
    }

    /// `env.pop(key, None)`.
    fn unset(&mut self, key: &OsStr) {
        self.vars.retain(|(k, _)| !key_eq(k, key));
    }
}

impl Env {
    /// The process environment minus `__PYVENV_LAUNCHER__` and `PYTHONEXECUTABLE`
    /// (`main.py:39-42`). Only `snp::entry` calls this; everything else receives an `Env`.
    pub fn capture_process() -> Self {
        #[expect(
            clippy::disallowed_methods,
            reason = "the single process-environment read (S6)"
        )]
        let vars = std::env::vars_os();
        let mut inner = EnvInner::default();
        for (k, v) in vars {
            if !DROPPED_AT_STARTUP.iter().any(|d| key_eq(&k, OsStr::new(d))) {
                inner.set(k, v);
            }
        }
        Self(Arc::new(inner))
    }

    /// A snapshot from explicit pairs (tests, hermetic child environments). A repeated key keeps
    /// its first position and spelling and takes the last value.
    pub fn from_pairs<K: Into<OsString>, V: Into<OsString>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut inner = EnvInner::default();
        for (k, v) in pairs {
            inner.set(k.into(), v.into());
        }
        Self(Arc::new(inner))
    }

    /// `os.environ.get(key)`.
    pub fn get(&self, key: &str) -> Option<&OsStr> {
        let i = self.0.position(OsStr::new(key))?;
        self.0.vars.get(i).map(|(_, v)| v.as_os_str())
    }

    /// `os.environ.get(key) or …`: `None` for unset or empty values.
    pub fn get_nonempty(&self, key: &str) -> Option<&OsStr> {
        self.get(key).filter(|v| !v.is_empty())
    }

    /// `key in os.environ` (presence semantics: `PRE_COMMIT_NO_CONCURRENCY`, `TRAVIS`).
    pub fn is_present(&self, key: &str) -> bool {
        self.0.position(OsStr::new(key)).is_some()
    }

    /// A flag variable (`CI`, `SNP_*`): set, non-empty and neither `0` nor `false`
    /// (ASCII-case-insensitive).
    pub fn is_truthy_flag(&self, key: &str) -> bool {
        self.get_nonempty(key).is_some_and(|v| {
            let b = v.as_encoded_bytes();
            b != b"0" && !b.eq_ignore_ascii_case(b"false")
        })
    }

    /// `envcontext(patch)`: operations apply in order; every `Var` (and [`EnvPatch::prepend_path`])
    /// reads the snapshot *before* the patch (`envcontext.py:33-53`).
    #[must_use]
    pub fn patched(&self, patch: &EnvPatch) -> Self {
        if patch.is_empty() {
            return self.clone();
        }
        let mut inner = (*self.0).clone();
        for (key, op) in &patch.0 {
            let key = OsStr::new(key.as_ref());
            match op {
                Op::Public(EnvOp::Set(v)) => inner.set(key.to_owned(), v.clone()),
                Op::Public(EnvOp::Unset) => inner.unset(key),
                Op::Public(EnvOp::Template(parts)) => {
                    inner.set(key.to_owned(), self.format_parts(parts));
                }
                Op::PrependPath { var, dirs } => {
                    let mut value = OsString::new();
                    for (i, d) in dirs.iter().enumerate() {
                        if i > 0 {
                            value.push(PATHSEP);
                        }
                        value.push(d);
                    }
                    if let Some(old) = self.get_nonempty(var) {
                        if !value.is_empty() {
                            value.push(PATHSEP);
                        }
                        value.push(old);
                    }
                    inner.set(key.to_owned(), value);
                }
            }
        }
        Self(Arc::new(inner))
    }

    /// `envcontext.format_env(parts, before)`.
    fn format_parts(&self, parts: &[Part]) -> OsString {
        let mut out = OsString::new();
        for p in parts {
            match p {
                Part::Lit(s) => out.push(s),
                Part::Var { name, default } => match self.get(name) {
                    Some(v) => out.push(v),
                    None => out.push(default.as_ref()),
                },
            }
        }
        out
    }

    /// A copy without `keys`.
    #[must_use]
    pub fn without(&self, keys: &[&str]) -> Self {
        let mut inner = (*self.0).clone();
        for k in keys {
            inner.unset(OsStr::new(k));
        }
        Self(Arc::new(inner))
    }

    /// The variables in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.0
            .vars
            .iter()
            .map(|(k, v)| (k.as_os_str(), v.as_os_str()))
    }

    /// `xargs._environ_size` (`xargs.py:41-46`): `8 * n + Σ(len(k) + len(v) + 2)` over
    /// `os.environb` (bytes) on POSIX and `os.environ` (characters) on Windows.
    pub fn python_environ_size(&self) -> usize {
        let len = |s: &OsStr| {
            if cfg!(windows) {
                s.to_string_lossy().chars().count()
            } else {
                s.as_encoded_bytes().len()
            }
        };
        self.iter()
            .fold(self.0.vars.len().saturating_mul(8), |acc, (k, v)| {
                acc.saturating_add(len(k))
                    .saturating_add(len(v))
                    .saturating_add(2)
            })
    }

    /// `env_clear()` + `envs(self)`: the child environment is exactly this snapshot (S6).
    pub fn apply_to(&self, cmd: &mut std::process::Command) {
        cmd.env_clear();
        cmd.envs(self.iter());
    }
}

/// One part of a template value (`envcontext.Var` or a literal string).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Part {
    /// A literal.
    Lit(OsString),
    /// `Var(name, default)`: the pre-patch value of `name`, or `default` when unset.
    Var {
        /// The variable read from the pre-patch snapshot.
        name: Cow<'static, str>,
        /// Used when `name` is unset (an empty value is used as is).
        default: Cow<'static, str>,
    },
}

/// One envcontext operation.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum EnvOp {
    /// `(key, value)`.
    Set(OsString),
    /// `(key, UNSET)`.
    Unset,
    /// `(key, (parts…))`.
    Template(Box<[Part]>),
}

/// Internal operations: the public ones plus `prepend_path` (D-SEC-07).
#[derive(Clone, Debug)]
enum Op {
    Public(EnvOp),
    PrependPath {
        var: &'static str,
        dirs: Vec<OsString>,
    },
}

/// An ordered envcontext patch (`PatchesT`), built with the methods below.
#[derive(Clone, Debug, Default)]
pub struct EnvPatch(Vec<(Cow<'static, str>, Op)>);

impl EnvPatch {
    /// `(k, v)`.
    #[must_use]
    pub fn set(mut self, k: impl Into<Cow<'static, str>>, v: impl Into<OsString>) -> Self {
        self.0.push((k.into(), Op::Public(EnvOp::Set(v.into()))));
        self
    }

    /// `(k, UNSET)`.
    #[must_use]
    pub fn unset(mut self, k: impl Into<Cow<'static, str>>) -> Self {
        self.0.push((k.into(), Op::Public(EnvOp::Unset)));
        self
    }

    /// `(key, (d1, PATHSEP, …, dN, PATHSEP, Var(key)))` — except that the trailing separator is
    /// omitted when `key` is unset or empty, so an unset `PATH` never puts the cwd on `PATH`
    /// (D-SEC-07).
    #[must_use]
    pub fn prepend_path(
        mut self,
        key: &'static str,
        dirs: impl IntoIterator<Item = PathBuf>,
    ) -> Self {
        let dirs = dirs.into_iter().map(PathBuf::into_os_string).collect();
        self.0
            .push((Cow::Borrowed(key), Op::PrependPath { var: key, dirs }));
        self
    }

    /// `(k, (parts…))`.
    #[must_use]
    pub fn template(mut self, k: impl Into<Cow<'static, str>>, parts: Vec<Part>) -> Self {
        self.0.push((
            k.into(),
            Op::Public(EnvOp::Template(parts.into_boxed_slice())),
        ));
        self
    }

    /// `self` followed by `other`.
    #[must_use]
    pub fn extend(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    /// Whether the patch has no operation.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(e: &Env) -> Vec<(String, String)> {
        e.iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }
    fn env(p: &[(&str, &str)]) -> Env {
        Env::from_pairs(p.iter().copied())
    }
    fn var(name: &'static str) -> Part {
        Part::Var {
            name: name.into(),
            default: "".into(),
        }
    }

    // T10-UTL-06: envcontext_test.py:13-103
    #[test]
    fn t10_utl_06_envcontext_trivial_and_noop() {
        assert_eq!(
            pairs(&env(&[]).patched(&EnvPatch::default())),
            Vec::<(String, String)>::new()
        );
        assert_eq!(
            pairs(&env(&[("foo", "bar")]).patched(&EnvPatch::default())),
            [("foo".into(), "bar".into())]
        );
    }

    #[test]
    fn t10_utl_06_envcontext_adds_overrides_unsets() {
        assert_eq!(
            pairs(&env(&[]).patched(&EnvPatch::default().set("foo", "bar"))),
            [("foo".into(), "bar".into())]
        );
        assert_eq!(
            pairs(&env(&[("foo", "baz")]).patched(&EnvPatch::default().set("foo", "bar"))),
            [("foo".into(), "bar".into())]
        );
        assert_eq!(
            pairs(&env(&[]).patched(&EnvPatch::default().unset("foo"))).len(),
            0
        );
        assert_eq!(
            pairs(&env(&[("PYTHONHOME", "")]).patched(&EnvPatch::default().unset("PYTHONHOME")))
                .len(),
            0
        );
    }

    #[test]
    fn t10_utl_06_envcontext_templates() {
        let missing =
            EnvPatch::default().template("PATH", vec![Part::Lit("~/bin:".into()), var("PATH")]);
        assert_eq!(
            pairs(&env(&[]).patched(&missing)),
            [("PATH".into(), "~/bin:".into())]
        );
        let defaulted = EnvPatch::default().template(
            "PATH",
            vec![
                Part::Lit("~/bin:".into()),
                Part::Var {
                    name: "PATH".into(),
                    default: "/bin".into(),
                },
            ],
        );
        assert_eq!(
            pairs(&env(&[]).patched(&defaulted)),
            [("PATH".into(), "~/bin:/bin".into())]
        );
        assert_eq!(
            pairs(&env(&[("PATH", "/usr/local/bin:/usr/bin")]).patched(&missing)),
            [("PATH".into(), "~/bin:/usr/local/bin:/usr/bin".into())]
        );
    }

    #[test]
    fn t10_utl_06_envcontext_sources_from_previous() {
        let p = EnvPatch::default()
            .set("foo", "baz")
            .template("herp", vec![Part::Lit("foo: ".into()), var("foo")]);
        assert_eq!(
            pairs(&env(&[("foo", "bar")]).patched(&p)),
            [
                ("foo".into(), "baz".into()),
                ("herp".into(), "foo: bar".into())
            ]
        );
    }

    #[test]
    fn t10_utl_06_envcontext_never_mutates_the_snapshot() {
        let before = env(&[("hello", "world")]);
        let after = before.patched(&EnvPatch::default().set("foo", "bar"));
        assert_eq!(pairs(&before), [("hello".into(), "world".into())]);
        assert_eq!(
            pairs(&after),
            [
                ("hello".into(), "world".into()),
                ("foo".into(), "bar".into())
            ]
        );
    }

    #[test]
    fn prepend_path_without_trailing_separator() {
        let p = EnvPatch::default()
            .prepend_path("PATH", [PathBuf::from("/venv/bin"), PathBuf::from("/x")]);
        let sep = PATHSEP;
        assert_eq!(
            env(&[]).patched(&p).get("PATH"),
            Some(OsStr::new(&format!("/venv/bin{sep}/x")))
        );
        assert_eq!(
            env(&[("PATH", "")]).patched(&p).get("PATH"),
            Some(OsStr::new(&format!("/venv/bin{sep}/x")))
        );
        assert_eq!(
            env(&[("PATH", "/usr/bin")]).patched(&p).get("PATH"),
            Some(OsStr::new(&format!("/venv/bin{sep}/x{sep}/usr/bin")))
        );
        // every Var reads the pre-patch snapshot, also for prepend_path
        let p2 = EnvPatch::default()
            .set("PATH", "/changed")
            .prepend_path("PATH", [PathBuf::from("/b")]);
        assert_eq!(
            env(&[("PATH", "/orig")]).patched(&p2).get("PATH"),
            Some(OsStr::new(&format!("/b{sep}/orig")))
        );
    }

    #[test]
    fn lookups() {
        let e = env(&[("A", ""), ("CI", "true"), ("Z", "0"), ("F", "False")]);
        assert_eq!(e.get("A"), Some(OsStr::new("")));
        assert_eq!(e.get_nonempty("A"), None);
        assert!(e.is_present("A") && !e.is_present("B"));
        assert!(e.is_truthy_flag("CI"));
        assert!(
            !e.is_truthy_flag("A")
                && !e.is_truthy_flag("Z")
                && !e.is_truthy_flag("F")
                && !e.is_truthy_flag("B")
        );
        let w = e.without(&["CI", "nope"]);
        assert!(!w.is_present("CI") && e.is_present("CI"));
        assert_eq!(w.iter().count(), 3);
        let dup = env(&[("K", "1"), ("X", "2"), ("K", "3")]);
        assert_eq!(
            pairs(&dup),
            [("K".into(), "3".into()), ("X".into(), "2".into())]
        );
        assert_eq!(
            format!("{e:?}"),
            "Env({\"A\": <0 bytes>, \"CI\": <4 bytes>, \"Z\": <1 bytes>, \"F\": <5 bytes>})"
        );
    }

    #[test]
    fn windows_keys_are_case_insensitive_first_spelling_kept() {
        let e = env(&[("Path", "a")]).patched(&EnvPatch::default().set("PATH", "b"));
        if cfg!(windows) {
            assert_eq!(pairs(&e), [("Path".into(), "b".into())]);
            assert_eq!(e.get("path"), Some(OsStr::new("b")));
        } else {
            assert_eq!(
                pairs(&e),
                [("Path".into(), "a".into()), ("PATH".into(), "b".into())]
            );
            assert_eq!(e.get("path"), None);
        }
    }

    #[test]
    fn python_environ_size_formula() {
        assert_eq!(env(&[]).python_environ_size(), 0);
        // 8 * 2 + (1 + 3 + 2) + (4 + 0 + 2)
        assert_eq!(env(&[("A", "abc"), ("PATH", "")]).python_environ_size(), 28);
        if !cfg!(windows) {
            assert_eq!(env(&[("é", "ü")]).python_environ_size(), 8 + 2 + 2 + 2);
        }
    }

    #[test]
    fn capture_drops_python_launcher_vars() {
        let e = Env::capture_process();
        assert!(!e.is_present("__PYVENV_LAUNCHER__") && !e.is_present("PYTHONEXECUTABLE"));
    }

    #[test]
    fn apply_to_clears() {
        let mut cmd = std::process::Command::new("x");
        env(&[("ONLY", "1")]).apply_to(&mut cmd);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert_eq!(envs, [(OsStr::new("ONLY"), Some(OsStr::new("1")))]);
    }
}
