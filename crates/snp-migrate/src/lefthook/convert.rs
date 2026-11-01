//! lefthook hooks → snp / pre-commit hooks (05 §19.3–§19.8): job flattening with scope inheritance (§6.3),
//! priority buckets (§19.4), the `run` → `entry` rules R1–R7 (§19.5), glob and `file_types` translation (§19.6,
//! §19.7), conditions, environment, `root`, and the report notes (§19.10).
//!
//! Owner: WP-34.

use std::fmt::Write as _;
use std::path::Path;

use super::emit::{CondItem, CondOut, OutHook};
use super::glob::{self, Matcher};
use super::load::{AVAILABLE_HOOKS, Loaded};
use super::model::{self, Hook, Job, Origin};
use super::shell::{self, Occ, Tpl};
use super::value::{LValue, weak_string};

/// pre-commit's `HOOK_TYPES` (`clientlib.py:26-37`), in its order.
pub(crate) const HOOK_TYPES: [&str; 10] = [
    "commit-msg",
    "post-checkout",
    "post-commit",
    "post-merge",
    "post-rewrite",
    "pre-commit",
    "pre-merge-commit",
    "pre-push",
    "pre-rebase",
    "prepare-commit-msg",
];

/// A report level (05 §19.10).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Level {
    /// Converted faithfully; something to know.
    Warning,
    /// lefthook behaviour that the output does not reproduce.
    Lossy,
}

/// One report note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Note {
    /// The level.
    pub(crate) level: Level,
    /// `<hook>[/<job>]` or the config file.
    pub(crate) scope: String,
    /// The message.
    pub(crate) msg: String,
}

impl Note {
    /// `snp-migrate: <level>: <scope>: <message>`.
    pub(crate) fn line(&self) -> String {
        let level = match self.level {
            Level::Warning => "warning",
            Level::Lossy => "lossy",
        };
        // One line without control characters: it is printed to a terminal and written into YAML comments.
        super::emit::one_line(&format!(
            "snp-migrate: {level}: {}: {}",
            self.scope, self.msg
        ))
    }
}

/// The conversion of one loaded configuration.
#[derive(Debug, Clone, Default)]
pub(crate) struct Conversion {
    /// The hooks in output order.
    pub(crate) hooks: Vec<OutHook>,
    /// The stage of each hook (parallel to `hooks`).
    pub(crate) stages: Vec<String>,
    /// Root `fail_on_changes: never` (snp mode, every lefthook hook used lefthook's default).
    pub(crate) root_never: bool,
    /// `default_install_hook_types` in pre-commit order.
    pub(crate) install_types: Vec<String>,
    /// Every note in report order.
    pub(crate) notes: Vec<Note>,
    /// Leaf jobs seen.
    pub(crate) jobs: usize,
    /// lefthook hooks seen.
    pub(crate) lefthook_hooks: usize,
}

/// Conversion options.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Opts<'a> {
    /// The repository root.
    pub(crate) root: &'a Path,
    /// `--mode portable`.
    pub(crate) portable: bool,
}

/// The inherited scope of a job (05 §6.3).
#[derive(Debug, Clone, Default)]
struct Scope {
    glob: Vec<String>,
    exclude: Vec<String>,
    file_types: Vec<String>,
    tags: Vec<String>,
    root: String,
    files: String,
    env: Vec<(String, String)>,
    skip: Vec<LValue>,
    only: Vec<LValue>,
    /// Inside a `piped` sequence.
    piped: bool,
}

impl Scope {
    /// The scope below `j` (a group job, or the leaf itself).
    fn with(&self, j: &Job) -> Self {
        let mut s = self.clone();
        s.glob.extend(j.glob.iter().cloned());
        s.exclude.extend(j.exclude.iter().cloned());
        s.file_types.extend(j.file_types.iter().cloned());
        s.tags.extend(j.tags.iter().cloned());
        if !j.root.is_empty() {
            s.root.clone_from(&j.root);
        }
        if !j.files.is_empty() {
            s.files.clone_from(&j.files);
        }
        for (k, v) in &j.env {
            match s.env.iter_mut().find(|(n, _)| n == k) {
                Some(slot) => slot.1.clone_from(v),
                None => s.env.push((k.clone(), v.clone())),
            }
        }
        if let Some(c) = &j.skip {
            s.skip.push(c.clone());
        }
        if let Some(c) = &j.only {
            s.only.push(c.clone());
        }
        s
    }
}

/// A leaf job with its effective scope.
#[derive(Debug, Clone)]
struct Leaf {
    job: Job,
    scope: Scope,
    /// 1-based position among the hook's leaves.
    index: usize,
    /// A lefthook validity failure (the job always fails).
    fail: Option<String>,
}

/// The scheduling tree of 05 §19.4.
#[derive(Debug, Clone)]
enum Node {
    Leaf(usize),
    Seq(Vec<Self>),
    Par(Vec<Self>),
}

/// The per-conversion state.
struct Cx<'a> {
    o: Opts<'a>,
    matcher: Matcher,
    templates: Vec<(String, String)>,
    source_dirs: Vec<String>,
    notes: Vec<Note>,
}

impl Cx<'_> {
    fn note(&mut self, level: Level, scope: &str, msg: impl Into<String>) {
        self.notes.push(Note {
            level,
            scope: scope.to_owned(),
            msg: msg.into(),
        });
    }
}

/// `TrimRight(root, "/")` plus `./` and `//` normalisation; `None` for nothing left.
fn normalize_root(r: &str) -> Option<String> {
    let parts: Vec<&str> = r
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Whether snp's `root` key accepts the (normalized) directory: relative, no `..`, no NUL.
fn root_key_ok(r: &str, original: &str) -> bool {
    !original.starts_with('/')
        && !original.starts_with('\\')
        && !r.contains('\0')
        && !r.split('/').any(|p| p == "..")
}

/// The id slug of 05 §19.3: lowercase ASCII alphanumeric runs joined by `-`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

/// A display name: one line, at most 60 characters plus `…`.
fn display_name(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 60 {
        let mut t: String = one.chars().take(60).collect();
        t.push('…');
        t
    } else {
        one
    }
}

/// The values of lefthook's positional templates in a stage (05 §19.5 table; `None` = no equivalent).
fn positional(stage: &str, n: u32) -> Option<&'static str> {
    Some(match (stage, n) {
        ("commit-msg" | "prepare-commit-msg", 1) => "\"$1\"",
        ("prepare-commit-msg", 2) => "\"$PRE_COMMIT_COMMIT_MSG_SOURCE\"",
        ("prepare-commit-msg", 3) => "\"$PRE_COMMIT_COMMIT_OBJECT_NAME\"",
        ("pre-push", 1) => "\"$PRE_COMMIT_REMOTE_NAME\"",
        ("pre-push", 2) => "\"$PRE_COMMIT_REMOTE_URL\"",
        ("post-checkout", 1) => "\"$PRE_COMMIT_FROM_REF\"",
        ("post-checkout", 2) => "\"$PRE_COMMIT_TO_REF\"",
        ("post-checkout", 3) => "\"$PRE_COMMIT_CHECKOUT_TYPE\"",
        ("post-merge", 1) => "\"$PRE_COMMIT_IS_SQUASH_MERGE\"",
        ("post-rewrite", 1) => "\"$PRE_COMMIT_REWRITE_COMMAND\"",
        ("pre-rebase", 1) => "\"$PRE_COMMIT_PRE_REBASE_UPSTREAM\"",
        ("pre-rebase", 2) => "\"$PRE_COMMIT_PRE_REBASE_BRANCH\"",
        _ => return None,
    })
}

/// How many positional values a stage has (`{0}` joins them).
const fn positional_count(stage: &str) -> u32 {
    match stage.as_bytes() {
        b"prepare-commit-msg" | b"post-checkout" => 3,
        b"pre-push" | b"pre-rebase" => 2,
        b"commit-msg" | b"post-merge" | b"post-rewrite" => 1,
        _ => 0,
    }
}

