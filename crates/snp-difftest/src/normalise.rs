//! Normalisation tiers and the brand map (ARCHITECTURE §13.2, §9.6; COMPATIBILITY §2 "Comparison tiers", §3.3;
//! research 10 §7.4.5).
//!
//! * **Tier A** — byte-exact: only the brand map is applied (snp-brand runs).
//! * **Tier C** — the "ignored" items are masked on both sides: `- duration: …s`, stash patch names and pids, the
//!   store roots (snp's store differs from pre-commit's by design, FD-2), random `repo…`/`tmp…` suffixes and the
//!   installer chatter between `[INFO] Installing environment for …` and the next status line, and the `rev:` of
//!   try-repo's shadow repository (committed by pre-commit without the fixed commit dates). This is what the
//!   `exact` compare mode uses: everything else stays byte-exact.
//! * **Tier B** — Tier C plus the semantic reductions: Python tracebacks to their last line, argparse usage blocks
//!   to their `error:` lines (the py312 oracle wraps differently, 12 §A.4.1), YAML error bodies below the `=====> `
//!   line, localized `[WinError N]` texts and version numbers.
//!
//! * **Generated reductions** — [`reduce_generated`], on top of a tier, only for the nightly generated suite: the
//!   differences it is known to hit, which would otherwise hide every new one (tests/differential/README.md).
//!
//! The brand map ([`BrandMap`]) is the machine form of COMPATIBILITY §3.3 (`tests/differential/brand_map.yaml`). It
//! is applied to the prog-patched oracle's output in snp-brand runs, never inside argparse blocks (a `usage: `
//! line through the next `…: error: …` line, or to the end of a help text), whose bytes the patched oracle already
//! renders with prog `snp` (ADR-058).
//!
//! Owner: WP-35.

use std::path::Path;

use crate::scenario::Brand;
use crate::text::{contains_bytes, lines_with_ends, replace_bytes};
use crate::yamlv::Yv;

/// How strictly two outputs are compared.
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum Tier {
    /// Exact.
    A,
    /// Semantic (crash-log body, tracebacks → last line, …).
    B,
    /// Ignored (durations, patch names, store roots, …).
    C,
}

/// One substring rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The compat (pre-commit 4.7.0) text.
    pub from: String,
    /// The snp-brand text.
    pub to: String,
}

/// `tests/differential/brand_map.yaml`; non-argparse texts only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrandMap {
    /// Whole-line rules (`--version`): a line exactly equal to `from` (without its newline) becomes `to`. Applied
    /// before the substring rules.
    whole_line: Vec<Rule>,
    /// Substring rules in file order.
    substitutions: Vec<Rule>,
    /// Texts a rule must never touch (validated at load).
    never: Vec<String>,
    /// Substring rules applied **inside** argparse blocks only: the top-level parser's command list, which snp's
    /// extra commands lengthen (the rendering stays on one line at any width ≥ 80, so a substitution is exact).
    argparse_subs: Vec<Rule>,
    /// Sub-commands whose parsers have snp-only options: their usage blocks cannot come from the oracle at all; both
    /// sides' blocks are reduced to the first usage line's command and the `error:` line (the full renderings are
    /// pinned by WP-12's `crates/snp-cli/tests/goldens/snp.json`).
    snp_only_parsers: Vec<String>,
}

impl BrandMap {
    /// The identity map (compat-brand runs).
    pub fn identity() -> Self {
        Self::default()
    }

    /// Whether the map changes nothing.
    pub fn is_identity(&self) -> bool {
        self.whole_line.is_empty() && self.substitutions.is_empty() && self.argparse_subs.is_empty()
    }

    /// Parses the YAML text of `brand_map.yaml` and validates it: `version: 1`, non-empty rules, and no rule whose
    /// `from` text occurs inside (or contains) a `never` entry.
    pub fn parse(text: &str) -> Result<Self, String> {
        let doc = Yv::parse(text).map_err(|e| format!("brand_map: {e}"))?;
        match doc.get("version").and_then(Yv::as_i64) {
            Some(1) => {}
            _ => return Err(String::from("brand_map: `version: 1` expected")),
        }
        let rules = |key: &str| -> Result<Vec<Rule>, String> {
            let Some(list) = doc.get(key) else {
                return Ok(Vec::new());
            };
            let items = list
                .as_seq()
                .ok_or_else(|| format!("brand_map.{key}: expected a list"))?;
            let mut out = Vec::new();
            for it in items {
                let from = it
                    .get("from")
                    .and_then(Yv::as_str)
                    .ok_or_else(|| format!("brand_map.{key}: rule without `from`"))?;
                let to = it
                    .get("to")
                    .and_then(Yv::as_str)
                    .ok_or_else(|| format!("brand_map.{key}: rule without `to`"))?;
                if from.is_empty() || from.contains('\n') {
                    return Err(format!("brand_map.{key}: bad `from` {from:?}"));
                }
                out.push(Rule {
                    from: from.to_owned(),
                    to: to.to_owned(),
                });
            }
            Ok(out)
        };
        let whole_line = rules("whole_line")?;
        let substitutions = rules("substitutions")?;
        let argparse_subs = rules("argparse_substitutions")?;
        let mut snp_only_parsers = Vec::new();
        if let Some(list) = doc.get("snp_only_option_parsers") {
            for it in list
                .as_seq()
                .ok_or("brand_map.snp_only_option_parsers: expected a list")?
            {
                snp_only_parsers.push(
                    it.as_str()
                        .ok_or("brand_map.snp_only_option_parsers: expected strings")?
                        .to_owned(),
                );
            }
        }
        let mut never = Vec::new();
        if let Some(list) = doc.get("never") {
            for it in list.as_seq().ok_or("brand_map.never: expected a list")? {
                never.push(
                    it.as_str()
                        .ok_or("brand_map.never: expected strings")?
                        .to_owned(),
                );
            }
        }
        for r in &substitutions {
            if let Some(n) = never
                .iter()
                .find(|n| n.contains(r.from.as_str()) || r.from.contains(n.as_str()))
            {
                return Err(format!(
                    "brand_map: rule {:?} would touch the protected text {n:?}",
                    r.from
                ));
            }
        }
        Ok(Self {
            whole_line,
            substitutions,
            never,
            argparse_subs,
            snp_only_parsers,
        })
    }

    /// Loads `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The committed `tests/differential/brand_map.yaml` of this checkout.
    pub fn workspace() -> Result<Self, String> {
        Self::load(&crate::workspace_root().join("tests/differential/brand_map.yaml"))
    }

    /// The protected texts.
    pub fn never(&self) -> &[String] {
        &self.never
    }

    /// The substring rules.
    pub fn substitutions(&self) -> &[Rule] {
        &self.substitutions
    }

    /// The whole-line rules.
    pub fn whole_line(&self) -> &[Rule] {
        &self.whole_line
    }

    /// The argparse-block rules.
    pub fn argparse_substitutions(&self) -> &[Rule] {
        &self.argparse_subs
    }

    /// The sub-commands whose snp-brand parsers have snp-only options.
    pub fn snp_only_option_parsers(&self) -> &[String] {
        &self.snp_only_parsers
    }

