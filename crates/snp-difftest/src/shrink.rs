//! Shrinking a diverging scenario and writing its `repro.sh` (ARCHITECTURE §13.2, research 10 §7.4.7).
//!
//! [`shrink`] reduces a scenario while a predicate ("still shows the same kind of difference") holds, in two
//! phases repeated to a fixpoint within a run budget:
//!
//! 1. **Model shrinking** (proptest-style, over the scenario model): drop whole steps, hook repositories the config
//!    no longer names, step environment entries, ttys and signals, unstaged edits, symlinks, modes, setup
//!    operations, git config entries — each list reduced by delta debugging, so a list of `n` items costs
//!    `O(log n)` runs when one item matters.
//! 2. **Delta debugging** (`ddmin`, Zeller & Hildebrandt) over the work files (removed together with their
//!    `commit`/`stage`/`exec` entries), the lines of `.pre-commit-config.yaml` (generated configs put one hook per
//!    line, so removing a line removes a hook) and the lines of each file's contents.
//!
//! [`repro_script`] renders a self-contained POSIX `repro.sh`: it materialises the scenario at the same root with
//! the same hermetic environment and commit dates, runs every step under the oracle and under snp, writes each
//! side's streams and exit codes, applies the Tier-C masks (durations, patch names) and `diff -ru`s the two sides
//! (exit 1 when they differ).
//!
//! Owner: WP-35.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::materialise::{BASE_DATE, Layout, fixture_dir};
use crate::runner::Runner;
use crate::scenario::{Brand, Op, Scenario, StepKind};
use crate::text::{sh_quote, sh_quote_bytes};

/// The result of [`shrink`].
#[derive(Debug, Clone)]
pub struct Shrunk {
    /// The smallest scenario found that still satisfies the predicate.
    pub scenario: Scenario,
    /// Predicate evaluations spent.
    pub runs: usize,
    /// Whether the run budget ran out before the fixpoint.
    pub budget_exhausted: bool,
}

/// Hooks in a scenario's config (`id:` occurrences): the size measure of the shrinker's acceptance test.
pub fn hook_count(s: &Scenario) -> usize {
    s.work
        .config
        .as_deref()
        .map_or(0, |c| c.matches("id:").count())
}

/// `ddmin`: a 1-minimal sublist of `items` for which `test` holds (assuming it holds for `items`).
pub fn ddmin<T: Clone>(items: &[T], test: &mut dyn FnMut(&[T]) -> bool) -> Vec<T> {
    let mut cur: Vec<T> = items.to_vec();
    let mut n = 2_usize;
    while cur.len() >= 2 {
        let chunk = cur.len().div_ceil(n).max(1);
        let mut reduced = false;
        // Try each complement (remove one chunk).
        let mut start = 0_usize;
        while start < cur.len() {
            let end = start.saturating_add(chunk).min(cur.len());
            let mut cand: Vec<T> = cur.get(..start).unwrap_or_default().to_vec();
            cand.extend_from_slice(cur.get(end..).unwrap_or_default());
            if test(&cand) {
                cur = cand;
                n = n.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
            start = end;
        }
        if !reduced {
            if n >= cur.len() {
                break;
            }
            n = n.saturating_mul(2).min(cur.len());
        }
    }
    if cur.len() == 1 && test(&[]) {
        cur.clear();
    }
    cur
}

/// Budgeted predicate.
struct Budget<'a> {
    pred: &'a mut dyn FnMut(&Scenario) -> bool,
    runs: usize,
    max: usize,
}

impl Budget<'_> {
    fn test(&mut self, s: &Scenario) -> bool {
        if self.runs >= self.max {
            return false;
        }
        self.runs = self.runs.saturating_add(1);
        (self.pred)(s)
    }
}

/// Reduces `s` while `still_fails` holds, spending at most `max_runs` evaluations. `still_fails(s)` is assumed
/// to hold for the input.
pub fn shrink(
    s: &Scenario,
    still_fails: &mut dyn FnMut(&Scenario) -> bool,
    max_runs: usize,
) -> Shrunk {
    let mut b = Budget {
        pred: still_fails,
        runs: 0,
        max: max_runs,
    };
    let mut cur = s.clone();
    loop {
        let before = cur.to_yaml();
        cur = model_pass(cur, &mut b);
        cur = dd_pass(cur, &mut b);
        if cur.to_yaml() == before || b.runs >= b.max {
            break;
        }
    }
    let budget_exhausted = b.runs >= b.max;
    Shrunk {
        scenario: cur,
        runs: b.runs,
        budget_exhausted,
    }
}

