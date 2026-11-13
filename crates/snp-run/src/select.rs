//! Hook selection (`run.py:425-443`), `SKIP`, and the snp selection extensions: hook groups (ADR-080),
//! `SNP_SKIP_GROUPS`, and the `skip`/`only` conditions (ARCHITECTURE §8.2).
//!
//! Owner: WP-32.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use snp_config::ext::Condition;
use snp_core::cancel::CancelToken;
use snp_core::env::{Env, EnvPatch};
use snp_core::stage::Stage;
use snp_git::{Git, RepoInfo};
use snp_proc::spawn::{self, Group, Role, Spec, Stdin};
use snp_pyre::{Options, RegexCache};
use snp_repo::ResolvedHook;

/// The selector matching hooks without groups (ADR-080).
pub(crate) const UNGROUPED: &str = "@ungrouped";

/// `--group` (OR), `--require-group` (AND), `--no-group` (exclusion wins), each allowing `@ungrouped`.
#[derive(Debug, Default, Clone)]
pub(crate) struct GroupFilter {
    any: Vec<String>,
    all: Vec<String>,
    none: Vec<String>,
}

impl GroupFilter {
    /// From the CLI values (non-UTF-8 names never match a group, which must be valid YAML strings).
    pub(crate) fn new(any: &[OsString], all: &[OsString], none: &[OsString]) -> Self {
        let conv = |v: &[OsString]| -> Vec<String> {
            v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
        };
        Self {
            any: conv(any),
            all: conv(all),
            none: conv(none),
        }
    }

    /// The first `--group`/`--require-group` name that no hook of `hooks` carries (`@ungrouped` counts as carried
    /// by the hooks without groups), and every group name the hooks carry, sorted (ADR-084).
    pub(crate) fn unknown<'h>(&self, hooks: &'h [ResolvedHook]) -> Option<(&str, Vec<&'h str>)> {
        let carried = |g: &str| {
            hooks.iter().any(|h| {
                if g == UNGROUPED {
                    h.def.ext.groups.is_empty()
                } else {
                    h.def.ext.groups.iter().any(|x| &**x == g)
                }
            })
        };
        let missing = self.any.iter().chain(&self.all).find(|g| !carried(g))?;
        let mut known: Vec<&str> = hooks
            .iter()
            .flat_map(|h| h.def.ext.groups.iter().map(|g| &**g))
            .collect();
        known.sort_unstable();
        known.dedup();
        Some((missing.as_str(), known))
    }

    /// The truth table of ADR-080.
    pub(crate) fn matches(&self, groups: &[Arc<str>]) -> bool {
        let has = |g: &String| -> bool {
            if g == UNGROUPED {
                groups.is_empty()
            } else {
                groups.iter().any(|h| **h == **g)
            }
        };
        if self.none.iter().any(has) {
            return false;
        }
        if !self.any.is_empty() && !self.any.iter().any(has) {
            return false;
        }
        self.all.iter().all(has)
    }
}

/// `h.id == hook or h.alias == hook` (`run.py:427`): literal comparison, so ids such as `lint:js` keep working
/// (ADR-069).
pub(crate) fn id_matches(h: &ResolvedHook, sel: &OsStr) -> bool {
    OsStr::new(&*h.def.id) == sel || (!h.def.alias.is_empty() && OsStr::new(&*h.def.alias) == sel)
}

/// The selected hooks, in config order, or the first selector that matched nothing (`No hook with id …`).
///
/// `[h for h in hooks if (not args.hook or h.id == args.hook or h.alias == args.hook) if args.hook_stage in
/// h.stages]`; the snp brand accepts several selectors (their union) and the group filter.
pub(crate) fn select<'h>(
    hooks: &'h [ResolvedHook],
    selectors: &[OsString],
    stage: Stage,
    groups: &GroupFilter,
) -> Result<Vec<&'h ResolvedHook>, OsString> {
    let eligible =
        |h: &ResolvedHook| h.def.stages.contains(stage) && groups.matches(&h.def.ext.groups);
    if let Some(missing) = selectors
        .iter()
        .find(|s| !hooks.iter().any(|h| id_matches(h, s) && eligible(h)))
    {
        return Err(missing.clone());
    }
    Ok(hooks
        .iter()
        .filter(|h| selectors.is_empty() || selectors.iter().any(|s| id_matches(h, s)))
        .filter(|h| eligible(h))
        .collect())
}

/// `SKIP` (`run._get_skips`, `strip_fsbytes` semantics) and `SNP_SKIP_GROUPS`.
#[derive(Debug, Default)]
pub(crate) struct Skips {
    ids: Vec<OsString>,
    groups: Vec<String>,
}