/// Whether the stage's message file is lefthook's `{1}` (pre-commit passes it as the only filename).
fn msg_file_stage(stage: &str) -> bool {
    matches!(stage, "commit-msg" | "prepare-commit-msg")
}

/// The main file template of a stage (05 §19.5).
fn main_tpl(stage: &str, t: &Tpl) -> bool {
    matches!(
        (stage, t),
        ("pre-commit", Tpl::Staged)
            | ("pre-push", Tpl::Push)
            | ("commit-msg" | "prepare-commit-msg", Tpl::Arg(1))
            | ("commit-msg", Tpl::Arg(0))
    )
}

/// The input of the `run` → `entry` rules.
struct EntryIn<'a> {
    cmd: &'a str,
    stage: &'a str,
    files_cmd: &'a str,
    glob: &'a [String],
    exclude: &'a [String],
    /// The normalized root, when the wrapper must `cd` into it.
    cd_root: Option<&'a str>,
    /// The root the R5 prelude filters and strips (`cd_root` or the snp `root` key).
    list_root: Option<&'a str>,
    /// `NAME="value"; export NAME;` assignments.
    env_prefix: &'a str,
    /// A script job (always runs).
    script: bool,
}

/// The result of the rules.
#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "pre-commit's boolean hook keys"
)]
struct EntryOut {
    entry: String,
    pass_filenames: bool,
    always_run: bool,
    require_serial: bool,
    /// The pre-commit `files`/`types` filters apply (false when a shell computes the list).
    filters_apply: bool,
    /// The entry is a plain command (R1/R2), usable with an overlay's `{entry}`.
    plain: bool,
    /// The job never runs (lefthook skips it every time).
    never: Option<String>,
    notes: Vec<(Level, String)>,
}

/// Sets `core.quotePath=false` for the file-list command through git's environment, unless the caller already
/// passes configuration that way.
const QUOTE_PATH: &str = "[ -n \"${GIT_CONFIG_COUNT-}\" ] || export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.quotePath GIT_CONFIG_VALUE_0=false; ";

/// The `sed` script trimming leading and trailing whitespace of each file-list line (Go's `strings.TrimSpace`).
const TRIM: &str = "'s/^[[:space:]]*//; s/[[:space:]]*$//'";

/// The R5 file-list pipeline: the command's output filtered by glob/exclude/root and lefthook's existence check.
fn list_pipeline(
    cmd: &str,
    i: &EntryIn<'_>,
    matcher: Matcher,
    notes: &mut Vec<(Level, String)>,
) -> String {
    let mut p = String::new();
    if let Some(r) = i.list_root {
        p.push_str(&root_pattern(r).0);
    }
    // lefthook trims each line (`strings.TrimSpace`) and `strconv.Unquote`s git's C-quoted names; with
    // `core.quotePath=false` git leaves non-ASCII names unquoted (names with `"`, `\` or control characters stay
    // quoted and are skipped by the existence check), and `sed` trims the lines.
    write!(p, "( {QUOTE_PATH}{cmd} ) | sed {TRIM}").unwrap_or_default();
    match glob::ere(i.glob, matcher, true) {
        Ok(Some(e)) => write!(p, " | grep -iE {}", shell::sh_quote(&e)).unwrap_or_default(),
        Ok(None) => {}
        Err(e) => notes.push((
            Level::Lossy,
            format!("glob not applied to the shell file list: {}", e.0),
        )),
    }
    match glob::ere(i.exclude, matcher, false) {
        Ok(Some(e)) => write!(p, " | grep -vE {}", shell::sh_quote(&e)).unwrap_or_default(),
        Ok(None) => {}
        Err(e) => notes.push((
            Level::Lossy,
            format!("exclude not applied to the shell file list: {}", e.0),
        )),
    }
    let strip = match i.list_root {
        Some(r) => {
            write!(
                p,
                " | grep -E {}",
                shell::sh_quote(&format!("^{}/", glob::ere_literal(r)))
            )
            .unwrap_or_default();
            format!("f=${{f#{}}}; ", root_pattern(r).1)
        }
        None => String::new(),
    };
    write!(
        p,
        " | while IFS= read -r f; do {strip}[ -f \"$f\" ] && printf '%s\\n' \"$f\"; done"
    )
    .unwrap_or_default();
    p
}

/// A warning when the `files` / `exclude` regex has more than [`glob::SLOW_RUNS`] unbounded runs.
fn slow_regex_note(what: &str, re: Option<&str>, notes: &mut Vec<(Level, String)>) {
    let n = re.map_or(0, glob::unbounded_runs);
    if n > glob::SLOW_RUNS {
        notes.push((
            Level::Warning,
            format!(
                "the {what} regex has {n} unbounded runs; Python's backtracking `re` may be slow on long paths (lefthook's matcher is linear)"
            ),
        ));
    }
}

/// The `<root>/` prefix as the pattern of a `${f#…}` expansion, plus the assignment it needs: a root made of
/// `shlex.quote`-safe characters is written as is (no glob or expansion character can occur); any other root is bound
/// to `snp_r` once and used as `"$snp_r"`, which the shell matches literally (a root is a directory, never code or a
/// pattern).
fn root_pattern(r: &str) -> (String, String) {
    let prefix = format!("{r}/");
    let quoted = shell::sh_quote(&prefix);
    if quoted == prefix {
        (String::new(), prefix)
    } else {
        (format!("snp_r={quoted}; "), "\"$snp_r\"".to_owned())
    }
}