    /// Reduces the usage blocks of [`BrandMap::snp_only_option_parsers`] (`usage: snp run …` through the
    /// `snp run: error: …` line) to `usage: snp run [SNP-ONLY OPTIONS]` plus the error line. Applied to both sides
    /// of snp-brand runs.
    pub fn reduce_snp_only_parsers(&self, bytes: &[u8]) -> Vec<u8> {
        if self.snp_only_parsers.is_empty() {
            return bytes.to_vec();
        }
        let mut out = Vec::new();
        let mut skipping = false;
        for line in lines_with_ends(bytes) {
            if skipping {
                if is_argparse_error_line(line) {
                    skipping = false;
                    out.extend_from_slice(line);
                }
                continue;
            }
            if let Some(cmd) = self.snp_only_parsers.iter().find(|c| {
                line.starts_with(format!("usage: snp {c} ").as_bytes())
                    || line == format!("usage: snp {c}\n").as_bytes()
            }) {
                out.extend_from_slice(format!("usage: snp {cmd} [SNP-ONLY OPTIONS]\n").as_bytes());
                skipping = true;
                continue;
            }
            out.extend_from_slice(line);
        }
        out
    }

    /// Compat text → snp-brand text (outside argparse blocks).
    pub fn apply(&self, bytes: &[u8]) -> Vec<u8> {
        self.map_text(bytes, false)
    }

    /// snp-brand text → compat text: the inverse rules in reverse order. `invert(apply(t)) == t` for every compat
    /// text `t` that contains no snp-brand phrase already (round-trip tested).
    pub fn invert(&self, bytes: &[u8]) -> Vec<u8> {
        self.map_text(bytes, true)
    }

    fn map_text(&self, bytes: &[u8], inverse: bool) -> Vec<u8> {
        if self.is_identity() {
            return bytes.to_vec();
        }
        let mut out = Vec::new();
        let mut in_argparse = false;
        for line in lines_with_ends(bytes) {
            if line.starts_with(b"usage: ") {
                in_argparse = true;
            }
            if in_argparse {
                let mut cur = line.to_vec();
                for r in &self.argparse_subs {
                    let (from, to) = if inverse {
                        (&r.to, &r.from)
                    } else {
                        (&r.from, &r.to)
                    };
                    cur = replace_bytes(&cur, from.as_bytes(), to.as_bytes());
                }
                out.extend_from_slice(&cur);
                if is_argparse_error_line(line) {
                    in_argparse = false;
                }
                continue;
            }
            out.extend_from_slice(&self.map_line(line, inverse));
        }
        out
    }

    fn map_line(&self, line: &[u8], inverse: bool) -> Vec<u8> {
        let (body, end) = split_eol(line);
        for r in &self.whole_line {
            let (from, to) = if inverse {
                (&r.to, &r.from)
            } else {
                (&r.from, &r.to)
            };
            if body == from.as_bytes() {
                let mut v = to.as_bytes().to_vec();
                v.extend_from_slice(end);
                return v;
            }
        }
        let mut cur = body.to_vec();
        if inverse {
            for r in self.substitutions.iter().rev() {
                cur = replace_bytes(&cur, r.to.as_bytes(), r.from.as_bytes());
            }
        } else {
            for r in &self.substitutions {
                cur = replace_bytes(&cur, r.from.as_bytes(), r.to.as_bytes());
            }
        }
        cur.extend_from_slice(end);
        cur
    }
}

/// `prog: error: …` / `prog sub: error: …` (argparse's `error()` line).
fn is_argparse_error_line(line: &[u8]) -> bool {
    let first = line.split(|&c| c == b' ').next().unwrap_or_default();
    !first.is_empty() && !first.starts_with(b"usage") && contains_bytes(line, b": error: ")
}

fn split_eol(line: &[u8]) -> (&[u8], &[u8]) {
    let n = if line.ends_with(b"\r\n") {
        2
    } else {
        usize::from(line.ends_with(b"\n"))
    };
    line.split_at_checked(line.len().saturating_sub(n))
        .unwrap_or((line, &[]))
}

/// Applies the tier's rules (and the brand map for snp-brand runs) to `bytes`.
///
/// The brand map is applied first (to non-argparse lines), then the tier: [`Tier::A`] adds nothing, [`Tier::C`]
/// masks the ignored items, [`Tier::B`] masks them and applies the semantic reductions. Paths are masked by the
/// generic rules only; the runner additionally replaces its fixed root's store paths (see [`mask_paths`]).
pub fn normalise(tier: Tier, bytes: &[u8], map: &BrandMap) -> Vec<u8> {
    let mapped = map.apply(bytes);
    match tier {
        Tier::A => mapped,
        Tier::C => tier_c(&canonical_help(&canonical_usage(&mapped))),
        Tier::B => tier_c(&tier_b(&canonical_help(&canonical_usage(&mapped)))),
    }
}

/// The remaining CPython 3.12 → 3.13 rows of research 12 §A.4.1 (the help option lines and try-repo's usage wrap):
/// 3.12 repeats the metavar after every option string (`-c CONFIG, --config CONFIG`), 3.13 prints it once
/// (`-c, --config CONFIG`), which also moves the help text. Literal rewrites of exactly the listed lines, so the
/// py312 runtime oracle's help texts compare with snp's canonical 3.13 rendering (COMPATIBILITY §1: the 3.13 texts
/// are canonical). Text already in the 3.13 form passes through unchanged.
pub(crate) fn canonical_help(bytes: &[u8]) -> Vec<u8> {
    const PAIRS: [(&[u8], &[u8]); 6] = [
        (
            b"  -c CONFIG, --config CONFIG\n                        Path to alternate config file\n",
            b"  -c, --config CONFIG   Path to alternate config file\n",
        ),
        (b"\n  -j JOBS, --jobs JOBS  ", b"\n  -j, --jobs JOBS       "),
        (
            b"\n  --from-ref FROM_REF, --source FROM_REF, -s FROM_REF\n",
            b"\n  --from-ref, --source, -s FROM_REF\n",
        ),
        (
            b"\n  --to-ref TO_REF, --origin TO_REF, -o TO_REF\n",
            b"\n  --to-ref, --origin, -o TO_REF\n",
        ),
        (b"\n  --ref REF, --rev REF  ", b"\n  --ref, --rev REF      "),
        (
            b"\n                           [--ref REF] [--verbose]\n                           [--all-files | --files [FILES ...]]\n                           [--show-diff-on-failure] [--fail-fast]\n",
            b"\n                           [--ref REF] [--verbose] [--all-files |\n                           --files [FILES ...]] [--show-diff-on-failure]\n                           [--fail-fast]\n",
        ),
    ];
    let mut cur = bytes.to_vec();
    for (from, to) in PAIRS {
        cur = replace_bytes(&cur, from, to);
        // Windows: Python's text-mode stdout writes the same lines with CRLF
        let (from, to) = (crlf(from), crlf(to));
        cur = replace_bytes(&cur, &from, &to);
    }
    // `-t {…}, --hook-type {…}` → `-t, --hook-type {…}` (the choices are the same list twice)
    let lines = lines_with_ends(&cur);
    let mut out: Vec<u8> = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix(b"  -t {")
            && let Some(at) = crate::text::find_bytes(rest, b"}, --hook-type ")
            && let Some(tail) = rest.get(at.saturating_add(3)..)
        {
            out.extend_from_slice(b"  -t, ");
            out.extend_from_slice(tail);
            continue;
        }
        out.extend_from_slice(line);
    }
    out
}

