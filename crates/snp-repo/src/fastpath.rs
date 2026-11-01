//! The pre-commit-hooks fast path: detection tiers and the per-hook gates G1–G6 (09 §6.2–§6.4, ARCHITECTURE §7.2,
//! ADR-031, ADR-070).
//!
//! * **Tier A** — `repo` names `github.com/pre-commit/pre-commit-hooks` and `rev` is a known tag or the full commit
//!   of one: the embedded per-tag manifest and epoch profile are used and nothing is cloned unless a hook delegates.
//! * **Tier B** — any other cloned repository whose hook `(entry, language)` is one of upstream's: the module the
//!   console script runs and its import closure must all have verified blob ids (`git ls-tree`), and the clone's
//!   `setup.cfg` must map the console script to that module; the manifest comes from the clone.
//!
//! The gates are pure data except G5, which is the [`crate::Preflight`] seam (snp-run's impl over
//! `snp_hooks::native`).
//!
//! Owner: WP-31.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use snp_config::ext::FastPath;
use snp_config::overlay::HookPatch;
use snp_config::{Config, ConfigHook};
use snp_core::failure::Failure;
use snp_core::lang::LanguageId;
use snp_hooks_data::{Epoch, Gate, HookClass, NativeSource, TagProfile};

use crate::{FastPathPolicy, Preflight};

/// Every pre-commit-hooks tag the embedded data covers (09 §5.1; `snp_hooks_data::GENERATED_FROM`), oldest first.
pub(crate) const TAGS: [&str; 16] = [
    "v3.0.0", "v3.0.1", "v3.1.0", "v3.2.0", "v3.3.0", "v3.4.0", "v4.0.0", "v4.0.1", "v4.1.0",
    "v4.2.0", "v4.3.0", "v4.4.0", "v4.5.0", "v4.6.0", "v5.0.0", "v6.0.0",
];

/// The modules live in this package directory of the repository.
const PACKAGE_DIR: &str = "pre_commit_hooks";

/// The effective G1 inputs: `cx.fast_path` (CLI flag / `SNP_NO_FAST_PATH` / root keys as snp-run built it) combined
/// with the config's own root keys, so either source can only narrow the fast path.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    /// The narrower of the two modes.
    pub(crate) mode: FastPath,
    /// `fast_path_exclude` of both sources.
    pub(crate) exclude: Vec<Arc<str>>,
    /// `install-hooks --with-fallback`.
    pub(crate) with_fallback: bool,
}

impl Policy {
    /// The policy for `cfg` under `p`.
    pub(crate) fn new(p: &FastPathPolicy, cfg: &Config) -> Self {
        let rank = |m: FastPath| match m {
            FastPath::Off => 0u8,
            FastPath::Exact => 1,
            _ => 2,
        };
        let mode = if rank(cfg.root_ext.fast_path) < rank(p.mode) {
            cfg.root_ext.fast_path
        } else {
            p.mode
        };
        let mut exclude = p.exclude.clone();
        for e in &cfg.root_ext.fast_path_exclude {
            if !exclude.contains(e) {
                exclude.push(Arc::clone(e));
            }
        }
        Self {
            mode,
            exclude,
            with_fallback: p.with_fallback,
        }
    }

    /// G1 for the whole configuration.
    pub(crate) fn enabled(&self) -> bool {
        self.mode != FastPath::Off
    }
}

/// A hook the fast path could run natively: its upstream identity and epoch.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// The upstream hook id the native implementation is registered under.
    pub(crate) id: &'static str,
    /// Its class at this epoch.
    pub(crate) class: HookClass,
    /// The behaviour epoch.
    pub(crate) epoch: Epoch,
    /// `pre-commit-hooks v6.0.0, tier A` / `verified blobs, tier B`.
    pub(crate) origin: String,
}

/// The outcome of the gates for one hook.
#[derive(Debug, Clone)]
pub(crate) enum Verdict {
    /// Runs natively.
    Native {
        /// Registry id.
        id: Arc<str>,
        /// Epoch.
        epoch: Epoch,
        /// The decision text.
        reason: String,
    },
    /// Runs through its language (the upstream Python hook or a `language: fail` entry).
    Delegate(String),
    /// A `language: fail` manifest entry: no Python and no clone at all (09 §4.F).
    FailLanguage,
}