/// Reduces one list field of the scenario with `ddmin`.
fn reduce_list<T: Clone>(
    cur: Scenario,
    b: &mut Budget<'_>,
    get: fn(&Scenario) -> Vec<T>,
    set: fn(&mut Scenario, Vec<T>),
    min_len: usize,
) -> Scenario {
    let items = get(&cur);
    if items.len() <= min_len {
        return cur;
    }
    let base = cur.clone();
    let kept = ddmin(&items, &mut |cand: &[T]| {
        if cand.len() < min_len {
            return false;
        }
        let mut s = base.clone();
        set(&mut s, cand.to_vec());
        b.test(&s)
    });
    let mut out = cur;
    if kept.len() < items.len() {
        set(&mut out, kept);
    }
    out
}

fn model_pass(cur: Scenario, b: &mut Budget<'_>) -> Scenario {
    let mut cur = reduce_list(cur, b, |s| s.steps.clone(), |s, v| s.steps = v, 1);
    // Per-step simplifications.
    for i in 0..cur.steps.len() {
        let simpler: Vec<fn(&mut crate::scenario::Step)> = vec![
            |st| st.env.clear(),
            |st| st.tty = false,
            |st| st.signal = None,
            |st| st.stdin.clear(),
            |st| st.cwd = None,
        ];
        for f in simpler {
            let mut cand = cur.clone();
            if let Some(st) = cand.steps.get_mut(i) {
                let before = st.clone();
                f(st);
                if *st != before && b.test(&cand) {
                    cur = cand;
                }
            }
        }
    }
    let mut cur = reduce_list(cur, b, |s| s.repos.clone(), |s, v| s.repos = v, 0);
    cur = reduce_list(
        cur,
        b,
        |s| s.work.unstaged.clone(),
        |s, v| s.work.unstaged = v,
        0,
    );
    cur = reduce_list(
        cur,
        b,
        |s| s.work.symlinks.clone(),
        |s, v| s.work.symlinks = v,
        0,
    );
    cur = reduce_list(cur, b, |s| s.work.modes.clone(), |s, v| s.work.modes = v, 0);
    cur = reduce_list(cur, b, |s| s.work.chmod.clone(), |s, v| s.work.chmod = v, 0);
    cur = reduce_list(cur, b, |s| s.work.setup.clone(), |s, v| s.work.setup = v, 0);
    cur = reduce_list(
        cur,
        b,
        |s| s.work.git_config.clone(),
        |s, v| s.work.git_config = v,
        0,
    );
    cur = reduce_list(cur, b, |s| s.work.fifos.clone(), |s, v| s.work.fifos = v, 0);
    cur = reduce_list(cur, b, |s| s.work.exec.clone(), |s, v| s.work.exec = v, 0);
    cur = reduce_list(cur, b, |s| s.work.stage.clone(), |s, v| s.work.stage = v, 0);
    for i in 0..cur.steps.len() {
        let base = cur.clone();
        let env = base.steps.get(i).map_or_default(|st| st.env.clone());
        if env.is_empty() {
            continue;
        }
        let kept = ddmin(&env, &mut |cand| {
            let mut s = base.clone();
            if let Some(st) = s.steps.get_mut(i) {
                st.env = cand.to_vec();
            }
            b.test(&s)
        });
        if kept.len() < env.len()
            && let Some(st) = cur.steps.get_mut(i)
        {
            st.env = kept;
        }
    }
    cur
}

/// Removes a work file together with its `commit`, `stage`, `exec`, `modes`, `chmod` and `unstaged` entries.
fn with_files(s: &Scenario, files: &[(Vec<u8>, Vec<u8>)]) -> Scenario {
    let mut out = s.clone();
    let keep = |p: &[u8]| files.iter().any(|(f, _)| f == p) || p == b".pre-commit-config.yaml";
    out.work.files = files.to_vec();
    out.work.commit.retain(|p| keep(p));
    out.work.stage.retain(|p| keep(p));
    out.work.exec.retain(|p| keep(p.as_bytes()));
    out.work.modes.retain(|(p, _)| keep(p.as_bytes()));
    out.work.chmod.retain(|(p, _)| keep(p.as_bytes()));
    out.work.unstaged.retain(|(p, _)| keep(p.as_bytes()));
    out
}

