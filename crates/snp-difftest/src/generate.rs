//! The nightly generated suite (research 10 §7.4.6 item 2): random but reproducible scenarios.
//!
//! [`generate`] maps `(seed, index)` to one scenario through a `SplitMix64` stream, so a failing case is replayed
//! with the same two numbers (`snp-difftest run --suite generated --seed S --case I`). A scenario combines
//!
//! * a file tree: up to 40 files (rarely 120) named from a pool with spaces, unicode (`интервью`, `啊`), a leading
//!   `-`, `#` and `[`, in sub-directories; contents text (LF, CRLF, trailing whitespace, no final newline), binary
//!   with NUL bytes, empty, shell/python scripts with a shebang (executable or not), JSON and YAML; posix symlinks
//!   (also dangling);
//! * a git state: committed, staged, unstaged modifications and deletions, intent-to-add, staged deletions, a
//!   detached `HEAD`;
//! * a config: random top-level `files`/`exclude`/`fail_fast`/`default_stages`, 1–15 hooks across `local`
//!   (`system` entries running snp-probe — or `true`/`false`/`echo` without it — `pygrep`, `fail`), `meta` and the
//!   `script_hooks_repo` fixture over `file://`, with random `files`/`exclude` from a Python-compatible regex
//!   grammar, `types`/`types_or`/`exclude_types` from identify tags, `stages`, `always_run`, `pass_filenames`,
//!   `require_serial`, `verbose`, `fail_fast`, `alias`, `args` (spaces, quotes, `{}`, non-ASCII) and `log_file`;
//!   every hook is one flow mapping on one line, so the shrinker removes hooks by removing lines;
//! * one or two invocations: `run` with random flags (`-a`, `--files`, `--hook-stage`, a hook id or alias,
//!   `--from-ref/--to-ref`, `--show-diff-on-failure`, `--verbose`, `--color always|never`) and `SKIP`, or
//!   `install` followed by a `git commit`, or `validate-config`.
//!
//! Owner: WP-35.

use std::fmt::Write as _;

use crate::scenario::{Op, RepoSpec, Scenario, Step, StepKind};

/// `SplitMix64` (Steele, Lea, Flood 2014): small, fast, and the same on every platform.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator for `seed`.
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`0` when `n == 0`).
    pub fn below(&mut self, n: usize) -> usize {
        let n64 = u64::try_from(n).unwrap_or(u64::MAX);
        if n64 == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % n64).unwrap_or(0)
    }

    /// `true` with probability `num / den`.
    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        den != 0 && self.next_u64() % den < num
    }

    /// One element of `items`.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        items.get(self.below(items.len()))
    }
}

const NAMES: &[&str] = &[
    "a.py",
    "b.py",
    "setup.py",
    "README.md",
    "notes.txt",
    "with space.txt",
    "интервью.txt",
    "啊.py",
    "-dash.txt",
    "#hash.md",
    "[bracket].txt",
    "run.sh",
    "tool",
    "data.bin",
    "empty",
    "conf.json",
    "conf.yaml",
    "Makefile",
    "src/main.py",
    "src/util.py",
    "src/deep/x.txt",
    "docs/guide.md",
    "docs/с пробелом.md",
    "tests/test_a.py",
    "CRLF.txt",
    "nonl.txt",
];

const TAGS: &[&str] =
    &["text", "binary", "python", "shell", "executable", "non-executable", "symlink", "markdown", "json", "yaml", "file"];

const REGEXES: &[&str] = &[
    r"\.py$",
    r"\.txt$",
    r"^src/",
    r"^docs/",
    r"\.(md|txt)$",
    r"(^|/)test_",
    r"[a-c]\.py$",
    r"^(?!src/)",
    r"\.ya?ml$",
    r"интервью",
    r"^$",
    r".",
    r"\s",
    r"^\[",
];

const PYGREP: &[&str] = &[r"TODO", r"^import ", r"\s+$", r"[A-Z]{3}", r"(?i)fixme", r"\t", r"^#!", r"x{2,}", "\u{00e9}"];

