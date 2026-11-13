//! lefthook's configuration loader (05 §2.1–§2.5; `internal/config/loader.go`): file lookup, `extends` with Go
//! `filepath.Glob`, koanf's merge with lefthook's `mergeHooks` / `mergeJobsSlice` / `{cmd}` rules, the local config,
//! and the per-hook final merge of `addHook`. Only reads files; remotes are never fetched (05 §19.1).
//!
//! Owner: WP-34.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::value::{self, Format, LMap, LValue};

/// lefthook's `{cmd}` merge placeholder (`hook.go:3`).
const CMD: &str = "{cmd}";

/// Main config base names (`loader.go:37`).
const MAIN_NAMES: [&str; 3] = ["lefthook", ".lefthook", ".config/lefthook"];
/// Local config base names (`loader.go:36`).
const LOCAL_NAMES: [&str; 3] = [
    "lefthook-local",
    ".lefthook-local",
    ".config/lefthook-local",
];
/// Extensions in lookup order (extension-major, `loader.go:89-102`).
const EXTENSIONS: [&str; 5] = [".yml", ".yaml", ".json", ".jsonc", ".toml"];

/// The 28 git hooks lefthook knows (`available_hooks.go:11-40`), in git-scm order.
pub(crate) const AVAILABLE_HOOKS: [&str; 28] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
    "fsmonitor-watchman",
    "p4-changelist",
    "p4-prepare-changelist",
    "p4-post-changelist",
    "p4-pre-submit",
    "post-index-change",
];

/// lefthook's global (non-hook) keys (`config.go:28-64`), never taken for custom hook names.
pub(crate) const GLOBAL_KEYS: [&str; 18] = [
    "min_version",
    "lefthook",
    "source_dir",
    "source_dir_local",
    "rc",
    "output",
    "colors",
    "extends",
    "no_tty",
    "assert_lefthook_installed",
    "skip_lfs",
    "no_auto_install",
    "install_non_git_hooks",
    "glob_matcher",
    "remotes",
    "templates",
    "ai",
    "$schema",
];

/// A loader failure (exit 1, nothing written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadError(pub(crate) String);

/// One lefthook hook after the final merge (`addHook`).
#[derive(Debug, Clone)]
pub(crate) struct RawHook {
    /// The hook name (`pre-commit`, `test`, …).
    pub(crate) name: String,
    /// The merged hook map.
    pub(crate) map: LMap,
}

/// Everything loaded for one conversion.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    /// `main.Merge(secondary)` without the hooks: the global options.
    pub(crate) global: LMap,
    /// The hooks: known git hooks in [`AVAILABLE_HOOKS`] order, then custom hooks sorted by name.
    pub(crate) hooks: Vec<RawHook>,
    /// The files read, repository-relative where possible, in load order.
    pub(crate) files: Vec<String>,
    /// The main config's display name (empty for local-only mode).
    pub(crate) main: String,
    /// The local config's display name, when one was merged.
    pub(crate) local: Option<String>,
    /// Non-fatal loader notes (`possible recursion in extends …`).
    pub(crate) warnings: Vec<String>,
    /// The main config's `remotes` (never fetched).
    pub(crate) remotes: Vec<LValue>,
}

impl Loaded {
    /// Nothing loaded (local-only conversions start from an empty main config).
    pub(crate) fn empty() -> Self {
        Self {
            global: LMap::new(),
            hooks: Vec::new(),
            files: Vec::new(),
            main: String::new(),
            local: None,
            warnings: Vec::new(),
            remotes: Vec::new(),
        }
    }
}

/// Whether a main config exists under `root` (the lookup of [`load`] without `--config`).
pub(crate) fn has_main(root: &Path) -> bool {
    find_first(root, &MAIN_NAMES).is_some()
}