fn split_lines(s: &[u8]) -> Vec<Vec<u8>> {
    crate::text::lines_with_ends(s)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect()
}

fn dd_pass(cur: Scenario, b: &mut Budget<'_>) -> Scenario {
    let mut cur = cur;
    // Files.
    if !cur.work.files.is_empty() {
        let base = cur.clone();
        let files = base.work.files.clone();
        let kept = ddmin(&files, &mut |cand| b.test(&with_files(&base, cand)));
        if kept.len() < files.len() {
            cur = with_files(&cur, &kept);
        }
    }
    // Config lines.
    if let Some(cfg) = cur.work.config.clone() {
        let lines = split_lines(cfg.as_bytes());
        if lines.len() > 1 {
            let base = cur.clone();
            let kept = ddmin(&lines, &mut |cand| {
                let mut s = base.clone();
                s.work.config = Some(String::from_utf8_lossy(&cand.concat()).into_owned());
                b.test(&s)
            });
            if kept.len() < lines.len() {
                cur.work.config = Some(String::from_utf8_lossy(&kept.concat()).into_owned());
            }
        }
    }
    // File contents, line by line.
    for i in 0..cur.work.files.len() {
        let Some((_, content)) = cur.work.files.get(i) else {
            continue;
        };
        let lines = split_lines(content);
        if lines.len() < 2 {
            continue;
        }
        let base = cur.clone();
        let kept = ddmin(&lines, &mut |cand| {
            let mut s = base.clone();
            if let Some(f) = s.work.files.get_mut(i) {
                f.1 = cand.concat();
            }
            b.test(&s)
        });
        if kept.len() < lines.len()
            && let Some(f) = cur.work.files.get_mut(i)
        {
            f.1 = kept.concat();
        }
    }
    cur
}

// ---------------------------------------------------------------------------------------------------------------
// repro.sh

/// The shell variable holding a repository's rev (`REV_<sanitised name>`).
fn rev_var(name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("REV_{n}")
}