/// 05 §19.5 rules R1–R7.
fn entry_rules(i: &EntryIn<'_>, matcher: Matcher) -> EntryOut {
    let mut out = EntryOut::default();
    let occs = shell::scan(i.cmd);
    let has = |t: &Tpl| occs.iter().any(|o| &o.tpl == t);
    if has(&Tpl::Staged) && has(&Tpl::Push) {
        // lefthook fails the job (`build_command.go`); emitted as a `fail` hook by the caller.
        out.never = Some("one of your runners contains incompatible file types".to_owned());
        return out;
    }
    let is_main = |o: &Occ| main_tpl(i.stage, &o.tpl);
    let main_count = occs.iter().filter(|o| is_main(o)).count();
    // Shell-computed lists: `{files}`, `{all_files}`, and a file template that is not the stage's main one.
    let mut list: Option<(String, Tpl)> = None;
    for o in &occs {
        if !o.tpl.is_file_list() || is_main(o) {
            continue;
        }
        let cmd = match o.tpl {
            Tpl::Files => {
                if i.files_cmd.trim().is_empty() {
                    out.never = Some(
                        "{files} without a `files` command: lefthook skips the job every time"
                            .to_owned(),
                    );
                    return out;
                }
                i.files_cmd.to_owned()
            }
            Tpl::All => "git ls-files --cached".to_owned(),
            Tpl::Staged => "git diff --name-only --cached --diff-filter=ACMR".to_owned(),
            _ => {
                out.notes.push((
                    Level::Lossy,
                    "{push_files} outside pre-push: approximated with `git diff --name-only HEAD @{push}`".to_owned(),
                ));
                "git diff --name-only HEAD @{push}".to_owned()
            }
        };
        match &list {
            None => list = Some((cmd, o.tpl.clone())),
            Some((_, t)) if *t == o.tpl => {}
            Some(_) => {
                out.notes.push((
                    Level::Lossy,
                    "several different file templates in one command: all receive the first one's list".to_owned(),
                ));
                break;
            }
        }
    }
    if main_count > 0 && list.is_some() {
        out.notes.push((
            Level::Lossy,
            "the stage's file list and a shell-computed file list in one command: both receive the stage's files"
                .to_owned(),
        ));
        list = None;
    }
    let gate = list.is_none() && main_count == 0 && !i.files_cmd.trim().is_empty();
    // Positional values.
    let arg_text = |n: u32, notes: &mut Vec<(Level, String)>| -> String {
        if n == 0 {
            let k = positional_count(i.stage);
            if k == 0 {
                notes.push((
                    Level::Lossy,
                    format!("no pre-commit equivalent for {{0}} in stage '{}'", i.stage),
                ));
                return String::new();
            }
            return (1..=k)
                .filter_map(|m| positional(i.stage, m))
                .collect::<Vec<_>>()
                .join(" ");
        }
        if let Some(v) = positional(i.stage, n) {
            v.to_owned()
        } else {
            notes.push((
                Level::Lossy,
                format!(
                    "no pre-commit equivalent for {{{n}}} in stage '{}'",
                    i.stage
                ),
            ));
            String::new()
        }
    };
    let pass = main_count > 0;
    let mut prefix = String::new();
    if let Some(r) = i.cd_root {
        write!(prefix, "cd {} || exit 1; ", shell::sh_quote(r)).unwrap_or_default();
    }
    prefix.push_str(i.env_prefix);
    if let (Some(r), true) = (i.cd_root, pass && !msg_file_stage(i.stage)) {
        let (bind, pat) = root_pattern(r);
        write!(
            prefix,
            "{bind}for f in \"$@\"; do set -- \"$@\" \"${{f#{pat}}}\"; shift; done; "
        )
        .unwrap_or_default();
    }
    out.pass_filenames = pass;
    out.require_serial = pass;
    out.always_run = i.script || (!pass && !matches!(i.stage, "pre-commit" | "pre-push"));
    out.filters_apply = matches!(i.stage, "pre-commit" | "pre-push") && list.is_none() && !gate;
    if list.is_some() || gate {
        out.always_run = true;
        out.notes.push((
            Level::Warning,
            "best-effort: file list computed in shell (names git must quote are skipped)"
                .to_owned(),
        ));
        let mut script = prefix;
        if let Some((cmd, _)) = &list {
            let p = list_pipeline(cmd, i, matcher, &mut out.notes);
            write!(
                script,
                "set -f; IFS='\n'; set -- $( {p}); unset IFS; set +f; [ $# -gt 0 ] || exit 0; "
            )
            .unwrap_or_default();
        } else {
            let p = list_pipeline(i.files_cmd, i, matcher, &mut out.notes);
            write!(script, "[ -n \"$( {p})\" ] || exit 0; ").unwrap_or_default();
        }
        let mut notes = Vec::new();
        let body = shell::replace_sh(i.cmd, &occs, |o| match &o.tpl {
            t if t.is_file_list() => "\"$@\"".to_owned(),
            Tpl::Arg(n) => unquote_in_dq(i.cmd, o, arg_text(*n, &mut notes)),
            t => t.text(),
        });
        out.notes.extend(notes);
        script.push_str(&body);
        out.entry = shell::wrapper(&script);
        return out;
    }
    let simple_ok = prefix.is_empty();
    if occs.iter().all(|o| matches!(o.tpl, Tpl::Other(_))) {
        if simple_ok && shell::is_simple(i.cmd) {
            i.cmd.trim().clone_into(&mut out.entry);
            out.plain = true;
        } else {
            out.entry = shell::wrapper(&format!("{prefix}{}", i.cmd));
        }
        return out;
    }
    // R2: one main occurrence at the very end of a simple prefix.
    if let [only] = occs.as_slice()
        && is_main(only)
        && simple_ok
        && i.cmd.get(only.end..).unwrap_or("").trim().is_empty()
    {
        let head = i.cmd.get(..only.start).unwrap_or("");
        // The template must start a word: `--edit={1}` or `x{staged_files}` glue the value to the text before it.
        if head.ends_with(|c: char| c.is_ascii_whitespace()) && shell::is_simple(head) {
            head.trim().clone_into(&mut out.entry);
            out.plain = true;
            return out;
        }
    }
    // R3 / R6: a wrapper.
    let mut notes = Vec::new();
    let body = shell::replace_sh(i.cmd, &occs, |o| {
        if is_main(o) {
            "\"$@\"".to_owned()
        } else {
            match &o.tpl {
                Tpl::Arg(n) => unquote_in_dq(i.cmd, o, arg_text(*n, &mut notes)),
                t => t.text(),
            }
        }
    });
    out.notes.extend(notes);
    out.entry = shell::wrapper(&format!("{prefix}{body}"));
    out
}

/// A positional value (`"$VAR"`, or several for `{0}`) written inside an existing double-quoted string: without its
/// own quotes, which would end and reopen the string and leave the value unquoted (field-split).
fn unquote_in_dq(cmd: &str, o: &Occ, value: String) -> String {
    if shell::in_double_quotes(cmd, o.start) {
        value.replace('"', "")
    } else {
        value
    }
}

/// A lefthook condition as snp sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum C {
    Always,
    Never,
    Items(Vec<CondItem>),
}

/// The states lefthook knows (`skip_checker.go`).
const STATES: [&str; 3] = ["merge", "rebase", "merge-commit"];

fn union(a: C, b: C) -> C {
    match (a, b) {
        (C::Always, _) | (_, C::Always) => C::Always,
        (C::Never, x) | (x, C::Never) => x,
        (C::Items(mut x), C::Items(y)) => {
            for i in y {
                if !x.contains(&i) {
                    x.push(i);
                }
            }
            C::Items(x)
        }
    }
}

/// One condition item or a whole condition (`skip_checker.go`; unrecognized values never match).
fn cond_value(v: &LValue, top: bool, notes: &mut Vec<(Level, String)>) -> C {
    match v {
        LValue::Bool(true) if top => C::Always,
        LValue::Str(s) if STATES.contains(&s.as_str()) => {
            C::Items(vec![CondItem::State(s.clone())])
        }
        LValue::Str(s) => {
            notes.push((
                Level::Warning,
                format!("condition '{s}' never matches (lefthook ignores it)"),
            ));
            C::Never
        }
        LValue::List(items) if top => items
            .iter()
            .fold(C::Never, |acc, i| union(acc, cond_value(i, false, notes))),
        LValue::Map(map) => {
            let mut cond = C::Never;
            if let Some(pat) = map.get("ref") {
                let pat = weak_string(pat, "ref").unwrap_or_default();
                match globset::Glob::new(&pat) {
                    Ok(_) => {
                        cond = union(
                            cond,
                            C::Items(vec![CondItem::Map(vec![("ref".to_owned(), pat)])]),
                        );
                    }
                    Err(e) => {
                        notes.push((Level::Lossy, format!("ref pattern '{pat}' dropped: {e}")));
                    }
                }
            }
            if let Some(cmd) = map.get("run") {
                let cmd = weak_string(cmd, "run").unwrap_or_default();
                cond = if cmd.is_empty() {
                    // `sh -c ""` exits 0: the item always matches.
                    C::Always
                } else {
                    union(
                        cond,
                        C::Items(vec![CondItem::Map(vec![("run".to_owned(), cmd)])]),
                    )
                };
            }
            cond
        }
        _ => C::Never,
    }
}

/// The most `only` items the AND of several levels may expand to (beyond it the conversion is lossy).
const MAX_ONLY_PRODUCT: usize = 64;