impl Verdict {
    /// `(decision, reason)` for the decision table.
    pub(crate) fn row(&self) -> (&'static str, String) {
        match self {
            Self::Native { reason, .. } => ("native", reason.clone()),
            Self::Delegate(reason) => ("delegate", reason.clone()),
            Self::FailLanguage => ("native", "language: fail (no environment)".to_owned()),
        }
    }
}

/// Tier A: the embedded profile of `(repo, rev)` when the fast path is on.
pub(crate) fn tier_a(policy: &Policy, repo: &str, rev: &str) -> Option<&'static TagProfile> {
    if !policy.enabled() || !snp_hooks_data::is_pre_commit_hooks_url(repo) {
        return None;
    }
    snp_hooks_data::profile_for_rev(rev)
}

/// The Tier A candidate for `id` in `profile`.
pub(crate) fn tier_a_candidate(profile: &'static TagProfile, id: &str) -> Option<Candidate> {
    profile
        .hooks
        .iter()
        .find(|(hid, ..)| *hid == id)
        .map(|&(hid, class, epoch)| Candidate {
            id: hid,
            class,
            epoch,
            origin: format!("pre-commit-hooks {}, tier A", profile.tag),
        })
}

/// G4 (09 §6.4): the user's hook config or the overlay patch sets `language`, `entry`, a non-empty
/// `additional_dependencies` or `interactive: true` (an interactive hook needs a real process, ADR-085); the offending
/// key.
fn user_override(c: Option<&ConfigHook>, patch: Option<&HookPatch>) -> Option<String> {
    let mut keys: Vec<(String, bool)> = Vec::new();
    if let Some(c) = c {
        for (k, v) in &c.raw {
            if let Some(k) = k.as_str() {
                keys.push((k.to_owned(), v.as_array().is_some_and(|a| !a.is_empty())));
            }
        }
    }
    if let Some(p) = patch {
        for (k, v) in &p.raw {
            if let Some(k) = k.as_str() {
                // a non-empty list, or `true`
                let set = v.as_array().is_some_and(|a| !a.is_empty()) || v.as_bool() == Some(true);
                keys.push((k.to_owned(), set));
            }
        }
    }
    // `ConfigHook::raw` holds pre-commit's keys only (its extension keys are parsed into `ext`, and in strict mode
    // `interactive` is an unknown key that changes nothing)
    if c.is_some_and(|c| c.ext.interactive) {
        keys.push(("interactive".to_owned(), true));
    }
    keys.into_iter()
        .find_map(|(k, non_empty)| match k.as_str() {
            "language" | "entry" => Some(k),
            "additional_dependencies" | "interactive" if non_empty => Some(k),
            _ => None,
        })
}

/// G3 (09 §6.4, ADR-031): `auto` = NATIVE (and the removed-hook stubs) plus NATIVE-WITH-CAVEATS hooks whose CI
/// parity gate passed; `exact` = hooks whose gate is exact. A hook whose gate says delegate never runs natively.
fn class_allowed(mode: FastPath, class: HookClass, id: &str) -> bool {
    let gate = snp_hooks_data::parity_gate(id);
    match class {
        HookClass::Native | HookClass::Stub => match mode {
            FastPath::Exact => gate == Gate::Exact,
            _ => gate != Gate::Delegate,
        },
        HookClass::NativeWithCaveats => match mode {
            FastPath::Exact => false,
            _ => matches!(gate, Gate::Exact | Gate::Caveat),
        },
        _ => false,
    }
}

/// Everything the gates read for one hook.
#[derive(Debug)]
pub(crate) struct GateInput<'a> {
    /// The effective policy.
    pub(crate) policy: &'a Policy,
    /// The hook id as configured.
    pub(crate) user_id: &'a str,
    /// The config hook (remote repos).
    pub(crate) config: Option<&'a ConfigHook>,
    /// The overlay patch.
    pub(crate) patch: Option<&'a HookPatch>,
    /// The merged arguments (G5).
    pub(crate) args: &'a [Arc<str>],
    /// G5.
    pub(crate) preflight: &'a dyn Preflight,
}