/// Pre-substitutes every `${…}` the script knows statically; `${rev:x}` becomes `@@REV_x@@` (filled at run time by
/// `fill_revs`), `${probe}` / `${probe_path}` / `${tool}` become shell expansions.
fn static_subst(s: &str, vars: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        let (head, tail) = rest.split_at_checked(start).unwrap_or((rest, ""));
        out.push_str(head);
        let tail = tail.get(2..).unwrap_or_default();
        let Some(end) = tail.find('}') else {
            out.push_str("${");
            rest = tail;
            continue;
        };
        let (name, after) = tail.split_at_checked(end).unwrap_or((tail, ""));
        if let Some((_, v)) = vars.iter().find(|(k, _)| k == name) {
            out.push_str(v);
        } else if let Some(repo) = name.strip_prefix("rev:") {
            write!(out, "@@{}@@", rev_var(repo)).unwrap_or_default();
        } else {
            write!(out, "${{{name}}}").unwrap_or_default();
        }
        rest = after.get(1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn emit_write(o: &mut String, dir: &str, rel: &[u8], content: &[u8], vars: &[(String, String)]) {
    let content: Vec<u8> = match std::str::from_utf8(content) {
        Ok(t) => static_subst(t, vars).into_bytes(),
        Err(_) => content.to_vec(),
    };
    let rel_q = sh_quote_bytes(rel);
    writeln!(o, "  wr {} {rel_q} {}", sh_quote(dir), printf_arg(&content)).unwrap_or_default();
}

/// A `printf '%b'`-safe octal-escaped argument for arbitrary bytes.
fn printf_arg(b: &[u8]) -> String {
    let mut s = String::from("'");
    for &c in b {
        if c.is_ascii_alphanumeric() || b" .,:;_-+=/@#%^&*()[]{}<>?!~|\"".contains(&c) {
            s.push(char::from(c));
        } else {
            write!(s, "\\0{c:03o}").unwrap_or_default();
        }
    }
    s.push('\'');
    s
}

fn emit_ops(o: &mut String, dir: &str, ops: &[Op], vars: &[(String, String)]) {
    for op in ops {
        match op {
            Op::Git(a) => {
                let args: Vec<String> =
                    a.iter().map(|x| sh_quote(&static_subst(x, vars))).collect();
                writeln!(o, "  (cd {} && g {})", sh_quote(dir), args.join(" ")).unwrap_or_default();
            }
            Op::Write(p, c) => emit_write(o, dir, p.as_bytes(), c, vars),
            Op::Remove(p) => {
                writeln!(o, "  rm -rf {}/{}", sh_quote(dir), sh_quote(p)).unwrap_or_default();
            }
            Op::Shell(c) => {
                writeln!(
                    o,
                    "  (cd {} && hermetic sh -c {})",
                    sh_quote(dir),
                    sh_quote(&static_subst(c, vars))
                )
                .unwrap_or_default();
            }
            Op::Commit(m) => {
                writeln!(
                    o,
                    "  (cd {} && g add -A && g commit -q --allow-empty -m {})",
                    sh_quote(dir),
                    sh_quote(m)
                )
                .unwrap_or_default();
            }
            Op::Tag(t) => {
                writeln!(o, "  (cd {} && g tag {})", sh_quote(dir), sh_quote(t))
                    .unwrap_or_default();
            }
        }
    }
}

/// A self-contained POSIX shell script reproducing `s` at `root` with the runner's binaries (see the module docs).
#[expect(
    clippy::many_single_char_names,
    reason = "s(cenario), r(unner), l(ayout), o(utput), w(ork): the script renderer's vocabulary"
)]
pub fn repro_script(s: &Scenario, r: &Runner, root: &Path) -> String {
    let l = Layout::new(root, &s.work);
    let mut vars = vec![
        (String::from("root"), slash(&l.root)),
        (String::from("work"), slash(&l.work)),
        (String::from("home"), slash(&l.home)),
        (String::from("store"), slash(&l.store)),
        (String::from("snp_store"), slash(&l.snp_store)),
        (String::from("probe"), String::from("$PROBE")),
        (String::from("probe_path"), String::from("$PROBE")),
        (String::from("tool"), String::from("$TOOL")),
        (String::from("oracle"), String::from("\"$ORACLE_BIN\"")),
        (
            String::from("z40"),
            String::from("0000000000000000000000000000000000000000"),
        ),
        (
            String::from("z64"),
            String::from("0000000000000000000000000000000000000000000000000000000000000000"),
        ),
    ];
    let repo_dir = |name: &str, path: Option<&str>| -> PathBuf {
        path.map_or_else(|| l.repos.join(name), |p| l.root.join(p))
    };
    for rp in &s.repos {
        if let Some((url, rev)) = &rp.external {
            vars.push((format!("repo:{}", rp.name), url.clone()));
            vars.push((format!("url:{}", rp.name), url.clone()));
            vars.push((format!("rev:{}", rp.name), rev.clone()));
            continue;
        }
        let d = slash(&repo_dir(&rp.name, rp.path.as_deref()));
        vars.push((format!("repo:{}", rp.name), d.clone()));
        vars.push((format!("url:{}", rp.name), format!("file://{d}")));
    }
    let mut o = String::new();
    writeln!(o, "#!/bin/sh").unwrap_or_default();
    writeln!(
        o,
        "# repro.sh: generated by snp-difftest for scenario {:?}.",
        s.name
    )
    .unwrap_or_default();
    writeln!(o, "# Materialises the scenario at a fixed root, runs it under the oracle and under snp, and diffs").unwrap_or_default();
    writeln!(
        o,
        "# the streams and exit codes (Tier-C masks applied). Exit 1 = the sides differ."
    )
    .unwrap_or_default();
    writeln!(
        o,
        "# Override BRAND=compat|snp, SNP_BIN, ORACLE_BIN, ORACLE_SNP_PROG, PROBE, OUT."
    )
    .unwrap_or_default();
    writeln!(o, "# Not reproduced here: ptys (tty steps run on pipes), step deadlines, the store comparison.").unwrap_or_default();
    if !s.projects.is_empty() {
        writeln!(
            o,
            "# `projects:` only the work directory {:?} is materialised and run.",
            s.work.dir.as_deref().unwrap_or("work")
        )
        .unwrap_or_default();
    }
    o.push_str("set -u\n");
    writeln!(o, "ROOT={}", sh_quote(&slash(&l.root))).unwrap_or_default();
    writeln!(o, "SNP_BIN=${{SNP_BIN:-{}}}", sh_quote(&slash(&r.snp_bin))).unwrap_or_default();
    writeln!(
        o,
        "ORACLE_BIN=${{ORACLE_BIN:-{}}}",
        sh_quote(&slash(&r.oracle_bin))
    )
    .unwrap_or_default();
    writeln!(
        o,
        "ORACLE_SNP_PROG=${{ORACLE_SNP_PROG:-{}}}",
        sh_quote(&slash(&r.oracle_snp_prog))
    )
    .unwrap_or_default();
    let probe = r
        .snp_bin
        .parent()
        .map_or_default(|d| slash(&d.join("snp-probe")));
    writeln!(o, "PROBE=${{PROBE:-{}}}", sh_quote(&probe)).unwrap_or_default();
    writeln!(
        o,
        "BRAND=${{BRAND:-{}}}",
        s.brands.first().copied().unwrap_or(Brand::Compat).name()
    )
    .unwrap_or_default();
    writeln!(o, "OUT=${{OUT:-$ROOT.repro}}").unwrap_or_default();
    writeln!(o, "FIXTURES={}", sh_quote(&slash(&fixture_dir("")))).unwrap_or_default();
    o.push_str(REPRO_PRELUDE);
    writeln!(o, "  BASE_DATE={BASE_DATE}").unwrap_or_default();
    o.push_str("  tick=0\n");
    for rp in s.repos.iter().filter(|r| r.external.is_none()) {
        let d = slash(&repo_dir(&rp.name, rp.path.as_deref()));
        let dq = sh_quote(&d);
        let mut init = String::from("g init -q");
        if rp.bare {
            init.push_str(" --bare");
        }
        if let Some(f) = &rp.object_format {
            write!(init, " --object-format={}", sh_quote(f)).unwrap_or_default();
        }
        writeln!(o, "  mkdir -p {dq} && (cd {dq} && {init} .)").unwrap_or_default();
        if rp.bare {
            continue;
        }
        if let Some(f) = &rp.fixture {
            writeln!(o, "  cp -R \"$FIXTURES\"/{}/. {dq}/", sh_quote(f)).unwrap_or_default();
        }
        for (p, c) in &rp.files {
            emit_write(&mut o, &d, p.as_bytes(), c, &vars);
            writeln!(o, "  fill_revs {dq}/{}", sh_quote(p)).unwrap_or_default();
        }
        for p in &rp.exec {
            writeln!(o, "  chmod +x {dq}/{}", sh_quote(p)).unwrap_or_default();
        }
        writeln!(o, "  (cd {dq} && g add -A)").unwrap_or_default();
        for p in &rp.exec {
            writeln!(
                o,
                "  (cd {dq} && g update-index --chmod=+x -- {})",
                sh_quote(p)
            )
            .unwrap_or_default();
        }
        writeln!(o, "  (cd {dq} && g commit -q --allow-empty -m 'Add hooks')").unwrap_or_default();
        for t in &rp.tags {
            writeln!(o, "  (cd {dq} && g tag {})", sh_quote(t)).unwrap_or_default();
        }
        emit_ops(&mut o, &d, &rp.then, &vars);
        writeln!(o, "  {}=$(cd {dq} && g rev-parse HEAD)", rev_var(&rp.name)).unwrap_or_default();
    }
    let w = &s.work;
    let wd = slash(&l.work);
    let wq = sh_quote(&wd);
    writeln!(o, "  mkdir -p {wq}").unwrap_or_default();
    if w.git {
        let mut init = String::from("g init -q");
        if let Some(f) = &w.object_format {
            write!(init, " --object-format={}", sh_quote(f)).unwrap_or_default();
        }
        writeln!(o, "  (cd {wq} && {init} .)").unwrap_or_default();
        for (k, v) in &w.git_config {
            writeln!(
                o,
                "  (cd {wq} && g config {} {})",
                sh_quote(k),
                sh_quote(&static_subst(v, &vars))
            )
            .unwrap_or_default();
        }
    }
    for (p, c) in &w.files {
        emit_write(&mut o, &wd, p, c, &vars);
        writeln!(o, "  fill_revs {wq}/{}", sh_quote_bytes(p)).unwrap_or_default();
    }
    if let Some(c) = &w.config {
        emit_write(&mut o, &wd, b".pre-commit-config.yaml", c.as_bytes(), &vars);
        writeln!(o, "  fill_revs {wq}/.pre-commit-config.yaml").unwrap_or_default();
    }
    for p in &w.exec {
        writeln!(o, "  chmod +x {wq}/{}", sh_quote(p)).unwrap_or_default();
    }
    for (link, target) in &w.symlinks {
        writeln!(
            o,
            "  ln -s {} {wq}/{}",
            sh_quote(&static_subst(target, &vars)),
            sh_quote(link)
        )
        .unwrap_or_default();
    }
    if w.git && !w.commit.is_empty() {
        let paths: Vec<String> = w.commit.iter().map(|p| sh_quote_bytes(p)).collect();
        writeln!(o, "  (cd {wq} && g add -- {})", paths.join(" ")).unwrap_or_default();
        for p in &w.exec {
            if w.commit.iter().any(|c| c == p.as_bytes()) {
                writeln!(
                    o,
                    "  (cd {wq} && g update-index --chmod=+x -- {})",
                    sh_quote(p)
                )
                .unwrap_or_default();
            }
        }
        writeln!(o, "  (cd {wq} && g commit -q -m init)").unwrap_or_default();
    }
    for (p, m) in &w.modes {
        writeln!(o, "  chmod {m:o} {wq}/{}", sh_quote(p)).unwrap_or_default();
    }
    if w.git && !w.stage.is_empty() {
        let paths: Vec<String> = w.stage.iter().map(|p| sh_quote_bytes(p)).collect();
        writeln!(o, "  (cd {wq} && g add -- {})", paths.join(" ")).unwrap_or_default();
    }
    for (p, c) in &w.unstaged {
        match c {
            Some(c) => emit_write(&mut o, &wd, p.as_bytes(), c, &vars),
            None => {
                writeln!(o, "  rm -f {wq}/{}", sh_quote(p)).unwrap_or_default();
            }
        }
    }
    for f in &w.fifos {
        writeln!(o, "  mkfifo {wq}/{}", sh_quote(f)).unwrap_or_default();
    }
    emit_ops(&mut o, &wd, &w.setup, &vars);
    for (p, m) in &w.chmod {
        writeln!(o, "  chmod {m:o} {wq}/{}", sh_quote(p)).unwrap_or_default();
    }
    o.push_str("}\n\nrun_side() {\n  side=$1; prog=$2\n  materialise\n");
    o.push_str("  ln -s \"$prog\" \"$ROOT/bin/$NAME\"\n  TOOL=\"$ROOT/bin/$NAME\"\n  mkdir -p \"$OUT/$side\"\n");
    for (i, st) in s.steps.iter().enumerate() {
        let n = i.saturating_add(1);
        let mut env = String::new();
        let mut unset = String::new();
        for (k, v) in &st.env {
            match v {
                Some(v) if k == "PATH_PREPEND" => {
                    write!(
                        env,
                        " PATH={}:\"$ROOT/bin:$PATH\"",
                        sh_quote(&static_subst(v, &vars))
                    )
                    .unwrap_or_default();
                }
                Some(v) => {
                    write!(env, " {k}={}", sh_quote(&static_subst(v, &vars))).unwrap_or_default();
                }
                None => {
                    write!(unset, " -u {k}").unwrap_or_default();
                }
            }
        }
        let cmd = match &st.kind {
            StepKind::Tool(a) => {
                let args: Vec<String> = a
                    .iter()
                    .map(|x| sh_quote(&static_subst(x, &vars)))
                    .collect();
                format!("\"$TOOL\" {}", args.join(" "))
            }
            StepKind::Git(a) => {
                let args: Vec<String> = a
                    .iter()
                    .map(|x| sh_quote(&static_subst(x, &vars)))
                    .collect();
                format!("git {}", args.join(" "))
            }
            StepKind::Shell(c) => format!("sh -c {}", sh_quote(&static_subst(c, &vars))),
        };
        let cwd = st
            .cwd
            .as_ref()
            .map_or_else(|| wd.clone(), |c| format!("{wd}/{c}"));
        writeln!(o, "  # step {n}").unwrap_or_default();
        writeln!(o, "  tick=$((tick + 1)); d=\"$((BASE_DATE + tick)) +0000\"").unwrap_or_default();
        let stdin = if st.stdin.is_empty() {
            String::from("</dev/null")
        } else {
            format!("< \"$OUT/$side/stdin-{n}\"")
        };
        if !st.stdin.is_empty() {
            writeln!(
                o,
                "  printf '%b' {} > \"$OUT/$side/stdin-{n}\"",
                printf_arg(&st.stdin)
            )
            .unwrap_or_default();
        }
        let run = format!(
            "(cd {} && hermetic env{unset} GIT_AUTHOR_DATE=\"$d\" GIT_COMMITTER_DATE=\"$d\" PRE_COMMIT=\"$TOOL\"{env} {cmd}) {stdin} > \"$OUT/$side/step-{n}.stdout\" 2> \"$OUT/$side/step-{n}.stderr\"",
            sh_quote(&cwd)
        );
        match &st.signal {
            Some(sig) => {
                writeln!(o, "  {run} &\n  pid=$!").unwrap_or_default();
                if let Some(f) = &sig.after_file {
                    writeln!(
                        o,
                        "  n=0; while [ ! -e {}/{} ] && [ $n -lt 1200 ]; do sleep 0.05; n=$((n + 1)); done",
                        sh_quote(&cwd),
                        sh_quote(f)
                    ).unwrap_or_default();
                }
                writeln!(
                    o,
                    "  sleep {}.{:03}; kill -{} \"$pid\" 2>/dev/null; wait \"$pid\"; echo $? > \"$OUT/$side/step-{n}.exit\"",
                    sig.after_ms / 1000,
                    sig.after_ms % 1000,
                    sig.name.trim_start_matches("SIG")
                ).unwrap_or_default();
            }
            None => {
                writeln!(o, "  {run}; echo $? > \"$OUT/$side/step-{n}.exit\"").unwrap_or_default();
            }
        }
    }
    o.push_str("  mask \"$OUT/$side\"\n}\n\n");
    o.push_str(REPRO_MAIN);
    o
}