impl Skips {
    pub(crate) fn from_env(env: &Env, strict: bool) -> Self {
        let ids = snp_pycompat::precommit::get_skips(env.get("SKIP"));
        let groups = if strict {
            Vec::new()
        } else {
            env.get("SNP_SKIP_GROUPS").map_or_default(|v| {
                v.to_string_lossy()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
        };
        Self { ids, groups }
    }

    /// `hook.id in skips or hook.alias in skips` (`run.py:150`) or a group in `SNP_SKIP_GROUPS`.
    pub(crate) fn skips(&self, h: &ResolvedHook) -> bool {
        let by_id = self
            .ids
            .iter()
            .any(|s| s == OsStr::new(&*h.def.id) || s == OsStr::new(&*h.def.alias));
        let by_group = self.groups.iter().any(|g| {
            if g == UNGROUPED {
                h.def.ext.groups.is_empty()
            } else {
                h.def.ext.groups.iter().any(|x| **x == **g)
            }
        });
        by_id || by_group
    }
}

/// Lazily evaluated `skip`/`only` conditions (ARCHITECTURE §8.2), cached per run.
#[derive(Debug)]
pub(crate) struct Conditions<'a> {
    git: &'a Git,
    info: &'a RepoInfo,
    env: &'a Env,
    cancel: &'a CancelToken,
    branch: OnceLock<Option<String>>,
    merge_commit: OnceLock<bool>,
    runs: Mutex<HashMap<RunKey, bool>>,
    regexes: &'a RegexCache,
}

/// A `{run: cmd}` cache key: the command and the hook's `env:`.
type RunKey = (String, Vec<(Arc<str>, Arc<str>)>);

/// `{run: cmd}` timeout.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

impl<'a> Conditions<'a> {
    pub(crate) fn new(
        git: &'a Git,
        info: &'a RepoInfo,
        env: &'a Env,
        cancel: &'a CancelToken,
        regexes: &'a RegexCache,
    ) -> Self {
        Self {
            git,
            info,
            env,
            cancel,
            branch: OnceLock::new(),
            merge_commit: OnceLock::new(),
            runs: Mutex::new(HashMap::new()),
            regexes,
        }
    }

    /// `skip` holds or `only` is given and fails, for a hook with extension `env`.
    pub(crate) fn skipped(
        &self,
        skip: &[Condition],
        only: &[Condition],
        hook_env: &[(Arc<str>, Arc<str>)],
    ) -> bool {
        skip.iter().any(|c| self.holds(c, hook_env))
            || (!only.is_empty() && !only.iter().any(|c| self.holds(c, hook_env)))
    }

    fn git_dir_has(&self, name: &str) -> bool {
        self.info.git_dir.join(name).exists()
    }

    fn holds(&self, c: &Condition, hook_env: &[(Arc<str>, Arc<str>)]) -> bool {
        match c {
            Condition::Merge => self.git_dir_has("MERGE_HEAD"),
            Condition::Rebase => {
                self.git_dir_has("rebase-merge") || self.git_dir_has("rebase-apply")
            }
            Condition::MergeCommit => *self.merge_commit.get_or_init(|| {
                self.git_dir_has("MERGE_HEAD")
                    || self.git_ok(&["rev-parse", "-q", "--verify", "HEAD^2"])
            }),
            Condition::CherryPick => self.git_dir_has("CHERRY_PICK_HEAD"),
            Condition::Revert => self.git_dir_has("REVERT_HEAD"),
            Condition::Bisect => self.git_dir_has("BISECT_LOG"),
            Condition::Ref(glob) => self
                .branch()
                .is_some_and(|b| glob_matches(glob, b, self.regexes)),
            Condition::Env(name) => self.env.get_nonempty(name).is_some(),
            Condition::Ci(want) => self.env.is_truthy_flag("CI") == *want,
            Condition::Run(cmd) => self.run_ok(cmd, hook_env),
            Condition::All(all) => all.iter().all(|c| self.holds(c, hook_env)),
            _ => false,
        }
    }

    fn git_ok(&self, args: &[&str]) -> bool {
        let a: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.git.run_raw(&a).is_ok_and(|c| c.status.0 == 0)
    }

    /// The short name of the current branch (`git symbolic-ref --short HEAD`); `None` when detached.
    fn branch(&self) -> Option<&str> {
        self.branch
            .get_or_init(|| {
                let a = [
                    OsStr::new("symbolic-ref"),
                    OsStr::new("--short"),
                    OsStr::new("-q"),
                    OsStr::new("HEAD"),
                ];
                let c = self.git.run_raw(&a).ok()?;
                (c.status.0 == 0).then(|| String::from_utf8_lossy(&c.stdout).trim().to_owned())
            })
            .as_deref()
    }