fn content(r: &mut Rng, name: &str) -> (Vec<u8>, bool) {
    let mut exec = false;
    let b: Vec<u8> = match r.below(10) {
        0 => Vec::new(),
        1 => vec![0x00, 0x01, 0xff, 0xfe, b'\n', 0x00],
        2 => {
            exec = r.chance(2, 3);
            b"#!/bin/sh\necho hi\n".to_vec()
        }
        3 => {
            exec = r.chance(1, 2);
            b"#!/usr/bin/env python3\nimport os\nprint(os.getcwd())\n".to_vec()
        }
        4 => b"line one\r\nline two\r\n".to_vec(),
        5 => b"trailing   \nTODO: fix\n\ttab\n".to_vec(),
        6 => b"no final newline".to_vec(),
        7 => b"{\"a\": [1, 2], \"b\": null}\n".to_vec(),
        8 => "caf\u{e9} \u{65e5}\u{672c} \u{0438}\u{043d}\n".as_bytes().to_vec(),
        _ => {
            let mut s = String::new();
            for i in 0..r.below(8).saturating_add(1) {
                let _ = writeln!(s, "{name} {i} xx");
            }
            s.into_bytes()
        }
    };
    let b = if name == "CRLF.txt" {
        b"a\r\nb\r\n".to_vec()
    } else if name == "nonl.txt" {
        b"x".to_vec()
    } else {
        b
    };
    (b, exec)
}

fn regex(r: &mut Rng) -> String {
    r.pick(REGEXES).copied().unwrap_or(".").to_owned()
}

/// A YAML single-quoted scalar.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn tags(r: &mut Rng) -> String {
    let n = r.below(2).saturating_add(1);
    let mut v: Vec<&str> = Vec::new();
    for _ in 0..n {
        if let Some(t) = r.pick(TAGS)
            && !v.contains(t)
        {
            v.push(t);
        }
    }
    format!("[{}]", v.join(", "))
}

fn args(r: &mut Rng) -> String {
    const ARGS: &[&str] = &["--flag", "with space", "it's", "{}", "\u{0436}\u{0443}\u{043a}", "\"q\"", "-x=1", ""];
    let n = r.below(3).saturating_add(1);
    let v: Vec<String> = (0..n).map(|_| sq(r.pick(ARGS).copied().unwrap_or("x"))).collect();
    format!("[{}]", v.join(", "))
}

/// One local hook as a flow mapping (one line).
fn local_hook(r: &mut Rng, i: usize, probe: bool) -> String {
    let id = format!("h{i}");
    let mut kv: Vec<String> = vec![format!("id: {id}"), format!("name: {}", sq(&format!("hook {i}")))];
    match r.below(10) {
        0..=5 => {
            let entry = if probe {
                match r.below(6) {
                    0 => "${probe} args",
                    1 => "${probe} count",
                    2 => "${probe} exit 1",
                    3 => "${probe} fail-on a.py",
                    4 => "${probe} append ' # touched' --",
                    _ => "${probe} interleave 2",
                }
            } else {
                match r.below(3) {
                    0 => "true",
                    1 => "false",
                    _ => "echo",
                }
            };
            kv.push(format!("entry: {}", sq(entry)));
            kv.push(String::from("language: system"));
        }
        6..=8 => {
            kv.push(format!("entry: {}", sq(r.pick(PYGREP).copied().unwrap_or("TODO"))));
            kv.push(String::from("language: pygrep"));
            if r.chance(1, 4) {
                kv.push(format!("args: [{}]", r.pick(&["--ignore-case", "--negate", "--multiline"]).copied().unwrap_or("--negate")));
            }
        }
        _ => {
            kv.push(format!("entry: {}", sq("must not be committed")));
            kv.push(String::from("language: fail"));
        }
    }
    if r.chance(1, 3) {
        kv.push(format!("files: {}", sq(&regex(r))));
    }
    if r.chance(1, 5) {
        kv.push(format!("exclude: {}", sq(&regex(r))));
    }
    match r.below(6) {
        0 => kv.push(format!("types: {}", tags(r))),
        1 => kv.push(format!("types_or: {}", tags(r))),
        2 => kv.push(format!("exclude_types: {}", tags(r))),
        _ => {}
    }
    if r.chance(1, 8) {
        kv.push(format!("stages: [{}]", r.pick(&["pre-commit", "manual", "pre-push", "commit-msg"]).copied().unwrap_or("manual")));
    }
    for (key, num) in [("always_run", 1), ("pass_filenames", 1), ("require_serial", 1), ("verbose", 1), ("fail_fast", 1)] {
        if r.chance(num, 8) {
            let v = if key == "pass_filenames" { "false" } else { "true" };
            kv.push(format!("{key}: {v}"));
        }
    }
    if r.chance(1, 8) {
        kv.push(format!("alias: a{i}"));
    }
    if r.chance(1, 5) {
        kv.push(format!("args: {}", args(r)));
    }
    if r.chance(1, 12) {
        kv.push(format!("log_file: {}", sq(&format!("log{i}.txt"))));
    }
    format!("    - {{{}}}\n", kv.join(", "))
}