const REPRO_PRELUDE: &str = r#"case "$BRAND" in
  compat) NAME=pre-commit; ORACLE="$ORACLE_BIN" ;;
  snp) NAME=snp; ORACLE="$ORACLE_SNP_PROG" ;;
  *) echo "BRAND must be compat or snp" >&2; exit 2 ;;
esac

hermetic() {
  env -i PATH="$ROOT/bin:$PATH" HOME="$ROOT/home" PRE_COMMIT_HOME="$ROOT/store" SNP_HOME="$ROOT/snp-store" \
    GIT_CONFIG_GLOBAL="$ROOT/home/.gitconfig" TMPDIR="$ROOT/tmp" GIT_CEILING_DIRECTORIES="$(dirname "$ROOT")" \
    XDG_CACHE_HOME="$ROOT/home/.cache" GIT_TEMPLATE_DIR="$ROOT/git-template" GIT_CONFIG_NOSYSTEM=1 \
    GIT_AUTHOR_NAME=test GIT_COMMITTER_NAME=test GIT_AUTHOR_EMAIL=test@example.com \
    GIT_COMMITTER_EMAIL=test@example.com GIT_AUTHOR_DATE="$BASE_DATE +0000" GIT_COMMITTER_DATE="$BASE_DATE +0000" \
    GIT_ALLOW_PROTOCOL=file LC_ALL=C.UTF-8 LANG=C.UTF-8 TZ=UTC VIRTUALENV_NO_DOWNLOAD=1 PYTHONHASHSEED=0 "$@"
}