/// The display name of `p` (relative to `root` when inside it, `/` separators).
pub(crate) fn display(root: &Path, p: &Path) -> String {
    let rel = p.trim_prefix(root);
    let s = rel.to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// Reads and parses one config file (format by extension).
fn read_config(root: &Path, path: &Path) -> Result<LMap, LoadError> {
    let shown = display(root, path);
    let Some(format) = Format::of(path) else {
        return Err(LoadError(format!(
            "can't parse config '{shown}', file has unsupported or no extension\nhint: rename {shown} to {shown}.yml"
        )));
    };
    // A FIFO or device named `*.yml` would block `read_to_end` forever (lefthook hangs too); only regular files
    // (symlinks followed) are read.
    let md = std::fs::metadata(path).map_err(|e| LoadError(format!("{shown}: {e}")))?;
    if !md.is_file() {
        return Err(LoadError(format!("{shown}: not a regular file")));
    }
    let mut bytes = Vec::new();
    let cap = u64::try_from(value::MAX_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    std::fs::File::open(path)
        .and_then(|f| f.take(cap).read_to_end(&mut bytes))
        .map_err(|e| LoadError(format!("{shown}: {e}")))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| LoadError(format!("{shown}: the file is not valid UTF-8: {e}")))?;
    let text = text.trim_prefix('\u{feff}');
    value::parse(text, format).map_err(|e| LoadError(format!("{shown}: {e}")))
}

/// The first existing file among `names` × [`EXTENSIONS`], extension-major (`loadFirst`).
fn find_first(root: &Path, names: &[&str]) -> Option<PathBuf> {
    EXTENSIONS.iter().find_map(|ext| {
        names
            .iter()
            .map(|n| root.join(format!("{n}{ext}")))
            .find(|p| p.exists())
    })
}

/// Go's `%q` of a `[]string`: `["lefthook" ".lefthook" ".config/lefthook"]`.
fn go_quote_list(names: &[&str]) -> String {
    let inner: Vec<String> = names.iter().map(|n| value::go_quote(n)).collect();
    format!("[{}]", inner.join(" "))
}

/// The main config path: `--config` (relative to the root), else the lookup; `Ok(None)` = local-only mode.
fn locate_main(root: &Path, config: Option<&Path>) -> Result<Option<PathBuf>, LoadError> {
    if let Some(c) = config {
        let p = if c.is_absolute() {
            c.to_path_buf()
        } else {
            root.join(c)
        };
        if !p.exists() {
            return Err(LoadError(format!(
                "Config file \"{}\" not found!",
                p.display()
            )));
        }
        return Ok(Some(p));
    }
    if let Some(p) = find_first(root, &MAIN_NAMES) {
        return Ok(Some(p));
    }
    if find_first(root, &LOCAL_NAMES).is_some() {
        return Ok(None);
    }
    Err(LoadError(format!(
        "No config files with names {} have been found in \"{}\"",
        go_quote_list(&MAIN_NAMES),
        root.display()
    )))
}

/// The local config path, when one exists.
pub(crate) fn locate_local(root: &Path) -> Option<PathBuf> {
    find_first(root, &LOCAL_NAMES)
}

/// koanf `Strings(key)`: a list's items as strings (`%v`), anything else empty.
fn strings(m: &LMap, key: &str) -> Vec<String> {
    match m.get(key) {
        Some(LValue::List(items)) => items
            .iter()
            .map(|i| match i {
                LValue::Str(s) => s.clone(),
                other => value::weak_string(other, key).unwrap_or_default(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// koanf `maps.Merge(src, dest)`: maps merge recursively, everything else (lists included) is replaced.
pub(crate) fn maps_merge(src: LMap, dest: &mut LMap) {
    for (k, v) in src {
        match (dest.get_mut(&k), v) {
            (Some(LValue::Map(d)), LValue::Map(s)) => maps_merge(s, d),
            (Some(slot), v) => *slot = v,
            (None, v) => {
                dest.insert(k, v);
            }
        }
    }
}

/// `extractHookSlices`: hook name → its `key` list, for hooks whose value is a map with a list under `key`.
fn hook_slices(hooks: &LMap, key: &str) -> Vec<(String, Vec<LValue>)> {
    hooks
        .iter()
        .filter_map(|(name, h)| match h {
            LValue::Map(m) => match m.get(key) {
                Some(LValue::List(l)) => Some((name.clone(), l.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// lefthook's `mergeHooks` (`loader.go:536-583`): `jobs` merged by name, `setup` appended, the rest `maps.Merge`.
fn merge_hooks(src: LMap, dest: &mut LMap, budget: &mut usize) -> Result<(), LoadError> {
    let src_jobs = hook_slices(&src, "jobs");
    let mut dest_jobs: Vec<(String, Vec<LValue>)> = hook_slices(dest, "jobs");
    let src_setup = hook_slices(&src, "setup");
    let mut dest_setup: Vec<(String, Vec<LValue>)> = hook_slices(dest, "setup");
    if (src_jobs.is_empty() || dest_jobs.is_empty())
        && (src_setup.is_empty() || dest_setup.is_empty())
    {
        maps_merge(src, dest);
        return Ok(());
    }
    for (hook, new_jobs) in src_jobs {
        match dest_jobs.iter_mut().find(|(n, _)| *n == hook) {
            Some((_, old)) => *old = merge_jobs_slice(new_jobs, std::mem::take(old), budget)?,
            None => dest_jobs.push((hook, new_jobs)),
        }
    }
    for (hook, new_setup) in src_setup {
        match dest_setup.iter_mut().find(|(n, _)| *n == hook) {
            Some((_, old)) => old.extend(new_setup),
            None => dest_setup.push((hook, new_setup)),
        }
    }
    maps_merge(src, dest);
    for (name, jobs) in dest_jobs {
        if let Some(LValue::Map(h)) = dest.get_mut(&name) {
            h.insert("jobs".to_owned(), LValue::List(jobs));
        }
    }
    for (name, setup) in dest_setup {
        if let Some(LValue::Map(h)) = dest.get_mut(&name) {
            h.insert("setup".to_owned(), LValue::List(setup));
        }
    }
    Ok(())
}

/// How many bytes `{cmd}` substitutions may add over one whole load: each extends level can double a `run`
/// (`run: '{cmd}{cmd}'`), so without a budget 40 small files would ask for gigabytes.
pub(crate) const CMD_BUDGET: usize = value::MAX_BYTES;

/// `strings.ReplaceAll(run, "{cmd}", earlier)` charged against `budget`.
fn expand_cmd(
    run: &str,
    earlier: &str,
    job: &str,
    budget: &mut usize,
) -> Result<String, LoadError> {
    let n = run.matches(CMD).count();
    let grown = n
        .saturating_mul(earlier.len())
        .saturating_sub(n.saturating_mul(CMD.len()));
    if grown > *budget {
        return Err(LoadError(format!(
            "`{{cmd}}` expansion of job '{job}' makes the configuration larger than {CMD_BUDGET} bytes"
        )));
    }
    *budget = budget.saturating_sub(grown);
    Ok(run.replace(CMD, earlier))
}

/// `job["group"]["jobs"]` as a list, if present.
fn group_jobs(job: &LMap) -> Option<&Vec<LValue>> {
    match job.get("group") {
        Some(LValue::Map(g)) => match g.get("jobs") {
            Some(LValue::List(l)) => Some(l),
            _ => None,
        },
        _ => None,
    }
}

/// lefthook's `mergeJobsSlice(src, dest)` (`loader.go:612-701`): `dest` order kept; a `src` job whose `name`
/// equals a named `dest` job is deep-merged into it (`{cmd}` in its `run` replaced by the earlier `run`, group jobs
/// merged recursively); other map entries are appended; non-map entries are dropped.
pub(crate) fn merge_jobs_slice(
    src: Vec<LValue>,
    dest: Vec<LValue>,
    budget: &mut usize,
) -> Result<Vec<LValue>, LoadError> {
    let mut result: Vec<LMap> = Vec::new();
    let mut mergeable: HashMap<String, usize> = HashMap::new();
    for job in dest {
        if let LValue::Map(m) = job {
            if let Some(LValue::Str(name)) = m.get("name") {
                mergeable.insert(name.clone(), result.len());
            }
            result.push(m);
        }
    }
    for job in src {
        let LValue::Map(mut src_job) = job else {
            continue;
        };
        let target = match src_job.get("name") {
            Some(LValue::Str(name)) => mergeable.get(name).copied(),
            _ => None,
        };
        let Some(dest_job) = target.and_then(|i| result.get_mut(i)) else {
            result.push(src_job);
            continue;
        };
        let mut sub_jobs = group_jobs(dest_job).cloned().unwrap_or_default();
        let src_sub = group_jobs(&src_job).cloned().unwrap_or_default();
        if !sub_jobs.is_empty() && !src_sub.is_empty() {
            sub_jobs = merge_jobs_slice(src_sub, sub_jobs, budget)?;
        }
        if let (Some(LValue::Str(src_run)), Some(LValue::Str(dest_run))) =
            (src_job.get("run"), dest_job.get("run"))
        {
            let name = match src_job.get("name") {
                Some(LValue::Str(n)) => n.as_str(),
                _ => "",
            };
            let new_run = expand_cmd(src_run, dest_run, name, budget)?;
            src_job.insert("run".to_owned(), LValue::Str(new_run));
        }
        maps_merge(src_job, dest_job);
        if !sub_jobs.is_empty()
            && let Some(LValue::Map(g)) = dest_job.get_mut("group")
            && matches!(g.get("jobs"), Some(LValue::List(_)))
        {
            g.insert("jobs".to_owned(), LValue::List(sub_jobs));
        }
    }
    Ok(result.into_iter().map(LValue::Map).collect())
}

/// Go's `filepath.Match` error.
const BAD_PATTERN: &str = "syntax error in pattern";

/// Decodes one rune of `s` (`utf8.DecodeRuneInString`): `(rune, width)`, `(U+FFFD, 1)` on invalid input.
fn decode_rune(s: &[u8]) -> (char, usize) {
    let width = match s.first() {
        None => return ('\u{fffd}', 0),
        Some(b) if *b < 0x80 => 1,
        Some(b) if *b >= 0xf0 => 4,
        Some(b) if *b >= 0xe0 => 3,
        Some(_) => 2,
    };
    s.get(..width)
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|t| t.chars().next())
        .map_or(('\u{fffd}', 1), |c| (c, width))
}

/// `scanChunk`: leading stars, the chunk up to the next unbracketed `*`, the rest.
fn scan_chunk(pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    let mut p = pattern;
    while let Some((b'*', rest)) = p.split_first() {
        p = rest;
        star = true;
    }
    let mut in_range = false;
    let mut i = 0_usize;
    while let Some(&b) = p.get(i) {
        match b {
            b'\\' => {
                if i.saturating_add(1) < p.len() {
                    i = i.saturating_add(1);
                }
            }
            b'[' => in_range = true,
            b']' => in_range = false,
            b'*' if !in_range => break,
            _ => {}
        }
        i = i.saturating_add(1);
    }
    let (chunk, rest) = p.split_at_checked(i).unwrap_or((p, &[]));
    (star, chunk, rest)
}

/// `getEsc`: one possibly escaped character of a class.
fn get_esc(chunk: &[u8]) -> Result<(char, &[u8]), &'static str> {
    let mut c = chunk;
    match c.first() {
        None | Some(b'-' | b']') => return Err(BAD_PATTERN),
        Some(b'\\') => {
            c = c.get(1..).unwrap_or(&[]);
            if c.is_empty() {
                return Err(BAD_PATTERN);
            }
        }
        Some(_) => {}
    }
    let (r, n) = decode_rune(c);
    if r == '\u{fffd}' && n == 1 {
        return Err(BAD_PATTERN);
    }
    let rest = c.get(n..).unwrap_or(&[]);
    if rest.is_empty() {
        return Err(BAD_PATTERN);
    }
    Ok((r, rest))
}

/// `matchChunk`: `Ok(Some(rest))` on a match, `Ok(None)` on none, `Err` for a malformed chunk.
fn match_chunk<'a>(chunk: &[u8], s: &'a [u8]) -> Result<Option<&'a [u8]>, &'static str> {
    let mut chunk = chunk;
    let mut s = s;
    let mut failed = false;
    while let Some(&c0) = chunk.first() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match c0 {
            b'[' => {
                let mut r = '\0';
                if !failed {
                    let (rr, n) = decode_rune(s);
                    r = rr;
                    s = s.get(n..).unwrap_or(&[]);
                }
                chunk = chunk.get(1..).unwrap_or(&[]);
                let mut negated = false;
                if chunk.first() == Some(&b'^') {
                    negated = true;
                    chunk = chunk.get(1..).unwrap_or(&[]);
                }
                let mut matched = false;
                let mut nrange = 0_usize;
                loop {
                    if chunk.first() == Some(&b']') && nrange > 0 {
                        chunk = chunk.get(1..).unwrap_or(&[]);
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    if chunk.first() == Some(&b'-') {
                        let (h, rest) = get_esc(chunk.get(1..).unwrap_or(&[]))?;
                        hi = h;
                        chunk = rest;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange = nrange.saturating_add(1);
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s.first() == Some(&b'/') {
                        failed = true;
                    }
                    let (_, n) = decode_rune(s);
                    s = s.get(n..).unwrap_or(&[]);
                }
                chunk = chunk.get(1..).unwrap_or(&[]);
            }
            _ => {
                if c0 == b'\\' {
                    chunk = chunk.get(1..).unwrap_or(&[]);
                    if chunk.is_empty() {
                        return Err(BAD_PATTERN);
                    }
                }
                if !failed {
                    if chunk.first() != s.first() {
                        failed = true;
                    }
                    s = s.get(1..).unwrap_or(&[]);
                }
                chunk = chunk.get(1..).unwrap_or(&[]);
            }
        }
    }
    Ok(if failed { None } else { Some(s) })
}

/// Go's `filepath.Match(pattern, name)` on Unix (`*` and `?` never match `/`, `[^…]` negates, `\` escapes).
pub(crate) fn go_match(pattern: &str, name: &str) -> Result<bool, &'static str> {
    let mut pattern = pattern.as_bytes();
    let mut name = name.as_bytes();
    'outer: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            return Ok(!name.contains(&b'/'));
        }
        let first = match_chunk(chunk, name);
        if let Ok(Some(t)) = first
            && (t.is_empty() || !pattern.is_empty())
        {
            name = t;
            continue;
        }
        first?;
        if star {
            let mut i = 0_usize;
            while let Some(&b) = name.get(i) {
                if b == b'/' {
                    break;
                }
                let after = name.get(i.saturating_add(1)..).unwrap_or(&[]);
                match match_chunk(chunk, after) {
                    Ok(Some(t)) => {
                        if pattern.is_empty() && !t.is_empty() {
                            i = i.saturating_add(1);
                            continue;
                        }
                        name = t;
                        continue 'outer;
                    }
                    Ok(None) => {}
                    Err(e) => return Err(e),
                }
                i = i.saturating_add(1);
            }
        }
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            match_chunk(chunk, &[])?;
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

/// `hasMeta`: `*?[`, plus the `\\` escape on Unix (on Windows `\\` is a separator, not an escape).
fn has_meta(p: &str) -> bool {
    p.contains(['*', '?', '[']) || (cfg!(not(windows)) && p.contains('\\'))
}

/// The deepest directory recursion of [`go_glob`].
const MAX_GLOB_DEPTH: usize = 32;

/// The `go_glob` error when a load's extends patterns go past their [`GlobBudget`].
pub(crate) const TOO_MANY: &str = "matches too many paths";

/// What the extends globs of one load may still read and return. Without it, directory symlinks (`a -> .`,
/// `b -> .`) or `/proc/*/root` make `*/*/…` patterns grow exponentially in time and memory; lefthook has no bound.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GlobBudget {
    /// Directory entries left to read (`read_dir` items, over every pattern of the load).
    pub(crate) entries: usize,
    /// Matches left to return.
    pub(crate) matches: usize,
}

impl GlobBudget {
    /// The per-load budget: 16384 directory entries and 4096 matches.
    pub(crate) const fn new() -> Self {
        Self {
            entries: 16_384,
            matches: 4_096,
        }
    }
}

/// Go's `filepath.Glob` (afero's copy) run from the directory `base` (lefthook globs the extends path as written,
/// relative to the repository it runs in, so metacharacters in `base` itself are never read as a pattern): a
/// pattern without metacharacters is returned when it exists (`Lstat`); otherwise every directory level with
/// metacharacters is expanded, names sorted. Unreadable directories match nothing. Matches are spelled like the
/// pattern (relative to `base` when it is relative).
pub(crate) fn go_glob(
    base: &Path,
    pattern: &str,
    depth: usize,
    budget: &mut GlobBudget,
) -> Result<Vec<String>, &'static str> {
    go_match(pattern, "")?;
    if !has_meta(pattern) {
        return Ok(if std::fs::symlink_metadata(base.join(pattern)).is_ok() {
            charge(&mut budget.matches, 1)?;
            vec![pattern.to_owned()]
        } else {
            Vec::new()
        });
    }
    if depth > MAX_GLOB_DEPTH {
        return Err(BAD_PATTERN);
    }
    let (dir, file) = match pattern.rfind('/') {
        Some(i) => (
            pattern.get(..=i).unwrap_or(""),
            pattern.get(i.saturating_add(1)..).unwrap_or(""),
        ),
        None => ("", pattern),
    };
    let dir = match dir {
        "" => ".".to_owned(),
        "/" => "/".to_owned(),
        d => d.trim_suffix('/').to_owned(),
    };
    if !has_meta(&dir) {
        return glob_dir(base, &dir, file, Vec::new(), budget);
    }
    if dir == pattern {
        return Err(BAD_PATTERN);
    }
    let mut matches = Vec::new();
    for d in go_glob(base, &dir, depth.saturating_add(1), budget)? {
        matches = glob_dir(base, &d, file, matches, budget)?;
    }
    Ok(matches)
}

/// Takes `n` from `left`, or fails with [`TOO_MANY`] when it is spent.
fn charge(left: &mut usize, n: usize) -> Result<(), &'static str> {
    *left = left.checked_sub(n).ok_or(TOO_MANY)?;
    Ok(())
}

/// `glob(dir, pattern, matches)`: the sorted entries of `dir` matching `pattern`, each entry read and each match
/// charged to `budget`.
fn glob_dir(
    base: &Path,
    dir: &str,
    pattern: &str,
    mut matches: Vec<String>,
    budget: &mut GlobBudget,
) -> Result<Vec<String>, &'static str> {
    let full = base.join(dir);
    if !full.is_dir() {
        return Ok(matches);
    }
    let Ok(entries) = std::fs::read_dir(&full) else {
        return Ok(matches);
    };
    let mut names: Vec<String> = Vec::new();
    for e in entries {
        charge(&mut budget.entries, 1)?;
        if let Some(n) = e.ok().and_then(|e| e.file_name().into_string().ok()) {
            names.push(n);
        }
    }
    names.sort();
    for n in names {
        if go_match(pattern, &n)? {
            charge(&mut budget.matches, 1)?;
            let joined = if dir == "." {
                n
            } else if dir.ends_with('/') {
                format!("{dir}{n}")
            } else {
                format!("{dir}/{n}")
            };
            matches.push(joined);
        }
    }
    Ok(matches)
}

/// The loader state shared by one conversion.
struct Ctx<'a> {
    root: &'a Path,
    files: Vec<String>,
    warnings: Vec<String>,
    /// What `{cmd}` substitutions may still add ([`CMD_BUDGET`]).
    budget: usize,
    /// What the extends globs may still read and return.
    globs: GlobBudget,
}

impl Ctx<'_> {
    /// Loads `path` into `k` with lefthook's merge function.
    fn load_into(&mut self, k: &mut LMap, path: &Path) -> Result<(), LoadError> {
        let m = read_config(self.root, path)?;
        self.files.push(display(self.root, path));
        merge_hooks(m, k, &mut self.budget)
    }

    /// `extend` / `extendRecursive` (`loader.go:333-376`); a path seen twice is a warning and skipped (05 §19.2).
    fn extend(
        &mut self,
        k: &mut LMap,
        extends: &[String],
        visited: &mut Vec<String>,
        depth: usize,
    ) -> Result<(), LoadError> {
        if depth > MAX_GLOB_DEPTH {
            return Err(LoadError("extends are nested too deeply".to_owned()));
        }
        for p in extends {
            // Windows' `filepath.Glob` reads `\\` as a separator; globbing works on `/`.
            let pattern = if cfg!(windows) {
                p.replace('\\', "/")
            } else {
                p.clone()
            };
            let paths = go_glob(self.root, &pattern, 0, &mut self.globs).map_err(|e| {
                if e == TOO_MANY {
                    LoadError(format!("extends pattern '{p}' {TOO_MANY}"))
                } else {
                    LoadError(format!("bad glob syntax for '{p}': {e}"))
                }
            })?;
            for rel in paths {
                let path = self.root.join(&rel).to_string_lossy().into_owned();
                if visited.contains(&path) {
                    self.warnings.push(format!(
                        "possible recursion in extends: path {} is specified multiple times; skipped",
                        display(self.root, Path::new(&path))
                    ));
                    continue;
                }
                visited.push(path.clone());
                let mut extent = LMap::new();
                self.load_into(&mut extent, Path::new(&path))?;
                let nested = strings(&extent, "extends");
                self.extend(&mut extent, &nested, visited, depth.saturating_add(1))?;
                merge_hooks(extent, k, &mut self.budget)?;
            }
        }
        Ok(())
    }
}

/// `addHook` (`loader.go:418-492`): the final main ← secondary merge of one hook with the `{cmd}` replacement in
/// commands and `setup` prepended.
fn add_hook(
    main: Option<&LValue>,
    secondary: Option<&LValue>,
    budget: &mut usize,
) -> Result<LMap, LoadError> {
    let mut dest = match main {
        Some(LValue::Map(m)) => m.clone(),
        _ => LMap::new(),
    };
    let src = match secondary {
        Some(LValue::Map(m)) => m.clone(),
        _ => return Ok(dest),
    };
    let mut dest_commands: Vec<(String, String)> = Vec::new();
    if let Some(LValue::Map(cmds)) = dest.get("commands") {
        for (name, c) in cmds {
            if let LValue::Map(c) = c
                && let Some(LValue::Str(run)) = c.get("run")
            {
                dest_commands.push((name.clone(), run.clone()));
            }
        }
    }
    let list = |m: &LMap, k: &str| match m.get(k) {
        Some(LValue::List(l)) => l.clone(),
        _ => Vec::new(),
    };
    let dest_jobs = merge_jobs_slice(list(&src, "jobs"), list(&dest, "jobs"), budget)?;
    let mut setup = list(&src, "setup");
    setup.extend(list(&dest, "setup"));
    maps_merge(src, &mut dest);
    if !dest_commands.is_empty()
        && let Some(LValue::Map(cmds)) = dest.get_mut("commands")
    {
        for (name, c) in cmds.iter_mut() {
            if let LValue::Map(c) = c
                && let Some(LValue::Str(run)) = c.get_mut("run")
            {
                let earlier = dest_commands
                    .iter()
                    .find(|(n, _)| n == name)
                    .map_or("", |(_, r)| r.as_str());
                *run = expand_cmd(run, earlier, name, budget)?;
            }
        }
    }
    if !dest_jobs.is_empty() {
        dest.insert("jobs".to_owned(), LValue::List(dest_jobs));
    }
    if !setup.is_empty() {
        dest.insert("setup".to_owned(), LValue::List(setup));
    }
    Ok(dest)
}

/// Whether a top-level key is a custom hook (`^([^.]+)\.(?:scripts|commands|jobs)` over koanf's flattened keys).
fn is_custom_hook(name: &str, v: &LValue) -> bool {
    if name.contains('.') || AVAILABLE_HOOKS.contains(&name) || GLOBAL_KEYS.contains(&name) {
        return false;
    }
    v.as_map().is_some_and(|m| {
        m.keys()
            .any(|k| k.starts_with("scripts") || k.starts_with("commands") || k.starts_with("jobs"))
    })
}

/// Loads the configuration under `root` (`LoadKoanf` + `unmarshalConfigs`): main (or `config`), its extends, then
/// the local config and its extends when `with_local`.
pub(crate) fn load(
    root: &Path,
    config: Option<&Path>,
    with_local: bool,
) -> Result<Loaded, LoadError> {
    let mut cx = Ctx {
        root,
        files: Vec::new(),
        warnings: Vec::new(),
        budget: CMD_BUDGET,
        globs: GlobBudget::new(),
    };
    let main_path = locate_main(root, config)?;
    let local_path = locate_local(root);
    if main_path.is_none() && !with_local {
        return Err(LoadError(format!(
            "only the local config {} exists; convert it with --local",
            local_path.as_deref().map_or_default(|p| display(root, p))
        )));
    }
    let mut main = LMap::new();
    if let Some(p) = &main_path {
        cx.load_into(&mut main, p)?;
    }
    let extends = strings(&main, "extends");
    let remotes = match main.get("remotes") {
        Some(LValue::List(l)) => l.clone(),
        Some(LValue::Map(m)) => vec![LValue::Map(m.clone())],
        _ => Vec::new(),
    };
    let mut secondary = LMap::new();
    let mut visited = Vec::new();
    cx.extend(&mut secondary, &extends, &mut visited, 0)?;
    if strings(&secondary, "extends") != extends {
        secondary.shift_remove("extends");
    }
    secondary.shift_remove("lefthook");
    let mut local = None;
    if with_local && let Some(lp) = &local_path {
        cx.load_into(&mut secondary, lp)?;
        local = Some(display(root, lp));
        let local_extends = strings(&secondary, "extends");
        if local_extends != extends {
            let mut visited = Vec::new();
            cx.extend(&mut secondary, &local_extends, &mut visited, 0)?;
        }
    }
    let mut hooks = Vec::new();
    for name in AVAILABLE_HOOKS {
        if main.contains_key(name) || secondary.contains_key(name) {
            hooks.push(RawHook {
                name: name.to_owned(),
                map: add_hook(main.get(name), secondary.get(name), &mut cx.budget)?,
            });
        }
    }
    let mut custom: Vec<String> = main
        .iter()
        .chain(secondary.iter())
        .filter(|(k, v)| is_custom_hook(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    custom.sort();
    custom.dedup();
    for name in custom {
        let map = add_hook(main.get(&name), secondary.get(&name), &mut cx.budget)?;
        hooks.push(RawHook { name, map });
    }
    let hook_names: Vec<String> = hooks.iter().map(|h| h.name.clone()).collect();
    let mut global = main;
    maps_merge(secondary, &mut global);
    global.retain(|k, _| !hook_names.contains(k));
    Ok(Loaded {
        global,
        hooks,
        files: cx.files,
        main: main_path.map_or_default(|p| display(root, &p)),
        local,
        warnings: cx.warnings,
        remotes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_match_vectors() {
        assert_eq!(go_match("*.yml", "a.yml"), Ok(true));
        assert_eq!(go_match("*.yml", "a/b.yml"), Ok(false));
        assert_eq!(go_match("a?c", "abc"), Ok(true));
        assert_eq!(go_match("a?c", "a/c"), Ok(false));
        assert_eq!(go_match("[a-c]x", "bx"), Ok(true));
        assert_eq!(go_match("[^a-c]x", "bx"), Ok(false));
        assert_eq!(go_match("[^a-c]x", "dx"), Ok(true));
        assert_eq!(go_match("a\\*", "a*"), Ok(true));
        assert_eq!(go_match("[", "a"), Err(BAD_PATTERN));
        assert_eq!(go_match("a[", "b"), Err(BAD_PATTERN));
        assert_eq!(go_match("*x", "abx"), Ok(true));
        assert_eq!(go_match("a*b*c", "aXbYc"), Ok(true));
        assert_eq!(go_match("ab\\", "ab"), Err(BAD_PATTERN));
    }

    #[test]
    fn job_slices_merge_by_name() {
        let parse = |s: &str| match value::parse(s, Format::Yaml).unwrap().shift_remove("j") {
            Some(LValue::List(l)) => l,
            _ => panic!(),
        };
        let dest = parse("j: [{name: a, run: x}, {run: u}]");
        let src = parse("j: [{name: a, run: 'w {cmd}'}, {run: v}, {name: b, run: y}, 3]");
        let out = merge_jobs_slice(src, dest, &mut CMD_BUDGET.clone()).unwrap();
        let runs: Vec<_> = out
            .iter()
            .map(|j| j.as_map().unwrap()["run"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(runs, ["w x", "u", "v", "y"]);
    }

    /// lefthook's own integration fixtures: the merged tree equals `lefthook dump` (compared after typing, since the
    /// dump is already weakly typed: `glob: 3` is dumped as `- "3"`).
    #[test]
    fn merge_equals_lefthook_dump() {
        for fixture in ["job_merging", "many_extends_levels"] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(fixture);
            let loaded = load(&root, None, false).unwrap();
            let dump_text =
                std::fs::read_to_string(root.join("lefthook-dump.yml.expected")).unwrap();
            let dump = value::parse(&dump_text, Format::Yaml).unwrap();
            let names: Vec<&str> = loaded.hooks.iter().map(|h| h.name.as_str()).collect();
            assert_eq!(names, ["pre-commit", "e1", "e2", "e3"], "{fixture}");
            for h in &loaded.hooks {
                let want =
                    super::super::model::hook(&h.name, dump[&h.name].as_map().unwrap()).unwrap();
                let got = super::super::model::hook(&h.name, &h.map).unwrap();
                assert_eq!(
                    format!("{got:?}"),
                    format!("{want:?}"),
                    "{fixture}: {}",
                    h.name
                );
            }
            assert_eq!(
                loaded.files,
                [
                    "lefthook.yml",
                    "extends/e1.yml",
                    "extends/e2.yml",
                    "extends/e3.yml"
                ],
                "{fixture}"
            );
            assert_eq!(loaded.warnings, Vec::<String>::new());
        }
    }
}