/// `b` with every `\n` written as `\r\n`.
fn crlf(b: &[u8]) -> Vec<u8> {
    replace_bytes(b, b"\n", b"\r\n")
}

/// CPython 3.12 → 3.13 argparse rendering (research 12 §A.4.1): 3.12 puts the `...` of a sub-command usage on its
/// own line after the choices (`{autoupdate,…,hook-impl}` / `...`); 3.13 renders `{…} ...` on one line. Inside an
/// argparse block, a line of indentation plus `...` that follows a line ending in `}` is joined onto it. Applied to
/// both sides in every tier but A (the py312 runtime oracle vs snp's canonical 3.13 rendering).
pub fn canonical_usage(bytes: &[u8]) -> Vec<u8> {
    let lines = lines_with_ends(bytes);
    let mut out: Vec<u8> = Vec::new();
    let mut in_usage = false;
    let mut prev_closes = false;
    for line in lines {
        if line.starts_with(b"usage: ") {
            in_usage = true;
        }
        // `\n` or (Windows text mode) `\r\n`
        let (trimmed, eol) = split_eol(line);
        let is_dots = in_usage
            && !trimmed.is_empty()
            && trimmed.trim_ascii_start() == b"..."
            && trimmed.starts_with(b" ");
        if is_dots && prev_closes && out.ends_with(b"\n") {
            out.pop();
            if out.ends_with(b"\r") {
                out.pop();
            }
            out.extend_from_slice(b" ...");
            out.extend_from_slice(eol);
            prev_closes = false;
            continue;
        }
        prev_closes = in_usage && trimmed.ends_with(b"}");
        if in_usage && (is_argparse_error_line(line) || trimmed.is_empty()) {
            in_usage = false;
        }
        out.extend_from_slice(line);
    }
    out
}

/// Replaces the scenario root's store directories (`<root>/store`, `<root>/snp-store`) with `[STORE]` and the
/// stash patch paths below them with `[STORE]/patch[TP]` (FD-2: the stores differ by design; Tier C).
pub fn mask_paths(bytes: &[u8], stores: &[&[u8]]) -> Vec<u8> {
    let mut cur = bytes.to_vec();
    let mut sorted: Vec<&[u8]> = stores.to_vec();
    sorted.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for s in sorted {
        if !s.is_empty() {
            cur = replace_bytes(&cur, s, b"[STORE]");
        }
    }
    // snp keeps its patches in `patches/` (ARCHITECTURE §5.2): `[STORE]/patches/patch…` ≡ `[STORE]/patch…` (and
    // `[STORE]\patches\patch…` ≡ `[STORE]\patch…` on Windows).
    let cur = replace_bytes(&cur, b"[STORE]/patches/patch", b"[STORE]/patch");
    replace_bytes(&cur, b"[STORE]\\patches\\patch", b"[STORE]\\patch")
}

// ---------------------------------------------------------------------------------------------------------------
// Tier C

fn tier_c(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut in_install = false;
    let mut after_shadow = false;
    for line in lines_with_ends(b) {
        if in_install {
            if is_status_line(line) {
                in_install = false;
            } else if !is_install_note(line) {
                continue;
            }
        }
        // try-repo's shadow repository (uncommitted changes) is committed under `no_git_env`, i.e. without the
        // harness's fixed `GIT_*_DATE`: its `rev:` is a wall-clock-dependent SHA on both sides. Only kept lines take
        // part, so masking is idempotent.
        let shadow_rev = std::mem::replace(&mut after_shadow, is_shadow_repo_line(line));
        if shadow_rev && let Some(indent) = shadow_rev_indent(line) {
            out.extend_from_slice(indent);
            out.extend_from_slice(b"rev: [SHADOW-REV]");
            out.extend_from_slice(split_eol(line).1);
            continue;
        }
        if line.starts_with(b"[INFO] Installing environment for ") {
            in_install = true;
        }
        let l = mask_duration(line);
        let l = mask_numbered(&l, b"patch", b"patch[TP]", true);
        let l = mask_rand(&l, b"repo", b"repo[RAND]");
        let l = mask_rand(&l, b"tmp", b"tmp[RAND]");
        out.extend_from_slice(&l);
    }
    out
}

/// `-   repo: <…>/shadow-repo` in try-repo's `Using config:` block.
fn is_shadow_repo_line(line: &[u8]) -> bool {
    let (body, _) = split_eol(line);
    // `\shadow-repo`: the tempdir joined by `os.path` on Windows
    (body.ends_with(b"/shadow-repo") || body.ends_with(b"\\shadow-repo"))
        && contains_bytes(body, b"repo: ")
}

/// The indentation of a `    rev: <40 or 64 hex digits>` line.
fn shadow_rev_indent(line: &[u8]) -> Option<&[u8]> {
    let (body, _) = split_eol(line);
    let indent = body.iter().take_while(|c| **c == b' ').count();
    let (head, rest) = body.split_at_checked(indent)?;
    let sha = rest.strip_prefix(b"rev: ")?;
    (matches!(sha.len(), 40 | 64) && sha.iter().all(u8::is_ascii_hexdigit)).then_some(head)
}

/// The two notes pre-commit prints right after `Installing environment for …`: part of the header, kept.
fn is_install_note(line: &[u8]) -> bool {
    line.starts_with(b"[INFO] Once installed this environment will be reused.")
        || line.starts_with(b"[INFO] This may take a few minutes...")
}

/// A line that ends the installer chatter: a log line, a hook status line or an error banner.
fn is_status_line(line: &[u8]) -> bool {
    if is_install_note(line) {
        return false;
    }
    line.starts_with(b"[INFO]")
        || line.starts_with(b"[WARNING]")
        || line.starts_with(b"[ERROR]")
        || line.starts_with(b"An error has occurred")
        || line.starts_with(b"An unexpected error has occurred")
        || contains_bytes(line, b"...Passed")
        || contains_bytes(line, b"...Failed")
        || contains_bytes(line, b"...Skipped")
        || contains_bytes(line, b")Skipped")
}

/// `- duration: 0.03s` → `- duration: [D]s` (and the coloured variant with the subtle escape).
fn mask_duration(line: &[u8]) -> Vec<u8> {
    let key = b"- duration: ";
    let Some(start) = crate::text::find_bytes(line, key) else {
        return line.to_vec();
    };
    let after = start.saturating_add(key.len());
    let rest = line.get(after..).unwrap_or_default();
    let num = rest
        .iter()
        .take_while(|c| c.is_ascii_digit() || **c == b'.')
        .count();
    if num == 0 || rest.get(num) != Some(&b's') {
        return line.to_vec();
    }
    let mut out = line.get(..after).unwrap_or_default().to_vec();
    out.extend_from_slice(b"[D]");
    out.extend_from_slice(rest.get(num..).unwrap_or_default());
    out
}