g() {
  case "$1" in commit|merge|tag|cherry-pick|rebase|revert|stash|am) tick=$((tick + 1)) ;; esac
  d="$((BASE_DATE + tick)) +0000"
  hermetic env GIT_AUTHOR_DATE="$d" GIT_COMMITTER_DATE="$d" git "$@"
}

wr() {
  mkdir -p "$(dirname "$1/$2")" && printf '%b' "$3" > "$1/$2"
}

fill_revs() {
  [ -f "$1" ] || return 0
  for v in $(set | sed -n 's/^\(REV_[A-Za-z0-9_]*\)=.*/\1/p'); do
    eval "val=\$$v"
    sed "s/@@$v@@/$val/g" "$1" > "$1.repro-tmp" && cat "$1.repro-tmp" > "$1" && rm -f "$1.repro-tmp"
  done
}

mask() {
  for f in "$1"/*.stdout "$1"/*.stderr; do
    [ -f "$f" ] || continue
    sed -e 's/- duration: [0-9.]*s/- duration: [D]s/' -e 's/patch[0-9][0-9]*-[0-9][0-9]*/patch[TP]/g' \
      -e "s#$ROOT/snp-store#[STORE]#g" -e "s#$ROOT/store#[STORE]#g" "$f" > "$f.m" && mv "$f.m" "$f"
  done
}