/// `skip` levels OR, `only` levels AND (05 §6.3, §8.1); an `only` that never matches skips the job.
fn conditions(
    skips: &[LValue],
    onlys: &[LValue],
    notes: &mut Vec<(Level, String)>,
) -> (Option<CondOut>, Option<CondOut>) {
    let skip = skips
        .iter()
        .fold(C::Never, |acc, v| union(acc, cond_value(v, true, notes)));
    let mut levels = Vec::new();
    let mut never = false;
    for v in onlys {
        match cond_value(v, true, notes) {
            C::Always => {}
            C::Never => never = true,
            C::Items(i) => levels.push(i),
        }
    }
    if never || skip == C::Always {
        return (Some(CondOut::Always), None);
    }
    let only = match levels.len() {
        0 => None,
        1 => levels.pop().map(CondOut::Items),
        _ => {
            // AND of OR-lists: a cross product of maps; states cannot be combined with other items.
            let mut acc: Vec<Vec<(String, String)>> = vec![Vec::new()];
            let mut ok = true;
            for level in &levels {
                // Bounded while it is built: N nested levels of k items would otherwise allocate k^N maps.
                if acc.len().saturating_mul(level.len()) > MAX_ONLY_PRODUCT {
                    ok = false;
                    break;
                }
                let mut next = Vec::new();
                for a in &acc {
                    for item in level {
                        let CondItem::Map(kv) = item else {
                            ok = false;
                            continue;
                        };
                        let mut m = a.clone();
                        for (k, v) in kv {
                            match m.iter().find(|(x, _)| x == k) {
                                Some((_, w)) if w != v => ok = false,
                                Some(_) => {}
                                None => m.push((k.clone(), v.clone())),
                            }
                        }
                        next.push(m);
                    }
                }
                acc = next;
            }
            if ok {
                Some(CondOut::Items(acc.into_iter().map(CondItem::Map).collect()))
            } else {
                notes.push((
                    Level::Lossy,
                    "`only` conditions of the hook/group and the job cannot be combined; only the job's is kept".to_owned(),
                ));
                levels.pop().map(CondOut::Items)
            }
        }
    };
    let skip = match skip {
        C::Items(i) => Some(CondOut::Items(i)),
        C::Never | C::Always => None,
    };
    (skip, only)
}

/// `file_types` → identify tags (05 §19.7): `(types, types_or, exclude_types)`.
fn file_types(
    fts: &[String],
    notes: &mut Vec<(Level, String)>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut types, mut types_or, mut exclude) = (Vec::new(), Vec::new(), Vec::new());
    let push = |v: &mut Vec<String>, t: &str| {
        if !v.iter().any(|x| x == t) {
            v.push(t.to_owned());
        }
    };
    for ft in fts {
        match ft.as_str() {
            "text" | "binary" | "executable" | "symlink" => push(&mut types, ft),
            "not executable" => push(&mut exclude, "executable"),
            "not symlink" => push(&mut exclude, "symlink"),
            m if m.contains('/') => {
                let tag = match m {
                    "text/x-python" => Some("python"),
                    "text/x-sh" | "text/x-shellscript" => Some("shell"),
                    "text/x-perl" => Some("perl"),
                    "text/x-lua" => Some("lua"),
                    "text/x-php" => Some("php"),
                    "text/html" => Some("html"),
                    "text/xml" => Some("xml"),
                    "text/javascript" => Some("javascript"),
                    "application/json" => Some("json"),
                    _ => None,
                };
                match tag.filter(|t| file_identify::ALL_TAGS.contains(t)) {
                    Some(t) => push(&mut types_or, t),
                    None => notes.push((
                        Level::Lossy,
                        format!("no identify tag for MIME '{m}'; filter dropped"),
                    )),
                }
            }
            other => notes.push((
                Level::Warning,
                format!("unknown file type '{other}' ignored (as lefthook does)"),
            )),
        }
    }
    (types, types_or, exclude)
}

/// lefthook's `fail_on_changes` → snp's (05 §19.8); `Err` for a value lefthook rejects.
fn fail_on_changes(v: &str) -> Result<&'static str, ()> {
    match v {
        "" | "never" | "false" | "0" => Ok("never"),
        "always" | "true" | "1" => Ok("always"),
        "ci" => Ok("ci"),
        "non-ci" => Ok("non-ci"),
        _ => Err(()),
    }
}

/// The fixer heuristic of 05 §19.8.
fn looks_like_fixer(run: &str) -> bool {
    run.split_whitespace().any(|w| {
        matches!(
            w,
            "--fix" | "--write" | "-w" | "fmt" | "format" | "-i" | "--autofix"
        )
    })
}

/// lefthook's failure for a job without exactly one of `run` / `script` / `group` (`job.go:20-79`).
const NEITHER: &str = "either `run`,`script`, or `group` must be provided for a job";

/// Flattens a hook's job list into the scheduling tree.
fn flatten(
    cx: &mut Cx<'_>,
    hook: &Hook,
    jobs: &[Job],
    scope: &Scope,
    leaves: &mut Vec<Leaf>,
    depth: usize,
) -> Vec<Node> {
    let mut nodes = Vec::new();
    for (pos, j) in jobs.iter().enumerate() {
        let printable = model::printable_name(j, pos);
        let all_tags: Vec<&String> = scope.tags.iter().chain(j.tags.iter()).collect();
        if hook
            .exclude_tags
            .iter()
            .any(|t| all_tags.contains(&t) || *t == printable)
        {
            cx.note(
                Level::Warning,
                &format!("{}/{}", hook.name, display_name(&printable)),
                "excluded by exclude_tags; not converted",
            );
            continue;
        }
        let has_cmd = !j.run.is_empty() || !j.script.is_empty();
        let fail = if !j.run.is_empty() && !j.script.is_empty() {
            Some(NEITHER.to_owned())
        } else if has_cmd {
            None
        } else if let Some(g) = &j.group {
            if !g.has_jobs || g.jobs.is_empty() {
                Some("group must have `jobs`".to_owned())
            } else if depth >= super::value::MAX_DEPTH {
                Some("groups are nested too deeply".to_owned())
            } else {
                let mut inner = scope.with(j);
                if g.piped && !g.parallel {
                    inner.piped = true;
                    cx.note(
                        Level::Lossy,
                        &format!("{}/{}", hook.name, display_name(&printable)),
                        "piped group: pre-commit fail_fast stops the whole run, lefthook piped only stopped its group",
                    );
                }
                let children = flatten(cx, hook, &g.jobs, &inner, leaves, depth.saturating_add(1));
                nodes.push(if g.parallel {
                    Node::Par(children)
                } else {
                    Node::Seq(children)
                });
                continue;
            }
        } else {
            Some(NEITHER.to_owned())
        };
        let index = leaves.len().saturating_add(1);
        nodes.push(Node::Leaf(leaves.len()));
        leaves.push(Leaf {
            job: j.clone(),
            scope: scope.with(j),
            index,
            fail,
        });
    }
    nodes
}

/// `schedule` of 05 §19.4: assigns buckets; returns the next free bucket.
fn schedule(n: &Node, b: usize, buckets: &mut [usize], reduced: &mut bool) -> usize {
    match n {
        Node::Leaf(i) => {
            if let Some(slot) = buckets.get_mut(*i) {
                *slot = b;
            }
            b.saturating_add(1)
        }
        Node::Seq(cs) => cs.iter().fold(b, |b, c| schedule(c, b, buckets, reduced)),
        Node::Par(cs) => {
            let mut end = b.saturating_add(1);
            for c in cs {
                let e = schedule(c, b, buckets, reduced);
                if e > b.saturating_add(1) && cs.len() > 1 {
                    *reduced = true;
                }
                end = end.max(e);
            }
            if cs.is_empty() { b } else { end }
        }
    }
}

fn has_par(n: &Node) -> bool {
    match n {
        Node::Leaf(_) => false,
        Node::Par(_) => true,
        Node::Seq(cs) => cs.iter().any(has_par),
    }
}

/// One converted hook before stage-level ordering.
struct Pending {
    hook: OutHook,
    stage: String,
    bucket: usize,
    dfs: usize,
}