/// G1–G6 in order (09 §6.4); `cand` is `Err(reason)` when no profile resolved (G2).
pub(crate) fn decide(g: &GateInput<'_>, cand: Result<Candidate, String>) -> Verdict {
    if !g.policy.enabled() || g.policy.exclude.iter().any(|e| &**e == g.user_id) {
        return Verdict::Delegate("disabled by user".to_owned());
    }
    let cand = match cand {
        Ok(c) => c,
        Err(reason) => return Verdict::Delegate(format!("unknown implementation ({reason})")),
    };
    if let Some(key) = user_override(g.config, g.patch) {
        return Verdict::Delegate(format!("user override (`{key}`)"));
    }
    match cand.class {
        HookClass::FailLanguage => return Verdict::FailLanguage,
        HookClass::Delegate => {
            return Verdict::Delegate(if cand.epoch == Epoch(0) {
                format!(
                    "{}: delegated upstream implementation ({})",
                    cand.id, cand.origin
                )
            } else {
                format!("{}: delegated hook ({})", cand.id, cand.origin)
            });
        }
        _ => {}
    }
    if cand.epoch == Epoch(0) {
        return Verdict::Delegate(format!("old version ({})", cand.origin));
    }
    if !class_allowed(g.policy.mode, cand.class, cand.id) {
        return Verdict::Delegate(format!(
            "mode `{}` does not allow this hook",
            mode_name(g.policy.mode)
        ));
    }
    if let Err(why) = g
        .preflight
        .check(NativeSource::PreCommitHooks, cand.id, cand.epoch, g.args)
    {
        return Verdict::Delegate(format!("unsupported argument: {why}"));
    }
    Verdict::Native {
        id: Arc::from(cand.id),
        epoch: cand.epoch,
        reason: format!("{}, epoch {}", cand.origin, cand.epoch.0),
    }
}

/// The config spelling of a mode.
const fn mode_name(m: FastPath) -> &'static str {
    match m {
        FastPath::Off => "off",
        FastPath::Exact => "exact",
        _ => "auto",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tier B
// ---------------------------------------------------------------------------------------------------------------

/// One upstream manifest entry: `(entry, language)` → the hook id it is published under and its module.
#[derive(Debug)]
struct Known {
    /// `entry` verbatim.
    entry: String,
    /// `language`.
    language: LanguageId,
    /// The upstream id.
    id: &'static str,
    /// The module of the entry's console script.
    module: &'static str,
}

/// The `(entry, language)` pairs of every embedded upstream manifest (09 §6.3 step 1), built once.
fn known_entries() -> &'static [Known] {
    static KNOWN: OnceLock<Vec<Known>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        let mut out: Vec<Known> = Vec::new();
        for tag in TAGS {
            let Some(p) = snp_hooks_data::profile_for_rev(tag) else {
                continue;
            };
            for (id, entry, language) in scan_manifest_text(p.manifest_yaml) {
                let Some(language) = LanguageId::from_config(&language) else {
                    continue;
                };
                let Some(&(hid, ..)) = p.hooks.iter().find(|h| h.0 == id) else {
                    continue;
                };
                let script = entry.split_whitespace().next().unwrap_or_default();
                let Some(module) = snp_hooks_data::module_for_entry(script) else {
                    continue;
                };
                if !out
                    .iter()
                    .any(|k| k.entry == entry && k.language == language)
                {
                    out.push(Known {
                        entry,
                        language,
                        id: hid,
                        module,
                    });
                }
            }
        }
        out
    })
}

/// `(id, entry, language)` of the hooks of an embedded, generated manifest (a fixed `-   id:` / `    key: value`
/// layout with plain or quoted scalars).
fn scan_manifest_text(yaml: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let mut cur: Option<(String, String, String)> = None;
    for line in yaml.lines() {
        if let Some(id) = line.strip_prefix("-   id:") {
            if let Some(h) = cur.take() {
                out.push(h);
            }
            cur = Some((unquote(id), String::new(), String::new()));
        } else if let Some(h) = cur.as_mut() {
            if let Some(e) = line.strip_prefix("    entry:") {
                h.1 = unquote(e);
            } else if let Some(l) = line.strip_prefix("    language:") {
                h.2 = unquote(l);
            }
        }
    }
    out.extend(cur);
    out
}

/// A plain, single- or double-quoted scalar.
fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return inner.replace("''", "'");
    }
    if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    v.to_owned()
}

/// Whether a merged hook could be a Tier B candidate (no git call is made for repositories without one).
pub(crate) fn tier_b_possible(entry: &str, language: LanguageId) -> bool {
    known_entries()
        .iter()
        .any(|k| k.entry == entry && k.language == language)
}