materialise() {
  if [ -e "$ROOT" ] && [ ! -e "$ROOT/.snp-difftest-root" ]; then
    echo "refusing to delete $ROOT: not a difftest root" >&2; exit 2
  fi
  chmod -R u+rwx "$ROOT" 2>/dev/null; rm -rf "$ROOT"
  mkdir -p "$ROOT/home/.cache" "$ROOT/store" "$ROOT/snp-store" "$ROOT/tmp" "$ROOT/bin" "$ROOT/repos" \
    "$ROOT/git-template"
  : > "$ROOT/.snp-difftest-root"
  printf '[init]\n\tdefaultBranch = master\n[user]\n\tname = test\n\temail = test@example.com\n[commit]\n\tgpgsign = false\n[tag]\n\tgpgsign = false\n[core]\n\tautocrlf = false\n\tfsmonitor = false\n[protocol "file"]\n\tallow = always\n[advice]\n\tdetachedHead = false\n[safe]\n\tdirectory = *\n' > "$ROOT/home/.gitconfig"
"#;

const REPRO_MAIN: &str = r#"rm -rf "$OUT"
run_side oracle "$ORACLE"
run_side snp "$SNP_BIN"
if diff -ru "$OUT/oracle" "$OUT/snp"; then
  echo "repro: no difference"
  exit 0
fi
echo "repro: the sides differ (outputs in $OUT)"
exit 1
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddmin_finds_the_culprits() {
        let items: Vec<u32> = (0..64).collect();
        let mut runs = 0;
        let kept = ddmin(&items, &mut |c: &[u32]| {
            runs += 1;
            c.contains(&17) && c.contains(&42)
        });
        assert_eq!(kept, vec![17, 42]);
        assert!(runs < 200, "{runs}");
        let none = ddmin(&items, &mut |_: &[u32]| true);
        assert_eq!(none, Vec::<u32>::new());
        let one = ddmin(&[1_u8], &mut |c: &[u8]| !c.is_empty());
        assert_eq!(one, vec![1]);
    }

    #[test]
    fn shrinks_files_hooks_and_steps() {
        let mut s = Scenario::new("x");
        s.work.files = (0..40)
            .map(|i| {
                (
                    format!("f{i}.txt").into_bytes(),
                    format!("a\nb{i}\nc\n").into_bytes(),
                )
            })
            .collect();
        s.work.stage = s.work.files.iter().map(|(p, _)| p.clone()).collect();
        let mut cfg = String::from("repos:\n-   repo: local\n    hooks:\n");
        for i in 0..12 {
            writeln!(
                cfg,
                "    - {{id: h{i}, name: h{i}, entry: x, language: system}}"
            )
            .unwrap_or_default();
        }
        s.work.config = Some(cfg);
        s.steps = (0..5)
            .map(|_| crate::scenario::Step {
                kind: StepKind::Tool(vec![String::from("run")]),
                env: vec![(String::from("SKIP"), Some(String::from("x")))],
                stdin: Vec::new(),
                tty: false,
                timeout_ms: None,
                cwd: None,
                signal: None,
            })
            .collect();
        // The "divergence": file f7.txt containing b7 staged, and hook h3 configured.
        let mut pred = |c: &Scenario| {
            c.work
                .files
                .iter()
                .any(|(p, b)| p == b"f7.txt" && crate::text::contains_bytes(b, b"b7"))
                && c.work.stage.iter().any(|p| p == b"f7.txt")
                && c.work
                    .config
                    .as_deref()
                    .is_some_and(|t| t.contains("id: h3,"))
                && !c.steps.is_empty()
        };
        let out = shrink(&s, &mut pred, 5000);
        assert!(!out.budget_exhausted);
        assert_eq!(
            out.scenario.work.files,
            vec![(b"f7.txt".to_vec(), b"b7\n".to_vec())]
        );
        assert_eq!(hook_count(&out.scenario), 1);
        assert_eq!(out.scenario.steps.len(), 1);
        assert_eq!(out.scenario.steps[0].env, Vec::new());
    }

    #[test]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "`${…}` is the materialiser's substitution syntax"
    )]
    fn static_substitution() {
        let vars = vec![
            (String::from("work"), String::from("/r/work")),
            (String::from("repo:h"), String::from("/r/repos/h")),
        ];
        assert_eq!(
            static_subst("${work}/a ${repo:h} ${rev:h} ${nope}", &vars),
            "/r/work/a /r/repos/h @@REV_h@@ ${nope}"
        );
        assert_eq!(printf_arg(b"a'b\n\xff"), "'a\\0047b\\0012\\0377'");
    }
}