/// `patch<digits>-<digits>` → `patch[TP]` (`numbered_pid`), also inside paths.
fn mask_numbered(line: &[u8], prefix: &[u8], repl: &[u8], with_pid: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(rest) = line.get(i..) {
        if rest.is_empty() {
            break;
        }
        if rest.starts_with(prefix) {
            let tail = rest.get(prefix.len()..).unwrap_or_default();
            let d1 = tail.iter().take_while(|c| c.is_ascii_digit()).count();
            let mut used = d1;
            let mut ok = d1 > 0;
            if ok && with_pid {
                if tail.get(d1) == Some(&b'-') {
                    let d2 = tail
                        .get(d1.saturating_add(1)..)
                        .unwrap_or_default()
                        .iter()
                        .take_while(|c| c.is_ascii_digit())
                        .count();
                    ok = d2 > 0;
                    used = d1.saturating_add(1).saturating_add(d2);
                } else {
                    ok = false;
                }
            }
            if ok {
                out.extend_from_slice(repl);
                i = i.saturating_add(prefix.len()).saturating_add(used);
                continue;
            }
        }
        out.extend_from_slice(rest.get(..1).unwrap_or_default());
        i = i.saturating_add(1);
    }
    out
}

/// `repo` + exactly 8 of `[a-z0-9_]` (pre-commit's `tempfile.mkdtemp(prefix='repo')` names) → `repo[RAND]`; the
/// same for `tmp`. The character before the prefix must not be alphanumeric (`/repoabc12345`, ` tmpxyz_1234`).
/// `tmp` also takes the names of Rust's `tempfile` crate (snp's own temporary directories): 6 or 8 of
/// `[A-Za-z0-9]` followed by a non-alphanumeric byte (random directory suffixes are Tier C, COMPATIBILITY §1).
fn mask_rand(line: &[u8], prefix: &[u8], repl: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0_usize;
    let is_rand = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_';
    while let Some(rest) = line.get(i..) {
        if rest.is_empty() {
            break;
        }
        let boundary = i
            .checked_sub(1)
            .and_then(|p| line.get(p))
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if boundary && rest.starts_with(prefix) {
            let tail = rest.get(prefix.len()..).unwrap_or_default();
            let n = tail.iter().take_while(|c| is_rand(c)).count();
            if n == 8 {
                out.extend_from_slice(repl);
                i = i.saturating_add(prefix.len()).saturating_add(8);
                continue;
            }
            let alnum = tail
                .iter()
                .take_while(|c| c.is_ascii_alphanumeric())
                .count();
            let ends = tail.get(alnum).is_none_or(|c| !c.is_ascii_alphanumeric());
            if prefix == b"tmp" && (alnum == 6 || alnum == 8) && ends {
                out.extend_from_slice(repl);
                i = i.saturating_add(prefix.len()).saturating_add(alnum);
                continue;
            }
        }
        out.extend_from_slice(rest.get(..1).unwrap_or_default());
        i = i.saturating_add(1);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Generated suite

/// The argparse error line whose argument list the two argparse profiles disagree on, in one brand of a generated
/// scenario. Python 3.12.3 (the compat oracle) lets a `nargs='*'` positional match nothing before an unknown option
/// and reports everything after it, 3.13.14+ (the canonical profile, the snp oracle, snp-argparse) goes on filling
/// it. The snp brand keeps that hook-time line byte-exact against 3.13. The other way round, snp-brand `run` takes
/// several hook ids (D-EXT-07), so positionals the oracle rejects are accepted; the compat brand keeps that one.
const fn unrecognized_prefix(brand: Brand) -> &'static [u8] {
    match brand {
        Brand::Compat => b"pygrep.py: error: unrecognized arguments: ",
        Brand::Snp => b"snp: error: unrecognized arguments: ",
    }
}

/// Reduces what the generated suite is known to hit (raw bytes, both sides, streams and files alike):
///
/// * the traceback of pygrep failing to `open()` a file collapses to its exception line, uncoloured (D-MISC-09;
///   CPython 3.13 colours tracebacks in a `--color always` run, where the hook has a pty);
/// * the argument list of [`unrecognized_prefix`] is masked.
///
/// Anything else stays exact: the suite's job is to find the difference nobody has seen yet, and a blanket Tier B
/// would hide the next one. Curated scenarios never come here, so `divergences/D-MISC-09` still sees the traceback.
pub(crate) fn reduce_generated(bytes: &[u8], brand: Brand) -> Vec<u8> {
    let lines = lines_with_ends(bytes);
    let prefix = unrecognized_prefix(brand);
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(line) = lines.get(i).copied() {
        if let Some((end, exception)) = pygrep_open_traceback(&lines, i) {
            out.extend_from_slice(&exception);
            i = end;
            continue;
        }
        i = i.saturating_add(1);
        if line.starts_with(prefix) {
            out.extend_from_slice(prefix);
            out.extend_from_slice(b"[ARGS]");
            out.extend_from_slice(split_eol(line).1);
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

/// When a traceback starts at `lines[start]` and is pygrep's failed `open()`: the index after it and its uncoloured
/// exception line. Every other traceback is left alone (`None`).
fn pygrep_open_traceback(lines: &[&[u8]], start: usize) -> Option<(usize, Vec<u8>)> {
    const HEADER: &[u8] = b"Traceback (most recent call last):";
    let header = lines.get(start)?;
    if !contains_bytes(header, HEADER) || split_eol(&strip_ansi(header)).0 != HEADER {
        return None;
    }
    let mut i = start.saturating_add(1);
    let mut in_pygrep = false;
    let mut opens = false;
    while let Some(line) = lines.get(i) {
        if !line.starts_with(b" ") {
            break;
        }
        let frame = strip_ansi(line);
        in_pygrep |= contains_bytes(&frame, b"/pre_commit/languages/pygrep.py\"");
        opens = contains_bytes(&frame, b"with open(filename, 'rb') as f:")
            || (opens && is_caret_line(&frame));
        i = i.saturating_add(1);
    }
    (in_pygrep && opens)
        .then(|| lines.get(i).map(|l| (i.saturating_add(1), strip_ansi(l))))
        .flatten()
}

/// The `^^^^` / `~~~~` marker line CPython prints under a traceback's source line.
fn is_caret_line(line: &[u8]) -> bool {
    let body = split_eol(line).0.trim_ascii();
    !body.is_empty() && body.iter().all(|b| matches!(b, b'^' | b'~'))
}

/// `bytes` without its SGR colour sequences (`ESC [ … m`).
fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = bytes;
    while let Some((&first, tail)) = rest.split_first() {
        if first == 0x1b
            && tail.first() == Some(&b'[')
            && let Some(len) = tail
                .iter()
                .skip(1)
                .position(|b| !(b.is_ascii_digit() || *b == b';'))
            && tail.get(len.saturating_add(1)) == Some(&b'm')
        {
            rest = tail.get(len.saturating_add(2)..).unwrap_or_default();
        } else {
            out.push(first);
            rest = tail;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Tier B

fn tier_b(b: &[u8]) -> Vec<u8> {
    let lines = lines_with_ends(b);
    let mut out = Vec::new();
    let mut i = 0_usize;
    let mut in_usage = false;
    // Where the exception line of the traceback just collapsed starts in `out` (a chained traceback replaces it).
    let mut last_exception: Option<usize> = None;
    while let Some(line) = lines.get(i).copied() {
        i = i.saturating_add(1);
        // argparse usage block: keep only the `error:` line (wrap points differ between the oracles).
        if line.starts_with(b"usage: ") {
            in_usage = true;
            out.extend_from_slice(b"usage: [USAGE]\n");
            continue;
        }
        if in_usage {
            if is_argparse_error_line(line) {
                in_usage = false;
                out.extend_from_slice(line);
            }
            continue;
        }
        // Python traceback: keep only the exception line (the first non-indented line after the header).
        if line.starts_with(b"Traceback (most recent call last):") {
            while lines
                .get(i)
                .is_some_and(|next| next.starts_with(b" ") || next.starts_with(b"\t"))
            {
                i = i.saturating_add(1);
            }
            last_exception = Some(out.len());
            continue;
        }
        // A chained traceback (`\nDuring handling of the above exception, …:\n\n`): the chain reduces to its final
        // exception line. How many links CPython prints for one EPIPE depends on when the reader went away.
        if let Some(start) = last_exception
            && split_eol(line).0.is_empty()
            && lines.get(i).is_some_and(|next| is_chain_separator(next))
        {
            out.truncate(start);
            i = i.saturating_add(1);
            if lines
                .get(i)
                .is_some_and(|next| split_eol(next).0.is_empty())
            {
                i = i.saturating_add(1);
            }
            continue;
        }
        // Only the line right after a traceback is its exception line; anything later ends the chain.
        if last_exception.is_some_and(|start| start != out.len()) {
            last_exception = None;
        }
        // YAML error bodies: keep `=====> ` + the first line, drop the libyaml mark lines.
        if line.starts_with(b"  in \"<unicode string>\"") || line.starts_with(b"  in \"<file>\"") {
            continue;
        }
        let l = mask_winerror(line);
        let l = mask_version(&l);
        out.extend_from_slice(&l);
    }
    out
}

/// The separator lines CPython prints between the links of an exception chain.
fn is_chain_separator(line: &[u8]) -> bool {
    let (body, _) = split_eol(line);
    body == b"During handling of the above exception, another exception occurred:"
        || body == b"The above exception was the direct cause of the following exception:"
}

/// `[WinError 32] <localized text>: 'path'` → `[WinError 32] [TEXT]: 'path'`.
fn mask_winerror(line: &[u8]) -> Vec<u8> {
    let key = b"[WinError ";
    let Some(start) = crate::text::find_bytes(line, key) else {
        return line.to_vec();
    };
    let after = start.saturating_add(key.len());
    let rest = line.get(after..).unwrap_or_default();
    let d = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if d == 0 || rest.get(d) != Some(&b']') {
        return line.to_vec();
    }
    let text_start = after.saturating_add(d).saturating_add(1);
    let text = line.get(text_start..).unwrap_or_default();
    let end = crate::text::find_bytes(text, b": ").unwrap_or_else(|| {
        text.len()
            .saturating_sub(usize::from(text.ends_with(b"\n")))
    });
    let mut out = line.get(..text_start).unwrap_or_default().to_vec();
    out.extend_from_slice(b" [TEXT]");
    out.extend_from_slice(text.get(end..).unwrap_or_default());
    out
}

/// `pre-commit 4.7.0` / `snp 2.0.1` / `version 4.7.0` → `… [VERSION]`.
fn mask_version(line: &[u8]) -> Vec<u8> {
    let mut cur = line.to_vec();
    for prefix in [&b"pre-commit "[..], b"snp ", b"version "] {
        let mut out = Vec::new();
        let mut i = 0_usize;
        while let Some(rest) = cur.get(i..) {
            if rest.is_empty() {
                break;
            }
            if rest.starts_with(prefix) {
                let tail = rest.get(prefix.len()..).unwrap_or_default();
                let n = version_len(tail);
                if n > 0 {
                    out.extend_from_slice(prefix);
                    out.extend_from_slice(b"[VERSION]");
                    i = i.saturating_add(prefix.len()).saturating_add(n);
                    continue;
                }
            }
            out.extend_from_slice(rest.get(..1).unwrap_or_default());
            i = i.saturating_add(1);
        }
        cur = out;
    }
    cur
}

/// Length of a leading `N.N.N` version.
fn version_len(b: &[u8]) -> usize {
    let mut i = 0_usize;
    for part in 0..3 {
        if part > 0 {
            if b.get(i) != Some(&b'.') {
                return 0;
            }
            i = i.saturating_add(1);
        }
        let d = b
            .get(i..)
            .unwrap_or_default()
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        if d == 0 {
            return 0;
        }
        i = i.saturating_add(d);
    }
    i
}

// ---------------------------------------------------------------------------------------------------------------
// class DIAG

/// The always-on stderr diagnostics of class DIAG (COMPATIBILITY §5.8) as `(register ID, line prefix)`. A line of
/// snp's stderr starting with one of these is stripped (documented Tier-C rule, stderr only) unless the scenario
/// is that ID's own divergence scenario, which asserts it.
pub const DIAG_LINES: &[(&str, &str)] = &[
    ("D-DIAG-01", "[WARNING] git "),
    ("D-DIAG-02", "Replaced hook script from "),
    ("D-DIAG-02", "Removed "),
    ("D-DIAG-02", "Note: "),
    ("D-DIAG-03", "hint: "),
    (
        "D-DIAG-04",
        "snp: waiting for another snp run in this worktree",
    ),
    ("D-DIAG-04", "snp: restoring stashed changes, please wait"),
    (
        "D-DIAG-04",
        "snp: git needed the terminal during the restore and was stopped",
    ),
    ("D-DIAG-04", "[WARNING] snp: a previous run"),
    ("D-DIAG-05", "[WARNING] snp store "),
    ("D-DIAG-05", "Removed snp 1.x cache data from "),
    ("D-DIAG-05", "[WARNING] the stash journal "),
    (
        "D-DIAG-06",
        "snp: no .snp.yaml or .pre-commit-config.yaml file was found",
    ),
];

/// Strips DIAG lines from snp's stderr; returns the remaining bytes and the IDs whose lines were removed. Lines of
/// `keep` (the scenario's own ID) stay.
pub fn strip_diag(stderr: &[u8], keep: Option<&str>) -> (Vec<u8>, Vec<&'static str>) {
    let mut out = Vec::new();
    let mut ids: Vec<&'static str> = Vec::new();
    for line in lines_with_ends(stderr) {
        let hit = DIAG_LINES.iter().find(|(id, prefix)| {
            Some(*id) != keep && line.starts_with(prefix.as_bytes()) && diag_shape(id, line)
        });
        match hit {
            Some((id, _)) => {
                if !ids.contains(id) {
                    ids.push(id);
                }
            }
            None => out.extend_from_slice(line),
        }
    }
    (out, ids)
}

/// Extra shape checks so a generic prefix (`Removed `, `Note: `, `[WARNING] git `) only matches the diagnostic.
fn diag_shape(id: &str, line: &[u8]) -> bool {
    match id {
        "D-DIAG-01" => contains_bytes(line, b"is older than 2.31.0"),
        "D-DIAG-02" => {
            contains_bytes(line, b"hook script from ")
                || contains_bytes(line, b"still runs ")
                || contains_bytes(line, b"Replaced hook script")
        }
        "D-DIAG-03" => contains_bytes(line, b", not snp; run `"),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> BrandMap {
        BrandMap::workspace().unwrap()
    }

    #[test]
    fn workspace_map_loads_and_validates() {
        let m = map();
        assert!(!m.is_identity());
        assert!(
            m.whole_line()
                .iter()
                .any(|r| r.from == "pre-commit 4.7.0" && r.to == "snp 2.0.1")
        );
        assert!(m.whole_line().iter().any(|r| r.from == "snp 4.7.0"));
        assert!(m.never().iter().any(|n| n == ".pre-commit-config.yaml"));
        let bad = "version: 1\nsubstitutions:\n  - {from: \"pre-commit-config\", to: \"x\"}\nnever: [\".pre-commit-config.yaml\"]\n";
        assert!(BrandMap::parse(bad).is_err());
        assert!(BrandMap::parse("version: 2\n").is_err());
        assert!(BrandMap::parse("version: 1\nsubstitutions: [{from: \"\", to: x}]\n").is_err());
    }

    #[test]
    fn version_line_is_a_whole_line_rule() {
        let m = map();
        assert_eq!(m.apply(b"pre-commit 4.7.0\n"), b"snp 2.0.1\n");
        assert_eq!(m.apply(b"snp 4.7.0\n"), b"snp 2.0.1\n");
        // Not a plain substring rule: inside a sentence it stays.
        assert_eq!(
            m.apply(b"see pre-commit 4.7.0 docs\n"),
            b"see pre-commit 4.7.0 docs\n"
        );
    }

    #[test]
    fn substitutions_follow_compatibility_3_3() {
        let m = map();
        let cases: &[(&[u8], &[u8])] = &[
            (b"pre-commit installed at .git/hooks/pre-commit\n", b"snp installed at .git/hooks/pre-commit\n"),
            (b"Use -f to use only pre-commit.\n", b"Use -f to use only snp.\n"),
            (
                b"If you are seeing this message in CI, reproduce locally with: `pre-commit run --all-files`.\n",
                b"If you are seeing this message in CI, reproduce locally with: `snp run --all-files`.\n",
            ),
            (b"Check the log at /r/store/pre-commit.log\n", b"Check the log at /r/store/snp.log\n"),
            (b"pre-commit hook(s) made changes.\n", b"pre-commit hook(s) made changes.\n"),
            (b"[ERROR] Your pre-commit configuration is unstaged.\n", b"[ERROR] Your pre-commit configuration is unstaged.\n"),
            (b"see https://pre-commit.com/#plugins\n", b"see https://pre-commit.com/#plugins\n"),
        ];
        for (from, to) in cases {
            assert_eq!(m.apply(from), *to, "{}", String::from_utf8_lossy(from));
        }
    }

    #[test]
    fn argparse_blocks_are_never_substituted() {
        let m = map();
        let usage = b"usage: snp run [-h] [--color {auto,always,never}]\n               [--files [FILES ...]]\nsnp run: error: unrecognized arguments: --bogus\npre-commit installed at x\n";
        let out = m.apply(usage);
        assert!(out.starts_with(b"usage: snp run [-h]"));
        assert!(contains_bytes(&out, b"snp installed at x\n"));
        let help = b"usage: snp install [-h]\n\noptions:\n  -t  Use -f to use only pre-commit.\n";
        assert_eq!(m.apply(help), help);
    }

    #[test]
    fn apply_invert_round_trip() {
        let m = map();
        let mut corpus: Vec<Vec<u8>> = m
            .substitutions()
            .iter()
            .map(|r| format!("x {} y\n", r.from).into_bytes())
            .collect();
        corpus.push(b"pre-commit 4.7.0\n".to_vec());
        corpus.push(b"plain text with nothing to map\r\n".to_vec());
        corpus.push(
            b"An error has occurred: FatalError: x\nCheck the log at /tmp/r/store/pre-commit.log\n"
                .to_vec(),
        );
        corpus.push(b"usage: snp [-h]\nsnp: error: argument: invalid choice: 'x'\n".to_vec());
        for t in &corpus {
            let forward = m.apply(t);
            assert_eq!(&m.invert(&forward), t, "{}", String::from_utf8_lossy(t));
        }
        for r in m.substitutions() {
            let snp = format!("{}\n", r.to).into_bytes();
            assert_eq!(m.apply(&m.invert(&snp)), snp, "{}", r.to);
        }
        assert_eq!(
            BrandMap::identity().apply(b"pre-commit 4.7.0\n"),
            b"pre-commit 4.7.0\n"
        );
    }

    #[test]
    fn py312_help_lines_are_canonicalised() {
        let py312 = b"options:\n  -h, --help            show this help message and exit\n  -c CONFIG, --config CONFIG\n                        Path to alternate config file\n  -j JOBS, --jobs JOBS  Number of threads to use. (default 1).\n  -t {pre-commit,pre-push}, --hook-type {pre-commit,pre-push}\n  --from-ref FROM_REF, --source FROM_REF, -s FROM_REF\n                        x\n";
        let py313 = b"options:\n  -h, --help            show this help message and exit\n  -c, --config CONFIG   Path to alternate config file\n  -j, --jobs JOBS       Number of threads to use. (default 1).\n  -t, --hook-type {pre-commit,pre-push}\n  --from-ref, --source, -s FROM_REF\n                        x\n";
        assert_eq!(canonical_help(py312), py313.to_vec());
        assert_eq!(canonical_help(py313), py313.to_vec());
    }

    #[test]
    fn py312_usage_is_canonicalised() {
        let py312 = b"usage: pre-commit [-h] [-V]\n                  {autoupdate,run,help,hook-impl}\n                  ...\npre-commit: error: unrecognized arguments: --bogus\n";
        let py313 = b"usage: pre-commit [-h] [-V]\n                  {autoupdate,run,help,hook-impl} ...\npre-commit: error: unrecognized arguments: --bogus\n";
        assert_eq!(canonical_usage(py312), py313.to_vec());
        assert_eq!(canonical_usage(py313), py313.to_vec());
        assert_eq!(canonical_usage(b"}\n   ...\n"), b"}\n   ...\n");
        // Windows text mode
        assert_eq!(canonical_usage(&crlf(py312)), crlf(py313));
    }

    #[test]
    fn py312_crlf_help_lines_are_canonicalised() {
        let py312 = b"options:\r\n  -c CONFIG, --config CONFIG\r\n                        Path to alternate config file\r\n  -t {a,b}, --hook-type {a,b}\r\n  --to-ref TO_REF, --origin TO_REF, -o TO_REF\r\n";
        let py313 = b"options:\r\n  -c, --config CONFIG   Path to alternate config file\r\n  -t, --hook-type {a,b}\r\n  --to-ref, --origin, -o TO_REF\r\n";
        assert_eq!(canonical_help(py312), py313.to_vec());
        assert_eq!(canonical_help(py313), py313.to_vec());
    }

    #[test]
    fn snp_only_parsers_and_top_level_choices() {
        let m = map();
        let oracle = b"usage: snp [-h] [-V]\n           {autoupdate,clean,gc,hazmat,init-templatedir,install,install-hooks,migrate-config,run,sample-config,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...\nsnp: error: unrecognized arguments: --bogus\n";
        let snp = b"usage: snp [-h] [-V]\n           {autoupdate,cache,clean,completions,config,doctor,gc,hazmat,init-templatedir,install,install-hooks,list,migrate,migrate-config,run,sample-config,self,try-repo,uninstall,validate-config,validate-manifest,help,hook-impl} ...\nsnp: error: unrecognized arguments: --bogus\n";
        assert_eq!(m.apply(oracle), snp.to_vec());
        assert_eq!(m.invert(snp), oracle.to_vec());
        let run = b"usage: snp run [-h] [--color {auto,always,never}]\n               [hook]\nsnp run: error: argument --hook-stage: invalid choice: 'x'\nafter\n";
        assert_eq!(
            m.reduce_snp_only_parsers(run),
            b"usage: snp run [SNP-ONLY OPTIONS]\nsnp run: error: argument --hook-stage: invalid choice: 'x'\nafter\n".to_vec()
        );
        let gc = b"usage: snp gc [-h]\nsnp gc: error: x\n";
        assert_eq!(m.reduce_snp_only_parsers(gc), gc.to_vec());
    }

    /// The argparse rules agree with WP-12's snp-brand goldens (the source of truth for snp-only parsers).
    #[test]
    fn argparse_rules_match_the_snp_cli_goldens() {
        let m = map();
        let goldens = std::fs::read_to_string(
            crate::workspace_root().join("crates/snp-cli/tests/goldens/snp.json"),
        )
        .unwrap();
        for r in m.argparse_substitutions() {
            assert!(
                goldens.contains(&r.to.replace('\'', "\\u0027")) || goldens.contains(&r.to),
                "{}",
                r.to
            );
        }
        for p in m.snp_only_option_parsers() {
            assert!(goldens.contains(&format!("usage: snp {p} ")), "{p}");
        }
    }

    #[test]
    fn tier_c_masks_the_ignored_items() {
        let out = normalise(
            Tier::C,
            b"- duration: 0.03s\n\x1b[2m- duration: 1.0s\x1b[m\n[INFO] Stashing unstaged files to /r/store/patch1700000000-4242.\nrepoab_cd123 tmp1234abcd repoTooLongForIt\n",
            &BrandMap::identity(),
        );
        assert_eq!(
            out,
            b"- duration: [D]s\n\x1b[2m- duration: [D]s\x1b[m\n[INFO] Stashing unstaged files to /r/store/patch[TP].\nrepo[RAND] tmp[RAND] repoTooLongForIt\n"
        );
        // snp's own temporary directories (Rust `tempfile` names); `tmpfiles` is a word, not a suffix
        assert_eq!(
            normalise(
                Tier::C,
                b"/t/tmpLVNaLv/.pre-commit-hooks.yaml /t/tmpAb3dE9gH/x tmpfiles\n",
                &BrandMap::identity()
            ),
            b"/t/tmp[RAND]/.pre-commit-hooks.yaml /t/tmp[RAND]/x tmpfiles\n"
        );
        let chatter = b"[INFO] Installing environment for x.\n[INFO] Once installed this environment will be reused.\nCollecting six\nnpm WARN x\nh.......Passed\n";
        assert_eq!(
            normalise(Tier::C, chatter, &BrandMap::identity()),
            b"[INFO] Installing environment for x.\n[INFO] Once installed this environment will be reused.\nh.......Passed\n"
        );
        assert_eq!(
            normalise(Tier::A, b"- duration: 0.03s\n", &BrandMap::identity()),
            b"- duration: 0.03s\n"
        );
        let shadow = b"-   repo: /r/tmp/tmpab12cd34/shadow-repo\n    rev: 508372f149f5f691200929308eaba68eb7ddd4da\n    rev: 508372f149f5f691200929308eaba68eb7ddd4da\n";
        assert_eq!(
            normalise(Tier::C, shadow, &BrandMap::identity()),
            b"-   repo: /r/tmp/tmp[RAND]/shadow-repo\n    rev: [SHADOW-REV]\n    rev: 508372f149f5f691200929308eaba68eb7ddd4da\n"
        );
        let shadow = b"-   repo: D:\\r\\tmp\\tmpab12cd34\\shadow-repo\r\n    rev: 508372f149f5f691200929308eaba68eb7ddd4da\r\n";
        assert_eq!(
            normalise(Tier::C, shadow, &BrandMap::identity()),
            b"-   repo: D:\\r\\tmp\\tmp[RAND]\\shadow-repo\r\n    rev: [SHADOW-REV]\r\n"
        );
    }

    #[test]
    fn tier_b_collapses_exception_chains() {
        let one = b"Traceback (most recent call last):\n  File \"a\", line 1\nBrokenPipeError: [Errno 32] Broken pipe\n\nDuring handling of the above exception, another exception occurred:\n\nTraceback (most recent call last):\n  File \"b\", line 2\nBrokenPipeError: [Errno 32] Broken pipe\nException ignored in: x\n";
        let two = b"Traceback (most recent call last):\n  File \"a\", line 1\nBrokenPipeError: [Errno 32] Broken pipe\n\nDuring handling of the above exception, another exception occurred:\n\nTraceback (most recent call last):\n  File \"b\", line 2\nBrokenPipeError: [Errno 32] Broken pipe\n\nThe above exception was the direct cause of the following exception:\n\nTraceback (most recent call last):\n  File \"c\", line 3\nBrokenPipeError: [Errno 32] Broken pipe\nException ignored in: x\n";
        let want = b"BrokenPipeError: [Errno 32] Broken pipe\nException ignored in: x\n";
        assert_eq!(normalise(Tier::B, one, &BrandMap::identity()), want);
        assert_eq!(normalise(Tier::B, two, &BrandMap::identity()), want);
        // A blank line that is not followed by a chain separator stays.
        let plain = b"Traceback (most recent call last):\n  x\nValueError: v\n\nnext\n";
        assert_eq!(
            normalise(Tier::B, plain, &BrandMap::identity()),
            b"ValueError: v\n\nnext\n"
        );
    }

    /// The traceback of `pygrep.py` on a missing file, as CPython 3.12 prints it (`~` markers from 3.13 on).
    const PYGREP_TRACEBACK: &str = concat!(
        "Traceback (most recent call last):\n",
        "  File \"<frozen runpy>\", line 198, in _run_module_as_main\n",
        "  File \"<frozen runpy>\", line 88, in _run_code\n",
        "  File \"/v/lib/python3.12/site-packages/pre_commit/languages/pygrep.py\", line 133, in <module>\n",
        "    raise SystemExit(main())\n",
        "                     ^^^^^^\n",
        "  File \"/v/lib/python3.12/site-packages/pre_commit/languages/pygrep.py\", line 128, in main\n",
        "    retv |= process_fn(pattern, filename)\n",
        "            ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^\n",
        "  File \"/v/lib/python3.12/site-packages/pre_commit/languages/pygrep.py\", line 24, in _process_filename_by_line\n",
        "    with open(filename, 'rb') as f:\n",
        "         ^^^^^^^^^^^^^^^^^^^^\n",
        "FileNotFoundError: [Errno 2] No such file or directory: 'TODO'\n",
    );

    #[test]
    fn generated_reduces_the_pygrep_open_traceback() {
        const LAST: &[u8] = b"FileNotFoundError: [Errno 2] No such file or directory: 'TODO'\n";
        let mut text = b"- exit code: 1\n\n".to_vec();
        text.extend_from_slice(PYGREP_TRACEBACK.as_bytes());
        text.extend_from_slice(PYGREP_TRACEBACK.as_bytes());
        text.extend_from_slice(b"\nnext hook\n");
        let mut want = b"- exit code: 1\n\n".to_vec();
        want.extend_from_slice(LAST);
        want.extend_from_slice(LAST);
        want.extend_from_slice(b"\nnext hook\n");
        assert_eq!(reduce_generated(&text, Brand::Compat), want);
        assert_eq!(reduce_generated(&text, Brand::Snp), want);
    }

    #[test]
    fn generated_reduces_the_coloured_traceback_of_python_313() {
        let text = concat!(
            "Traceback (most recent call last):\n",
            "  File \x1b[35m\"/v/pre_commit/languages/pygrep.py\"\x1b[0m, line \x1b[35m24\x1b[0m, in \x1b[35mf\x1b[0m\n",
            "    with \x1b[31mopen\x1b[0m\x1b[1;31m(filename, 'rb')\x1b[0m as f:\n",
            "         \x1b[31m~~~~\x1b[0m\x1b[1;31m^^^^^^^^^^^^^^^^\x1b[0m\n",
            "\x1b[1;35mFileNotFoundError\x1b[0m: \x1b[35m[Errno 2] No such file or directory: ''\x1b[0m\n",
        );
        assert_eq!(
            reduce_generated(text.as_bytes(), Brand::Snp),
            b"FileNotFoundError: [Errno 2] No such file or directory: ''\n".to_vec()
        );
    }

    #[test]
    fn generated_leaves_every_other_traceback_alone() {
        // pygrep's own frames, but not its `open()`: an invalid entry stays D-MISC-05's
        let invalid_entry = PYGREP_TRACEBACK.replace(
            "    with open(filename, 'rb') as f:\n",
            "    pattern = re.compile(args.pattern.encode(), flags)\n",
        );
        // `open()` in the same words, somewhere other than pygrep
        let elsewhere = PYGREP_TRACEBACK.replace("pre_commit/languages/pygrep.py", "tool.py");
        // a source line after `open()` must end the match: the failure is not `open()`'s
        let later_frame = PYGREP_TRACEBACK.replace(
            "         ^^^^^^^^^^^^^^^^^^^^\n",
            "         ^^^^^^^^^^^^^^^^^^^^\n    f.read()\n",
        );
        let truncated = PYGREP_TRACEBACK
            .strip_suffix("FileNotFoundError: [Errno 2] No such file or directory: 'TODO'\n")
            .unwrap();
        for text in [invalid_entry.as_str(), &elsewhere, &later_frame, truncated] {
            assert_eq!(
                reduce_generated(text.as_bytes(), Brand::Compat),
                text.as_bytes().to_vec()
            );
        }
    }

    #[test]
    fn generated_masks_the_argument_list_of_one_brand_only() {
        let py312 = b"pygrep.py: error: unrecognized arguments: -x=1 -dash.txt a.py b.py\n";
        assert_eq!(
            reduce_generated(py312, Brand::Compat),
            b"pygrep.py: error: unrecognized arguments: [ARGS]\n".to_vec()
        );
        // the snp brand holds this line to Python 3.13, the compat brand the next one
        assert_eq!(reduce_generated(py312, Brand::Snp), py312.to_vec());
        let top = b"snp: error: unrecognized arguments: -dash.txt CRLF.txt\r\n";
        assert_eq!(
            reduce_generated(top, Brand::Snp),
            b"snp: error: unrecognized arguments: [ARGS]\r\n".to_vec()
        );
        assert_eq!(reduce_generated(top, Brand::Compat), top.to_vec());
        // not a usage error at all
        let other = b"see pygrep.py: error: unrecognized arguments: x\n";
        assert_eq!(reduce_generated(other, Brand::Compat), other.to_vec());
    }

    #[test]
    fn strip_ansi_only_removes_sgr_sequences() {
        assert_eq!(strip_ansi(b"\x1b[1;35mA\x1b[0m: \x1b[m"), b"A: ".to_vec());
        assert_eq!(strip_ansi(b"\x1b[2Kkept"), b"\x1b[2Kkept".to_vec());
        assert_eq!(strip_ansi(b"tail\x1b["), b"tail\x1b[".to_vec());
    }

    #[test]
    fn tier_b_reduces_semantically() {
        let tb = b"Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    boom()\nValueError: boom\n";
        assert_eq!(
            normalise(Tier::B, tb, &BrandMap::identity()),
            b"ValueError: boom\n"
        );
        let usage = b"usage: pre-commit run [-h] [--color {auto,always,never}]\n                      [-c CONFIG]\npre-commit run: error: unrecognized arguments: --bogus\n";
        assert_eq!(
            normalise(Tier::B, usage, &BrandMap::identity()),
            b"usage: [USAGE]\npre-commit run: error: unrecognized arguments: --bogus\n"
        );
        let yaml = b"==> File x.yaml\n=====> while parsing a flow node\ndid not find expected node content\n  in \"<unicode string>\", line 2, column 1\n";
        assert_eq!(normalise(Tier::B, yaml, &BrandMap::identity()), b"==> File x.yaml\n=====> while parsing a flow node\ndid not find expected node content\n");
        assert_eq!(
            normalise(
                Tier::B,
                b"PermissionError: [WinError 32] Der Prozess kann nicht: 'C:\\\\x'\n", // spellchecker:disable-line
                &BrandMap::identity()
            ),
            b"PermissionError: [WinError 32] [TEXT]: 'C:\\\\x'\n"
        );
        assert_eq!(
            normalise(Tier::B, b"pre-commit 4.7.0\n", &BrandMap::identity()),
            b"pre-commit [VERSION]\n"
        );
    }

    #[test]
    fn store_paths_are_masked() {
        let out = mask_paths(
            b"/r/snp-store/patches/patch[TP] /r/store/pre-commit.log",
            &[b"/r/store", b"/r/snp-store"],
        );
        assert_eq!(out, b"[STORE]/patch[TP] [STORE]/pre-commit.log");
        let out = mask_paths(
            b"C:\\r\\snp-store\\patches\\patch[TP]",
            &[b"C:\\r\\snp-store"],
        );
        assert_eq!(out, b"[STORE]\\patch[TP]");
    }

    #[test]
    fn diag_lines_are_stripped_from_stderr_only_when_foreign() {
        let err = b"[WARNING] git 2.30.0 is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.\nreal error\n";
        let (rest, ids) = strip_diag(err, None);
        assert_eq!(rest, b"real error\n");
        assert_eq!(ids, vec!["D-DIAG-01"]);
        let (kept, ids) = strip_diag(err, Some("D-DIAG-01"));
        assert_eq!(kept, err);
        assert_eq!(ids, Vec::<&str>::new());
        let (rest, _) = strip_diag(
            b"[WARNING] git said something else\nRemoved nothing\n",
            None,
        );
        assert_eq!(
            rest,
            b"[WARNING] git said something else\nRemoved nothing\n"
        );
    }
}