/// What Tier B reads from a clone (09 §6.3 steps 2–3): the blobs of `pre_commit_hooks/` and the console scripts
/// of `setup.cfg`. Cached next to the clone (immutable: a checkout detached at `rev`).
#[derive(Debug, Clone, Default)]
pub(crate) struct CloneFacts {
    /// `(path, blob id)` of `pre_commit_hooks/*`.
    blobs: Vec<(String, String)>,
    /// `(console script, target)` of `setup.cfg` `[options.entry_points] console_scripts`.
    scripts: Vec<(String, String)>,
}

/// What [`CloneFacts::load`] reads on a miss: the `(path, blob id)` pairs of `pre_commit_hooks/` and `setup.cfg`.
pub(crate) type TreeFacts = (Vec<(String, String)>, Option<Vec<u8>>);

/// The cache file inside the repository's `envs/` directory (environment names never start with `.`).
const FACTS_FILE: &str = ".snp-tier-b";

impl CloneFacts {
    /// Reads the facts of `src` (cached in `envs`), spawning `git ls-tree` and `git cat-file` on a miss.
    pub(crate) fn load(
        envs: &Path,
        read: &mut dyn FnMut() -> Result<TreeFacts, Failure>,
    ) -> Result<Self, Failure> {
        let cache = envs.join(FACTS_FILE);
        if let Ok(bytes) = snp_os::fs::read_capped(&cache, 1 << 20)
            && let Some(f) = Self::decode(&bytes)
        {
            return Ok(f);
        }
        let (blobs, setup_cfg) = read()?;
        let facts = Self {
            blobs,
            scripts: setup_cfg
                .as_deref()
                .map(console_scripts)
                .unwrap_or_default(),
        };
        // Best effort: a read-only store recomputes next time.
        let _cached: std::io::Result<()> =
            snp_os::fs::atomic_replace(&cache, &facts.encode(), None);
        Ok(facts)
    }

    /// `v1\n` then `b <path> <sha>` / `s <name> <target>` lines.
    fn encode(&self) -> Vec<u8> {
        let mut s = String::from("v1\n");
        for (p, sha) in &self.blobs {
            s.push_str("b ");
            s.push_str(p);
            s.push(' ');
            s.push_str(sha);
            s.push('\n');
        }
        for (n, t) in &self.scripts {
            s.push_str("s ");
            s.push_str(n);
            s.push(' ');
            s.push_str(t);
            s.push('\n');
        }
        s.into_bytes()
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.lines();
        if lines.next()? != "v1" {
            return None;
        }
        let mut f = Self::default();
        for line in lines {
            let (kind, rest) = line.split_once(' ')?;
            let (a, b) = rest.rsplit_once(' ')?;
            match kind {
                "b" => f.blobs.push((a.to_owned(), b.to_owned())),
                "s" => f.scripts.push((a.to_owned(), b.to_owned())),
                _ => return None,
            }
        }
        Some(f)
    }

    /// The blob id of `pre_commit_hooks/<module>.py`.
    fn blob(&self, module: &str) -> Option<&str> {
        let path = format!("{PACKAGE_DIR}/{module}.py");
        self.blobs
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, sha)| sha.as_str())
    }

    /// Tier B for one merged hook (09 §6.3 steps 1–4).
    pub(crate) fn candidate(&self, entry: &str, language: LanguageId) -> Result<Candidate, String> {
        let known = known_entries()
            .iter()
            .find(|k| k.entry == entry && k.language == language)
            .ok_or_else(|| "not an upstream pre-commit-hooks entry".to_owned())?;
        let script = entry.split_whitespace().next().unwrap_or_default();
        let target = format!("{PACKAGE_DIR}.{}:main", known.module);
        if !self
            .scripts
            .iter()
            .any(|(n, t)| n == script && *t == target)
        {
            return Err(format!("setup.cfg does not map `{script}` to `{target}`"));
        }
        let epoch = verified(self.blob(known.module), known.module)?;
        for dep in snp_hooks_data::import_closure(known.module) {
            verified(self.blob(dep), dep)?;
        }
        let class = class_at(known.id, epoch);
        Ok(Candidate {
            id: known.id,
            class,
            epoch,
            origin: format!("verified blobs of `{}`, tier B", known.module),
        })
    }
}