/// The case `index` of the suite seeded with `seed`. `probe`: entries may use `${probe}` (the scenario then
/// requires snp-probe).
pub fn generate(seed: u64, index: u64, probe: bool) -> Scenario {
    let mut r = Rng::new(seed ^ index.wrapping_mul(0xa076_1d64_78bd_642f));
    let mut s = Scenario::new(&format!("generated/{seed}-{index}"));
    s.tags = vec![String::from("generated")];
    s.requires.os = vec![String::from("linux"), String::from("macos")];
    // File tree.
    let n_files = if r.chance(1, 20) { r.below(120) } else { r.below(12) };
    let mut names: Vec<String> = Vec::new();
    for k in 0..n_files {
        let base = r.pick(NAMES).copied().unwrap_or("f.txt");
        let name = if names.iter().any(|n| n == base) { format!("gen{k}/{base}") } else { base.to_owned() };
        if names.contains(&name) {
            continue;
        }
        let (bytes, exec) = content(&mut r, &name);
        if exec {
            s.work.exec.push(name.clone());
        }
        s.work.files.push((name.clone().into_bytes(), bytes));
        names.push(name);
    }
    if r.chance(1, 8) {
        let target = names.first().cloned().unwrap_or_else(|| String::from("missing-target"));
        s.work.symlinks.push((String::from("link"), target));
        if r.chance(1, 2) {
            s.work.symlinks.push((String::from("dangling"), String::from("does/not/exist")));
        }
    }
    // Config.
    let mut cfg = String::new();
    if r.chance(1, 6) {
        let _ = writeln!(cfg, "files: {}", sq(&regex(&mut r)));
    }
    if r.chance(1, 6) {
        let _ = writeln!(cfg, "exclude: {}", sq(&regex(&mut r)));
    }
    if r.chance(1, 8) {
        cfg.push_str("fail_fast: true\n");
    }
    if r.chance(1, 10) {
        cfg.push_str("default_stages: [pre-commit, manual]\n");
    }
    cfg.push_str("repos:\n-   repo: local\n    hooks:\n");
    let n_hooks = r.below(15).saturating_add(1);
    for i in 0..n_hooks {
        cfg.push_str(&local_hook(&mut r, i, probe));
    }
    if r.chance(1, 6) {
        cfg.push_str("-   repo: meta\n    hooks:\n");
        let id = r.pick(&["check-hooks-apply", "check-useless-excludes", "identity"]).copied().unwrap_or("identity");
        let _ = writeln!(cfg, "    - {{id: {id}}}");
    }
    if r.chance(1, 8) {
        s.repos.push(RepoSpec {
            name: String::from("hooks"),
            fixture: Some(String::from("script_hooks_repo")),
            ..RepoSpec::default()
        });
        cfg.push_str("-   repo: ${repo:hooks}\n    rev: ${rev:hooks}\n    hooks:\n    - {id: bash_hook}\n");
    }
    s.work.config = Some(cfg);
    // Git state.
    let mut committed: Vec<Vec<u8>> = vec![b".pre-commit-config.yaml".to_vec()];
    let mut staged: Vec<Vec<u8>> = Vec::new();
    for n in &names {
        match r.below(4) {
            0 => committed.push(n.clone().into_bytes()),
            1 | 2 => staged.push(n.clone().into_bytes()),
            _ => {}
        }
    }
    for (l, _) in &s.work.symlinks {
        if r.chance(1, 2) {
            staged.push(l.clone().into_bytes());
        }
    }
    s.work.commit = committed.clone();
    s.work.stage = staged.clone();
    for n in &names {
        let nb = n.as_bytes();
        if (committed.iter().any(|c| c == nb) || staged.iter().any(|c| c == nb)) && r.chance(1, 5) {
            s.work.unstaged.push((n.clone(), if r.chance(1, 4) { None } else { Some(b"changed later\n".to_vec()) }));
        }
    }
    if r.chance(1, 10)
        && let Some(n) = names.iter().find(|n| !committed.iter().chain(&staged).any(|c| c == n.as_bytes()))
    {
        s.work.setup.push(Op::Git(vec![String::from("add"), String::from("-N"), String::from("--"), n.clone()]));
    }
    if r.chance(1, 10)
        && let Some(n) = committed.iter().skip(1).find(|c| !s.work.unstaged.iter().any(|(u, _)| u.as_bytes() == c.as_slice()))
    {
        let n = String::from_utf8_lossy(n).into_owned();
        s.work.setup.push(Op::Git(vec![String::from("rm"), String::from("-q"), String::from("--cached"), String::from("--"), n]));
    }
    if r.chance(1, 15) {
        s.work.setup.push(Op::Git(vec![String::from("checkout"), String::from("-q"), String::from("--detach")]));
    }
    // Invocations.
    let step = |kind: StepKind, env: Vec<(String, Option<String>)>| Step {
        kind,
        env,
        stdin: Vec::new(),
        tty: false,
        timeout_ms: Some(60_000),
        cwd: None,
        signal: None,
    };
    match r.below(10) {
        0 => {
            s.steps.push(step(StepKind::Tool(vec![String::from("install")]), Vec::new()));
            s.steps.push(step(
                StepKind::Git(vec![String::from("commit"), String::from("-q"), String::from("-m"), String::from("msg")]),
                Vec::new(),
            ));
        }
        1 => s.steps.push(step(StepKind::Tool(vec![String::from("validate-config"), String::from(".pre-commit-config.yaml")]), Vec::new())),
        _ => {
            let mut a = vec![String::from("run")];
            match r.below(5) {
                0 | 1 => a.push(String::from("--all-files")),
                2 if !names.is_empty() => {
                    a.push(String::from("--files"));
                    for _ in 0..r.below(3).saturating_add(1) {
                        if let Some(n) = r.pick(&names) {
                            a.push(n.clone());
                        }
                    }
                }
                3 => {
                    a.push(String::from("--from-ref"));
                    a.push(String::from("HEAD"));
                    a.push(String::from("--to-ref"));
                    a.push(String::from("HEAD"));
                }
                _ => {}
            }
            if r.chance(1, 6) {
                a.push(String::from("--hook-stage"));
                a.push(String::from("manual"));
            }
            if r.chance(1, 6) {
                a.push(format!("h{}", r.below(n_hooks)));
            }
            if r.chance(1, 4) {
                a.push(String::from("--show-diff-on-failure"));
            }
            if r.chance(1, 4) {
                a.push(String::from("--verbose"));
            }
            if r.chance(1, 4) {
                a.push(String::from("--color"));
                a.push(r.pick(&["always", "never"]).copied().unwrap_or("never").to_owned());
            }
            let mut env = Vec::new();
            if r.chance(1, 5) {
                env.push((String::from("SKIP"), Some(format!("h{},nonexistent", r.below(n_hooks)))));
            }
            s.steps.push(step(StepKind::Tool(a), env));
        }
    }
    if probe {
        s.requires.probe = true;
    }
    s
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn deterministic_and_parseable() {
        for i in 0..300 {
            let a = generate(7, i, i % 2 == 0);
            let b = generate(7, i, i % 2 == 0);
            assert_eq!(a.to_yaml(), b.to_yaml());
            let back = Scenario::parse(&a.to_yaml(), Path::new("g.yaml")).unwrap();
            assert_eq!(back.work, a.work, "case {i}");
            assert_eq!(back.steps, a.steps, "case {i}");
            assert!(!a.steps.is_empty());
            assert!(crate::shrink::hook_count(&a) >= 1);
        }
        assert_ne!(generate(1, 0, true).to_yaml(), generate(2, 0, true).to_yaml());
    }

    #[test]
    fn rng_basics() {
        let mut r = Rng::new(0);
        assert_eq!(r.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(r.below(0), 0);
        assert!((0..100).all(|_| r.below(3) < 3));
        assert!(!r.chance(1, 0));
    }
}