/// The script lookup of 05 §7.4: the first existing `<dir>/<hook>/<script>`, and how many exist.
fn find_script(cx: &Cx<'_>, hook: &str, script: &str) -> (Option<String>, usize) {
    let mut found = Vec::new();
    for d in &cx.source_dirs {
        let rel = format!("{}/{hook}/{script}", d.trim_end_matches('/'));
        if cx.o.root.join(&rel).is_file() {
            found.push(rel);
        }
    }
    let n = found.len();
    (found.into_iter().next(), n)
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_: &Path) -> bool {
    true
}

/// Converts one leaf.
#[expect(clippy::too_many_arguments, reason = "the leaf's context")]
fn convert_leaf(
    cx: &Cx<'_>,
    hook: &Hook,
    stage: &str,
    custom_group: Option<&str>,
    leaf: &Leaf,
    fail_value: Option<&str>,
    notes: &mut Vec<(Level, String)>,
    out: &mut OutHook,
) -> bool {
    let j = &leaf.job;
    let s = &leaf.scope;
    let portable = cx.o.portable;
    let printable = model::printable_name(j, leaf.index.saturating_sub(1));
    out.name = if j.name.is_empty() {
        display_name(&printable)
    } else {
        j.name.clone()
    };
    if portable && let Some(g) = custom_group {
        out.name = format!("{} ({g})", out.name);
    }
    "system".clone_into(&mut out.language);
    out.stages = vec![stage.to_owned()];
    out.pass_filenames = true;
    out.fail_fast = s.piped;
    out.verbose = hook.follow;
    let fail_hook = |out: &mut OutHook, msg: &str| {
        "fail".clone_into(&mut out.language);
        out.entry = format!("lefthook: {msg}");
        out.pass_filenames = false;
        out.always_run = true;
    };
    if let Some(msg) = &leaf.fail {
        fail_hook(out, msg);
        notes.push((
            Level::Warning,
            format!("lefthook fails this job ({msg}); converted to a `fail` hook"),
        ));
        return true;
    }
    // Conditions.
    let (skip, only) = conditions(&s.skip, &s.only, notes);
    if portable {
        if skip == Some(CondOut::Always) {
            notes.push((
                Level::Warning,
                "always skipped by lefthook; not converted".to_owned(),
            ));
            return false;
        }
        if skip.is_some() || only.is_some() {
            notes.push((
                Level::Lossy,
                "skip/only conditions dropped (portable mode)".to_owned(),
            ));
        }
    } else {
        out.skip = skip;
        out.only = only;
    }
    // Root.
    let root = normalize_root(&s.root);
    let root_ok = root.as_deref().is_some_and(|r| root_key_ok(r, &s.root));
    if root.is_some() && !root_ok {
        notes.push((
            Level::Lossy,
            format!(
                "root '{}' must be a relative directory inside the repository; ignored",
                s.root
            ),
        ));
    }
    let root = root.filter(|_| root_ok);
    // Environment.
    let mut env_prefix = String::new();
    for (k, v) in &s.env {
        let in_wrapper = portable || v.contains('$');
        if in_wrapper {
            if shell::is_var_name(k) {
                write!(env_prefix, "{k}={}; export {k}; ", shell::dq_expand(v)).unwrap_or_default();
            } else {
                notes.push((
                    Level::Lossy,
                    format!("env '{k}' is not a shell variable name; dropped"),
                ));
            }
        } else if k.is_empty() || k.contains(['=', '\0']) || v.contains('\0') {
            notes.push((
                Level::Lossy,
                format!("env '{k}' is not a valid variable; dropped"),
            ));
        } else {
            out.env.push((k.clone(), v.clone()));
        }
    }
    // The command.
    let mut cmd_src = j.run.clone();
    let mut script_mode = false;
    if j.origin == Origin::Script || (!j.script.is_empty() && j.run.is_empty()) {
        script_mode = true;
        let (path, n) = find_script(cx, &hook.name, &j.script);
        let Some(path) = path else {
            fail_hook(out, &format!("script does not exist: {}", j.script));
            notes.push((
                Level::Warning,
                format!(
                    "script '{}' not found in the source directories; converted to a `fail` hook",
                    j.script
                ),
            ));
            return true;
        };
        if n > 1 {
            notes.push((Level::Lossy, format!("script '{}' exists in {n} source directories; lefthook ran every copy, only {path} is converted", j.script)));
        }
        if j.runner.is_empty() && !is_executable(&cx.o.root.join(&path)) {
            notes.push((
                Level::Lossy,
                format!(
                    "{path} is not executable (lefthook ran chmod on it); run `chmod +x {path}`"
                ),
            ));
        }
        cmd_src = if j.runner.is_empty() {
            shell::sh_quote(&path)
        } else {
            format!("{} {}", j.runner, shell::sh_quote(&path))
        };
        if !j.args.is_empty() {
            cmd_src.push(' ');
            cmd_src.push_str(&j.args);
        } else if positional_count(stage) > 0 {
            cmd_src.push_str(" {0}");
        }
    } else if !j.args.is_empty() {
        cmd_src.push(' ');
        cmd_src.push_str(&j.args);
    }
    let cmd = shell::inline_templates(&cmd_src, &cx.templates, &printable);
    // snp's `root` key, unless the message file must stay reachable or the mode is portable.
    let root_key = root.is_some() && !portable && !msg_file_stage(stage);
    let cd_root = if root_key { None } else { root.as_deref() };
    let files_cmd = shell::inline_templates(&s.files, &cx.templates, &printable);
    let input = EntryIn {
        cmd: &cmd,
        stage,
        files_cmd: &files_cmd,
        glob: &s.glob,
        exclude: &s.exclude,
        cd_root,
        list_root: root.as_deref(),
        env_prefix: &env_prefix,
        script: script_mode,
    };
    let e = entry_rules(&input, cx.matcher);
    notes.extend(e.notes);
    if let Some(msg) = e.never {
        if msg.starts_with("one of") {
            fail_hook(out, &msg);
            notes.push((
                Level::Warning,
                format!("lefthook fails this job ({msg}); converted to a `fail` hook"),
            ));
            return true;
        }
        notes.push((Level::Warning, format!("{msg}; not converted")));
        return false;
    }
    out.entry = e.entry;
    out.pass_filenames = e.pass_filenames;
    out.always_run = e.always_run;
    out.require_serial = e.require_serial;
    if j.origin == Origin::Script && j.runner.is_empty() && e.plain {
        "script".clone_into(&mut out.language);
    }
    if root_key {
        out.root.clone_from(&root);
    }
    // Filters.
    if e.filters_apply {
        let root_prefix = if portable { root.as_deref() } else { None };
        match glob::files_regex(&s.glob, cx.matcher, root_prefix) {
            Ok(f) => {
                slow_regex_note("glob", f.as_deref(), notes);
                out.files = f;
            }
            Err(err) if cx.matcher == Matcher::Gobwas => {
                fail_hook(out, &format!("invalid glob pattern: {}", err.0));
                notes.push((
                    Level::Warning,
                    format!(
                        "lefthook panics on this job's glob ({}); converted to a `fail` hook",
                        err.0
                    ),
                ));
                return true;
            }
            Err(err) => {
                notes.push((
                    Level::Warning,
                    format!(
                        "doublestar treats an invalid glob as no match ({}); not converted",
                        err.0
                    ),
                ));
                return false;
            }
        }
        match glob::exclude_regex(&s.exclude, cx.matcher) {
            Ok(x) => {
                slow_regex_note("exclude", x.as_deref(), notes);
                out.exclude = x;
            }
            Err(err) if cx.matcher == Matcher::Gobwas => {
                fail_hook(out, &format!("invalid glob pattern: {}", err.0));
                notes.push((
                    Level::Warning,
                    format!(
                        "lefthook panics on this job's exclude ({}); converted to a `fail` hook",
                        err.0
                    ),
                ));
                return true;
            }
            Err(err) => notes.push((
                Level::Warning,
                format!(
                    "doublestar treats an invalid exclude as no match ({}); exclude dropped",
                    err.0
                ),
            )),
        }
        let (t, to, xt) = file_types(&s.file_types, notes);
        out.types = t;
        out.types_or = to;
        out.exclude_types = xt;
        if let Some(r) = &root {
            for x in &s.exclude {
                let literal = !x.starts_with(['*', '?', '[', '{']);
                if literal && !x.starts_with(&format!("{r}/")) {
                    notes.push((
                        Level::Warning,
                        format!(
                            "exclude '{x}' is matched from the repo root (lefthook semantics) and never matches under root '{}'",
                            s.root
                        ),
                    ));
                }
            }
        }
    } else if !s.file_types.is_empty() && matches!(stage, "pre-commit" | "pre-push") {
        notes.push((
            Level::Lossy,
            "file_types cannot filter a shell-computed file list; dropped".to_owned(),
        ));
    }
    // Groups.
    let mut groups: Vec<String> = Vec::new();
    for g in custom_group
        .into_iter()
        .map(str::to_owned)
        .chain(s.tags.iter().cloned())
    {
        if groups.contains(&g) {
            continue;
        }
        if g.is_empty() || g.starts_with('@') || g.chars().any(char::is_whitespace) {
            if !portable {
                notes.push((
                    Level::Warning,
                    format!("tag '{g}' is not a valid snp group name; dropped"),
                ));
            }
        } else {
            groups.push(g);
        }
    }
    if portable {
        if !s.tags.is_empty() {
            // Lossy (05 §19.1 `--strict`): `snp run --group` selection is gone.
            notes.push((Level::Lossy, "tags dropped (portable mode)".to_owned()));
        }
    } else {
        out.groups = groups;
    }
    // Per-job keys.
    if !j.fail_text.is_empty() {
        if portable {
            notes.push((Level::Lossy, "fail_text dropped (portable mode)".to_owned()));
        } else {
            out.fail_text = Some(j.fail_text.clone());
        }
    }
    if let Some(t) = &j.timeout {
        match snp_config::ext::parse_go_duration(t) {
            Ok(d) if d.is_zero() => {}
            Ok(d) if d <= std::time::Duration::from_hours(24) => {
                if portable {
                    notes.push((Level::Lossy, "timeout dropped (portable mode)".to_owned()));
                } else {
                    out.timeout = Some(t.clone());
                }
            }
            Ok(_) => notes.push((
                Level::Lossy,
                format!("timeout '{t}' is longer than 24h; dropped"),
            )),
            Err(e) => notes.push((Level::Lossy, format!("timeout '{t}' dropped: {e}"))),
        }
    }
    if j.use_stdin {
        if portable {
            notes.push((Level::Lossy, "use_stdin dropped (portable mode)".to_owned()));
        } else {
            out.use_stdin = true;
        }
    }
    if j.interactive {
        out.require_serial = true;
        if portable {
            notes.push((
                Level::Lossy,
                "interactive (TTY passthrough) is not supported yet; dropped".to_owned(),
            ));
        } else {
            out.interactive = true;
        }
    }
    if j.stage_fixed {
        if stage != "pre-commit" {
            notes.push((
                Level::Warning,
                "stage_fixed only applies to the pre-commit hook; dropped".to_owned(),
            ));
        } else if portable {
            notes.push((
                Level::Lossy,
                "stage_fixed dropped (portable mode): fixes are no longer staged".to_owned(),
            ));
        } else {
            out.stage_fixed = true;
        }
    }
    if let Some(v) = fail_value
        && !portable
    {
        out.fail_on_changes = Some(v.to_owned());
    }
    if !portable
        && stage == "pre-commit"
        && !j.stage_fixed
        && fail_value.is_none_or(|v| v == "never")
        && looks_like_fixer(&cmd)
    {
        notes.push((
            Level::Warning,
            "looks like a fixer: consider `stage_fixed: true` and removing fail_on_changes"
                .to_owned(),
        ));
    }
    true
}