    /// `sh -c <cmd>` in the repo root exits 0 (30 s, cached per run and env).
    fn run_ok(&self, cmd: &str, hook_env: &[(Arc<str>, Arc<str>)]) -> bool {
        let key = (cmd.to_owned(), hook_env.to_vec());
        if let Some(v) = snp_core::sync::lock(&self.runs).get(&key) {
            return *v;
        }
        let mut patch = EnvPatch::default();
        for (k, v) in hook_env {
            patch = patch.set(k.to_string(), OsString::from(&**v));
        }
        let env = self.env.patched(&patch);
        let ok = run_sh(cmd, &env, self.git.root(), self.cancel);
        snp_core::sync::lock(&self.runs).insert(key, ok);
        ok
    }
}

/// `sh -c cmd` with the 30 s deadline; any spawn failure is "false".
fn run_sh(cmd: &str, env: &Env, cwd: &Path, cancel: &CancelToken) -> bool {
    let Ok(argv) = snp_proc::resolve::normalize_cmd(
        &[
            OsString::from("sh"),
            OsString::from("-c"),
            OsString::from(cmd),
        ],
        env,
        cwd,
    ) else {
        return false;
    };
    let spec = Spec {
        argv: &argv,
        env,
        cwd,
        stdin: Stdin::Null,
        group: Group::Isolated,
        role: Role::Tool,
        deadline: Instant::now().checked_add(RUN_TIMEOUT),
        cancel,
    };
    spawn::capture(&spec).is_ok_and(|c| c.status.0 == 0)
}

/// A gobwas-style glob where `*` also crosses `/` (COMPATIBILITY §7.1 `{ref: glob}`), translated to a regex.
fn glob_matches(glob: &str, s: &str, regexes: &RegexCache) -> bool {
    let mut re = String::from("^(?:");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                while chars.peek() == Some(&'*') {
                    chars.next();
                }
                re.push_str(".*");
            }
            '?' => re.push('.'),
            '[' => {
                re.push('[');
                if chars.peek() == Some(&'!') {
                    chars.next();
                    re.push('^');
                }
                for d in chars.by_ref() {
                    if d == ']' {
                        break;
                    }
                    if d == '\\' {
                        re.push_str("\\\\");
                    } else {
                        re.push(d);
                    }
                }
                re.push(']');
            }
            '{' => re.push_str("(?:"),
            '}' => re.push(')'),
            ',' => re.push('|'),
            c if c.is_ascii_alphanumeric() || c == '/' || c == '_' || c == '-' => re.push(c),
            c => {
                re.push('\\');
                re.push(c);
            }
        }
    }
    re.push_str(")\\Z");
    regexes
        .get(&re, Options::CONFIG)
        .is_ok_and(|r| r.search(s).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(v: &[&str]) -> Vec<Arc<str>> {
        v.iter().map(|s| Arc::from(*s)).collect()
    }

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn group_truth_table() {
        let f = GroupFilter::new(&os(&["a", "b"]), &[], &[]);
        assert!(f.matches(&groups(&["a"])));
        assert!(f.matches(&groups(&["b", "c"])));
        assert!(!f.matches(&groups(&["c"])));
        assert!(!f.matches(&groups(&[])));

        let f = GroupFilter::new(&[], &os(&["a", "b"]), &[]);
        assert!(f.matches(&groups(&["a", "b", "c"])));
        assert!(!f.matches(&groups(&["a"])));

        let f = GroupFilter::new(&os(&["a"]), &[], &os(&["slow"]));
        assert!(f.matches(&groups(&["a"])));
        assert!(!f.matches(&groups(&["a", "slow"])));

        let f = GroupFilter::new(&os(&[UNGROUPED]), &[], &[]);
        assert!(f.matches(&groups(&[])));
        assert!(!f.matches(&groups(&["a"])));

        let f = GroupFilter::new(&[], &[], &os(&[UNGROUPED]));
        assert!(!f.matches(&groups(&[])));
        assert!(f.matches(&groups(&["x"])));

        assert!(GroupFilter::default().matches(&groups(&[])));
    }

    #[test]
    fn globs_cross_slashes() {
        let c = RegexCache::default();
        assert!(glob_matches("feature/*", "feature/a/b", &c));
        assert!(glob_matches("main", "main", &c));
        assert!(!glob_matches("main", "main2", &c));
        assert!(glob_matches("{main,dev}", "dev", &c));
        assert!(glob_matches("rel-?.x", "rel-1.x", &c));
        assert!(!glob_matches("rel-?.x", "rel-1yx", &c));
        assert!(glob_matches("v[0-9]*", "v1.2", &c));
    }

    #[test]
    fn t10_run_04_skip_parsing() {
        let skips = |v: Option<&str>| {
            let env = match v {
                Some(s) => Env::from_pairs([("SKIP", s)]),
                None => Env::from_pairs::<&str, &str>([]),
            };
            Skips::from_env(&env, false).ids
        };
        let none: Vec<OsString> = Vec::new();
        assert_eq!(skips(None), none);
        assert_eq!(skips(Some("")), none);
        assert_eq!(skips(Some(",")), none);
        assert_eq!(skips(Some(",foo")), os(&["foo"]));
        assert_eq!(skips(Some("foo,bar")), os(&["foo", "bar"]));
        assert_eq!(skips(Some(" foo , bar")), os(&["foo", "bar"]));
    }
}