/// The epoch of a verified blob of `module`.
fn verified(blob: Option<&str>, module: &str) -> Result<Epoch, String> {
    let differs = || format!("implementation differs from verified upstream blobs (`{module}`)");
    let blob = blob.ok_or_else(differs)?;
    match snp_hooks_data::verified_blob(blob) {
        Some((m, epoch)) if m == module => Ok(epoch),
        _ => Err(differs()),
    }
}

/// The class of upstream hook `id` at `epoch`, from the embedded profiles.
fn class_at(id: &str, epoch: Epoch) -> HookClass {
    if epoch == Epoch(0) {
        return HookClass::Delegate;
    }
    for tag in TAGS.iter().rev() {
        if let Some(p) = snp_hooks_data::profile_for_rev(tag)
            && let Some(&(_, class, e)) = p.hooks.iter().find(|h| h.0 == id)
            && e == epoch
        {
            return class;
        }
    }
    HookClass::Delegate
}

/// `setup.cfg` `console_scripts` entries: `name = target` lines below `console_scripts =`.
fn console_scripts(cfg: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(cfg);
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let body = line.trim();
        if body.starts_with('[') {
            inside = false;
            continue;
        }
        if let Some((k, v)) = body.split_once('=') {
            let indented = line.starts_with([' ', '\t']);
            if !indented {
                inside = k.trim() == "console_scripts" && v.trim().is_empty();
                continue;
            }
            if inside {
                out.push((k.trim().to_owned(), v.trim().to_owned()));
            }
        } else if !line.starts_with([' ', '\t']) && !body.is_empty() {
            inside = false;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tag_resolves_and_every_profile_hook_is_scanned() {
        for tag in TAGS {
            let p = snp_hooks_data::profile_for_rev(tag).unwrap_or_else(|| panic!("{tag}"));
            let scanned = scan_manifest_text(p.manifest_yaml);
            assert_eq!(scanned.len(), p.hooks.len(), "{tag}");
            for (id, entry, language) in &scanned {
                assert!(p.hooks.iter().any(|h| h.0 == id), "{tag} {id}");
                assert!(
                    !entry.is_empty() && LanguageId::from_config(language).is_some(),
                    "{tag} {id}"
                );
            }
        }
        assert!(snp_hooks_data::GENERATED_FROM.contains("v3.0.0..v6.0.0"));
    }

    #[test]
    fn known_entries_cover_console_scripts() {
        assert!(tier_b_possible(
            "trailing-whitespace-fixer",
            LanguageId::Python
        ));
        assert!(!tier_b_possible(
            "trailing-whitespace-fixer",
            LanguageId::Node
        ));
        assert!(!tier_b_possible("my-own-script", LanguageId::Python));
        let k = known_entries()
            .iter()
            .find(|k| k.entry == "check-json")
            .unwrap();
        assert_eq!((k.id, k.module), ("check-json", "check_json"));
    }

    #[test]
    fn setup_cfg_console_scripts() {
        let cfg = b"[metadata]\nname = pre_commit_hooks\n\n[options.entry_points]\nconsole_scripts =\n    check-json = pre_commit_hooks.check_json:main\n    trailing-whitespace-fixer = pre_commit_hooks.trailing_whitespace_fixer:main\n\n[bdist_wheel]\nuniversal = True\n";
        assert_eq!(
            console_scripts(cfg),
            [
                (
                    "check-json".to_owned(),
                    "pre_commit_hooks.check_json:main".to_owned()
                ),
                (
                    "trailing-whitespace-fixer".to_owned(),
                    "pre_commit_hooks.trailing_whitespace_fixer:main".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn facts_round_trip() {
        let f = CloneFacts {
            blobs: vec![(
                "pre_commit_hooks/a b.py".to_owned(),
                "0000000000000000000000000000000000000000".to_owned(),
            )],
            scripts: vec![("x".to_owned(), "pre_commit_hooks.x:main".to_owned())],
        };
        let back = CloneFacts::decode(&f.encode()).unwrap();
        assert_eq!(back.blobs, f.blobs);
        assert_eq!(back.scripts, f.scripts);
        assert!(CloneFacts::decode(b"v2\n").is_none());
    }

    #[test]
    fn classes_by_epoch() {
        assert_eq!(class_at("check-json", Epoch(2)), HookClass::Native);
        assert_eq!(
            class_at("check-yaml", Epoch(2)),
            HookClass::NativeWithCaveats
        );
        assert_eq!(class_at("check-ast", Epoch(0)), HookClass::Delegate);
    }
}