/// Converts every hook of `loaded` (05 §19.2 step 4).
pub(crate) fn convert(loaded: &Loaded, o: Opts<'_>) -> Result<Conversion, String> {
    let g = &loaded.global;
    let gs = |k: &str| {
        g.get(k)
            .and_then(|v| weak_string(v, k).ok())
            .unwrap_or_default()
    };
    let matcher = match gs("glob_matcher").as_str() {
        "doublestar" => Matcher::Doublestar,
        _ => Matcher::Gobwas,
    };
    let templates = match g.get("templates") {
        Some(LValue::Map(m)) => m
            .iter()
            .map(|(k, v)| (k.clone(), weak_string(v, k).unwrap_or_default()))
            .collect(),
        _ => Vec::new(),
    };
    let mut source_dir = gs("source_dir");
    if source_dir.is_empty() {
        ".lefthook".clone_into(&mut source_dir);
    }
    let mut source_dir_local = gs("source_dir_local");
    if source_dir_local.is_empty() {
        ".lefthook-local".clone_into(&mut source_dir_local);
    }
    let mut cx = Cx {
        o,
        matcher,
        templates,
        source_dirs: vec![
            source_dir,
            source_dir_local,
            ".config/lefthook".to_owned(),
            ".config/lefthook-local".to_owned(),
        ],
        notes: Vec::new(),
    };
    let main_name = if loaded.main.is_empty() {
        loaded.local.clone().unwrap_or_default()
    } else {
        loaded.main.clone()
    };
    let mut conv = Conversion::default();
    for w in &loaded.warnings {
        cx.note(Level::Warning, &main_name, w.clone());
    }
    // Hooks decode first, so a type error fails before anything is reported.
    let mut hooks = Vec::new();
    for raw in &loaded.hooks {
        let h = model::hook(&raw.name, &raw.map).map_err(|e| format!("{}: {}", e.path, e.msg))?;
        hooks.push(h);
    }
    conv.lefthook_hooks = hooks.len();
    // fail_on_changes (05 §19.8).
    let mut values = Vec::new();
    for h in &hooks {
        if let Ok(v) = fail_on_changes(&h.fail_on_changes) {
            values.push(v);
        } else {
            cx.note(
                Level::Lossy,
                &h.name,
                format!(
                    "invalid value for fail_on_changes: {} (lefthook fails the hook); treated as never",
                    h.fail_on_changes
                ),
            );
            values.push("never");
        }
    }
    let all_never = values.iter().all(|v| *v == "never");
    conv.root_never = !o.portable && all_never && !hooks.is_empty();
    if o.portable && values.contains(&"never") {
        // Lossy (05 §19.1 `--strict`): a hook that modifies files now fails, which lefthook's default never did.
        cx.note(
            Level::Lossy,
            &main_name,
            "pre-commit fails hooks that modify files; lefthook did not",
        );
    }
    let mut pending: Vec<Pending> = Vec::new();
    let mut stage_par: Vec<(String, bool, usize)> = Vec::new(); // stage, has Par, next free bucket
    for (h, fval) in hooks.iter().zip(values) {
        let (stage, custom_group) = if HOOK_TYPES.contains(&h.name.as_str()) {
            (h.name.clone(), None)
        } else {
            if AVAILABLE_HOOKS.contains(&h.name.as_str()) {
                cx.note(
                    Level::Lossy,
                    &h.name,
                    format!(
                        "git hook '{}' is not supported by pre-commit; converted to manual stage",
                        h.name
                    ),
                );
            }
            ("manual".to_owned(), Some(h.name.clone()))
        };
        for s in &h.setup {
            cx.note(
                Level::Lossy,
                &h.name,
                format!("setup command not converted: {}", display_name(s)),
            );
        }
        let mut piped = h.piped;
        let mut parallel = h.parallel;
        if piped && parallel {
            cx.note(
                Level::Lossy,
                &h.name,
                "conflicting options 'piped' and 'parallel' (lefthook refuses to run the hook); converted as piped",
            );
            parallel = false;
            piped = true;
        }
        let scope = Scope {
            exclude: h.exclude.clone(),
            files: h.files.clone(),
            skip: h.skip.iter().cloned().collect(),
            only: h.only.iter().cloned().collect(),
            piped,
            ..Scope::default()
        };
        let mut leaves = Vec::new();
        let children = flatten(&mut cx, h, &h.jobs, &scope, &mut leaves, 0);
        let tree = if parallel {
            Node::Par(children)
        } else {
            Node::Seq(children)
        };
        conv.jobs = conv.jobs.saturating_add(leaves.len());
        let mut buckets = vec![0_usize; leaves.len()];
        let mut reduced = false;
        let start = stage_par
            .iter()
            .find(|(s, _, _)| *s == stage)
            .map_or(0, |(_, _, n)| *n);
        let end = schedule(&tree, start, &mut buckets, &mut reduced);
        let par = has_par(&tree);
        match stage_par.iter_mut().find(|(s, _, _)| *s == stage) {
            Some(slot) => {
                slot.1 |= par;
                slot.2 = end;
            }
            None => stage_par.push((stage.clone(), par, end)),
        }
        if reduced {
            cx.note(
                Level::Warning,
                &h.name,
                "parallelism reduced: a sequential group inside a parallel one is serialised",
            );
        }
        let fail_value = if o.portable || all_never {
            None
        } else {
            Some(fval)
        };
        for (k, leaf) in leaves.iter().enumerate() {
            let mut notes = Vec::new();
            let mut out = OutHook::default();
            let keep = convert_leaf(
                &cx,
                h,
                &stage,
                custom_group.as_deref(),
                leaf,
                fail_value,
                &mut notes,
                &mut out,
            );
            let printable = model::printable_name(&leaf.job, leaf.index.saturating_sub(1));
            let scope = format!("{}/{}", h.name, display_name(&printable));
            for (level, msg) in notes {
                let n = Note {
                    level,
                    scope: scope.clone(),
                    msg,
                };
                if keep {
                    out.comments.push(n.line());
                }
                cx.notes.push(n);
            }
            if !keep {
                continue;
            }
            let base = if leaf.job.name.is_empty() {
                String::new()
            } else {
                slug(&leaf.job.name)
            };
            out.id = if base.is_empty() {
                let s = slug(&printable);
                if leaf.job.name.is_empty() || s.is_empty() {
                    format!("{}-job-{}", h.name, leaf.index)
                } else {
                    s
                }
            } else {
                base
            };
            pending.push(Pending {
                hook: out,
                stage: stage.clone(),
                bucket: buckets.get(k).copied().unwrap_or(0),
                dfs: pending.len(),
            });
        }
    }
    // Stage ordering, ids and priorities.
    for (stage, par, _) in &stage_par {
        let mut group: Vec<Pending> = Vec::new();
        let mut rest = Vec::new();
        for p in std::mem::take(&mut pending) {
            if p.stage == *stage {
                group.push(p);
            } else {
                rest.push(p);
            }
        }
        pending = rest;
        group.sort_by_key(|p| (p.bucket, p.dfs));
        let mut used: Vec<String> = Vec::new();
        for mut p in group {
            let base = p.hook.id.clone();
            let mut id = base.clone();
            let mut n = 2_usize;
            while used.contains(&id) {
                id = format!("{base}-{n}");
                n = n.saturating_add(1);
            }
            used.push(id.clone());
            p.hook.id = id;
            if *par && !o.portable {
                p.hook.priority =
                    Some(u32::try_from(p.bucket.saturating_mul(10)).unwrap_or(u32::MAX));
            }
            conv.stages.push(stage.clone());
            conv.hooks.push(p.hook);
        }
    }
    conv.install_types = HOOK_TYPES
        .iter()
        .filter(|t| conv.stages.iter().any(|s| s == *t))
        .map(|t| (*t).to_owned())
        .collect();
    conv.notes = cx.notes;
    Ok(conv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(cmd: &str, stage: &str) -> EntryOut {
        entry_rules(
            &EntryIn {
                cmd,
                stage,
                files_cmd: "",
                glob: &[],
                exclude: &[],
                cd_root: None,
                list_root: None,
                env_prefix: "",
                script: false,
            },
            Matcher::Gobwas,
        )
    }

    #[test]
    fn rule_r1() {
        let e = rules("yarn test", "pre-push");
        assert_eq!(e.entry, "yarn test");
        assert!(!e.pass_filenames && !e.always_run && e.plain);
        let e = rules("make", "post-checkout");
        assert!(e.always_run);
        let e = rules("a | b", "pre-commit");
        assert_eq!(e.entry, "sh -c 'a | b' --");
        assert!(!e.pass_filenames);
    }

    #[test]
    fn rule_r2() {
        let e = rules("yarn eslint --fix {staged_files}", "pre-commit");
        assert_eq!(e.entry, "yarn eslint --fix");
        assert!(e.pass_filenames && e.require_serial);
        let e = rules("npx commitlint --edit {1}", "commit-msg");
        assert_eq!(e.entry, "npx commitlint --edit");
        assert!(e.pass_filenames && !e.always_run);
        let e = rules("rubocop \"{staged_files}\"", "pre-commit");
        assert_eq!(e.entry, "rubocop");
    }

    #[test]
    fn rule_r3_and_r6() {
        let e = rules("eslint {staged_files} && echo ok", "pre-commit");
        assert_eq!(e.entry, "sh -c 'eslint \"$@\" && echo ok' --");
        let e = rules("echo {1} {2}", "pre-push");
        assert_eq!(
            e.entry,
            "sh -c 'echo \"$PRE_COMMIT_REMOTE_NAME\" \"$PRE_COMMIT_REMOTE_URL\"' --"
        );
        assert!(!e.pass_filenames && !e.always_run);
        let e = rules("echo {0}", "post-checkout");
        assert_eq!(
            e.entry,
            "sh -c 'echo \"$PRE_COMMIT_FROM_REF\" \"$PRE_COMMIT_TO_REF\" \"$PRE_COMMIT_CHECKOUT_TYPE\"' --"
        );
        let e = rules("echo {3}", "pre-push");
        assert_eq!(e.entry, "sh -c 'echo ' --");
        assert_eq!(e.notes[0].0, Level::Lossy);
    }

    #[test]
    fn rule_r5() {
        let e = entry_rules(
            &EntryIn {
                cmd: "rubocop {files}",
                stage: "pre-commit",
                files_cmd: "git diff --name-only main",
                glob: &["*.rb".to_owned()],
                exclude: &["vendor/*".to_owned()],
                cd_root: None,
                list_root: None,
                env_prefix: "",
                script: false,
            },
            Matcher::Gobwas,
        );
        assert!(e.always_run && !e.pass_filenames && !e.filters_apply);
        assert_eq!(
            e.entry,
            "sh -c 'set -f; IFS='\\''\n'\\''; set -- $( ( [ -n \"${GIT_CONFIG_COUNT-}\" ] || export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.quotePath GIT_CONFIG_VALUE_0=false; git diff --name-only main ) | sed '\\''s/^[[:space:]]*//; s/[[:space:]]*$//'\\'' | grep -iE '\\''^(.*\\.rb)$'\\'' | grep -vE '\\''^(vendor/.*)$'\\'' | while IFS= read -r f; do [ -f \"$f\" ] && printf '\\''%s\\n'\\'' \"$f\"; done); unset IFS; set +f; [ $# -gt 0 ] || exit 0; rubocop \"$@\"' --"
        );
        let e = rules("lint {all_files}", "pre-commit");
        assert!(e.entry.contains("git ls-files --cached"));
        let e = rules("lint {files}", "pre-commit");
        assert!(e.never.is_some());
    }

    #[test]
    fn incompatible_templates() {
        assert!(
            rules("x {staged_files} {push_files}", "pre-commit")
                .never
                .is_some()
        );
    }

    #[test]
    fn slugs_and_names() {
        assert_eq!(slug("Lint: JS files!"), "lint-js-files");
        assert_eq!(slug("--"), "");
        let long: String = std::iter::repeat_n('x', 70).collect();
        let short: String = std::iter::repeat_n('x', 60).collect();
        assert_eq!(display_name(&long), format!("{short}…"));
    }

    #[test]
    fn condition_translation() {
        let mut notes = Vec::new();
        let v = super::super::value::parse(
            "s: [merge, {ref: main, run: test -f x}, bogus]\n",
            super::super::value::Format::Yaml,
        )
        .unwrap();
        let (skip, only) = conditions(&[v["s"].clone()], &[], &mut notes);
        assert_eq!(
            skip,
            Some(CondOut::Items(vec![
                CondItem::State("merge".to_owned()),
                CondItem::Map(vec![("ref".to_owned(), "main".to_owned())]),
                CondItem::Map(vec![("run".to_owned(), "test -f x".to_owned())]),
            ]))
        );
        assert!(only.is_none());
        let (skip, _) = conditions(&[], &[LValue::Bool(false)], &mut notes);
        assert_eq!(skip, Some(CondOut::Always));
        let (skip, only) = conditions(&[], &[LValue::Bool(true)], &mut notes);
        assert!(skip.is_none() && only.is_none());
    }

    /// R2 only when the template starts a word; otherwise R3 keeps the glued text in the same argument.
    #[test]
    fn rule_r2_needs_a_word_start() {
        let e = rules("commitlint --edit={1}", "commit-msg");
        assert_eq!(e.entry, "sh -c 'commitlint --edit=\"$@\"' --");
        assert!(e.pass_filenames && !e.plain);
        let e = rules("argv x{staged_files}", "pre-commit");
        assert_eq!(e.entry, "sh -c 'argv x\"$@\"' --");
        let e = rules("lint --x={push_files}", "pre-push");
        assert_eq!(e.entry, "sh -c 'lint --x=\"$@\"' --");
        let e = rules("lint\t{staged_files}", "pre-commit");
        assert_eq!(e.entry, "lint");
    }

    /// Shell-only builtins and reserved words run in `sh`, as lefthook runs every command.
    #[test]
    fn shell_builtins_are_wrapped() {
        for cmd in [
            "exit 0",
            "source ./env",
            "ulimit -n 100",
            ". ./x",
            ": ok",
            "cd sub",
            "! false",
        ] {
            let e = rules(cmd, "pre-commit");
            assert_eq!(e.entry, shell::wrapper(cmd), "{cmd}");
            assert!(!e.plain, "{cmd}");
        }
        let e = rules("exit {staged_files}", "pre-commit");
        assert_eq!(e.entry, "sh -c 'exit \"$@\"' --");
        assert_eq!(rules("exitcode 0", "pre-commit").entry, "exitcode 0");
    }

    /// A template after an unescaped backslash: the backslash goes (lefthook's would escape the substituted
    /// value's first character), so the quote of `"$@"` is never escaped.
    #[test]
    fn backslash_before_a_template() {
        let e = rules("argv x\\{staged_files}", "pre-commit");
        assert_eq!(e.entry, "sh -c 'argv x\"$@\"' --");
        let e = rules("argv x\\\\{staged_files}", "pre-commit");
        assert_eq!(e.entry, "sh -c 'argv x\\\\\"$@\"' --");
        let e = rules("echo \\{1}", "commit-msg");
        assert_eq!(e.entry, "sh -c 'echo \"$@\"' --");
        let e = rules("echo \\{unknown} {staged_files}", "pre-commit");
        assert!(e.entry.contains("\\{unknown}"), "{}", e.entry);
    }

    /// The script of `sh -c '<script>' --` (the inverse of [`shell::wrapper`]).
    fn script_of(entry: &str) -> String {
        let quoted = entry
            .strip_prefix("sh -c ")
            .and_then(|s| s.strip_suffix(" --"))
            .unwrap();
        quoted
            .strip_prefix('\'')
            .and_then(|q| q.strip_suffix('\''))
            .unwrap()
            .replace("'\\''", "'")
    }

    #[cfg(unix)]
    fn run_sh(dir: &Path, script: &str, args: &[&str]) -> String {
        let out = std::process::Command::new("sh")
            .current_dir(dir)
            .arg("-c")
            .arg(script)
            .arg("--")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{script}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    /// The root is data: `*`, `[`, `}`, `"`, `$(…)` and spaces in it are matched literally by the `${f#…}` strip,
    /// never run or used as a pattern (R5 and R7).
    #[cfg(unix)]
    #[test]
    fn root_is_never_shell_code() {
        let root = "a b*[x]}\"$(touch PWNED)";
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(root).join("sub")).unwrap();
        std::fs::write(d.path().join(root).join("f1.txt"), "").unwrap();
        std::fs::write(d.path().join(root).join("sub/f2"), "").unwrap();
        let (bind, pat) = root_pattern(root);
        assert_eq!(pat, "\"$snp_r\"");
        assert!(
            bind.starts_with("snp_r='a b*[x]}\"$(touch PWNED)/'"),
            "{bind}"
        );
        assert_eq!(
            root_pattern("backend"),
            (String::new(), "backend/".to_owned())
        );
        // R7: the `"$@"` paths lose the root prefix.
        let e = entry_rules(
            &EntryIn {
                cmd: "printf '%s\\n' {staged_files}",
                stage: "pre-commit",
                files_cmd: "",
                glob: &[],
                exclude: &[],
                cd_root: Some(root),
                list_root: None,
                env_prefix: "",
                script: false,
            },
            Matcher::Gobwas,
        );
        let f1 = format!("{root}/f1.txt");
        let f2 = format!("{root}/sub/f2");
        assert_eq!(
            run_sh(d.path(), &script_of(&e.entry), &[&f1, &f2, "other/x"]),
            "f1.txt\nsub/f2\nother/x\n"
        );
        // R5: the shell file list is filtered by the root and stripped of it.
        std::fs::write(
            d.path().join("list.txt"),
            format!("{f1}\n{f2}\nother/x\n{root}x/f1.txt\n"),
        )
        .unwrap();
        let e = entry_rules(
            &EntryIn {
                cmd: "printf '%s\\n' {files}",
                stage: "pre-commit",
                files_cmd: "cat ../list.txt",
                glob: &[],
                exclude: &[],
                cd_root: Some(root),
                list_root: Some(root),
                env_prefix: "",
                script: false,
            },
            Matcher::Gobwas,
        );
        assert_eq!(
            run_sh(d.path(), &script_of(&e.entry), &[]),
            "f1.txt\nsub/f2\n"
        );
        assert!(!d.path().join("PWNED").exists());
        assert!(!d.path().join(root).join("PWNED").exists());
    }

    /// N nested `only` levels of k items never build the k^N product (it stops at the bound and falls back).
    #[test]
    fn only_product_is_bounded() {
        let level = |key: &str| {
            LValue::List(
                (0..4)
                    .map(|i| {
                        let mut m = super::super::value::LMap::new();
                        m.insert(key.to_owned(), LValue::Str(format!("{key}{i}")));
                        LValue::Map(m)
                    })
                    .collect(),
            )
        };
        let levels = vec![level("ref"); 40];
        let mut notes = Vec::new();
        let (_, only) = conditions(&[], &levels, &mut notes);
        assert!(notes.iter().any(|(l, _)| *l == Level::Lossy));
        assert!(matches!(only, Some(CondOut::Items(i)) if i.len() == 4));
        let mut notes = Vec::new();
        let (_, only) = conditions(&[], &[level("ref"), level("run")], &mut notes);
        assert!(matches!(only, Some(CondOut::Items(i)) if i.len() == 16));
        assert_eq!(notes, Vec::new());
    }
}
