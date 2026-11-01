//! Normalisation tiers and the brand map (ARCHITECTURE §13.2, §9.6; COMPATIBILITY §2 "Comparison tiers", §3.3;
//! research 10 §7.4.5).
//!
//! * **Tier A** — byte-exact: only the brand map is applied (snp-brand runs).
//! * **Tier C** — the "ignored" items are masked on both sides: `- duration: …s`, stash patch names and pids, the
//!   store roots (snp's store differs from pre-commit's by design, FD-2), random `repo…`/`tmp…` suffixes and the
//!   installer chatter between `[INFO] Installing environment for …` and the next status line. This is what the
//!   `exact` compare mode uses: everything else stays byte-exact.
//! * **Tier B** — Tier C plus the semantic reductions: Python tracebacks to their last line, argparse usage blocks
//!   to their `error:` lines (the py312 oracle wraps differently, 12 §A.4.1), YAML error bodies below the `=====> `
//!   line, localized `[WinError N]` texts and version numbers.
//!
//! The brand map ([`BrandMap`]) is the machine form of COMPATIBILITY §3.3 (`tests/differential/brand_map.yaml`). It
//! is applied to the prog-patched oracle's output in snp-brand runs, never inside argparse blocks (a `usage: `
//! line through the next `…: error: …` line, or to the end of a help text), whose bytes the patched oracle already
//! renders with prog `snp` (ADR-058).
//!
//! Owner: WP-35.

use std::path::Path;

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
    /// The compat (pre-commit 4.6.2) text.
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
}

impl BrandMap {
    /// The identity map (compat-brand runs).
    pub fn identity() -> Self {
        Self::default()
    }

    /// Whether the map changes nothing.
    pub fn is_identity(&self) -> bool {
        self.whole_line.is_empty() && self.substitutions.is_empty()
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
            let Some(list) = doc.get(key) else { return Ok(Vec::new()) };
            let items = list.as_seq().ok_or_else(|| format!("brand_map.{key}: expected a list"))?;
            let mut out = Vec::new();
            for it in items {
                let from = it.get("from").and_then(Yv::as_str).ok_or_else(|| format!("brand_map.{key}: rule without `from`"))?;
                let to = it.get("to").and_then(Yv::as_str).ok_or_else(|| format!("brand_map.{key}: rule without `to`"))?;
                if from.is_empty() || from.contains('\n') {
                    return Err(format!("brand_map.{key}: bad `from` {from:?}"));
                }
                out.push(Rule { from: from.to_owned(), to: to.to_owned() });
            }
            Ok(out)
        };
        let whole_line = rules("whole_line")?;
        let substitutions = rules("substitutions")?;
        let mut never = Vec::new();
        if let Some(list) = doc.get("never") {
            for it in list.as_seq().ok_or("brand_map.never: expected a list")? {
                never.push(it.as_str().ok_or("brand_map.never: expected strings")?.to_owned());
            }
        }
        for r in &substitutions {
            if let Some(n) = never.iter().find(|n| n.contains(r.from.as_str()) || r.from.contains(n.as_str())) {
                return Err(format!("brand_map: rule {:?} would touch the protected text {n:?}", r.from));
            }
        }
        Ok(Self { whole_line, substitutions, never })
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
                out.extend_from_slice(line);
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
            let (from, to) = if inverse { (&r.to, &r.from) } else { (&r.from, &r.to) };
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
    line.split_at_checked(line.len().saturating_sub(n)).unwrap_or((line, &[]))
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
        Tier::C => tier_c(&mapped),
        Tier::B => tier_b(&tier_c(&mapped)),
    }
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
    // snp keeps its patches in `patches/` (ARCHITECTURE §5.2): `[STORE]/patches/patch…` ≡ `[STORE]/patch…`.
    replace_bytes(&cur, b"[STORE]/patches/patch", b"[STORE]/patch")
}

// ---------------------------------------------------------------------------------------------------------------
// Tier C

fn tier_c(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut in_install = false;
    for line in lines_with_ends(b) {
        if in_install {
            if is_status_line(line) {
                in_install = false;
            } else if !is_install_note(line) {
                continue;
            }
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
    let Some(start) = crate::text::find_bytes(line, key) else { return line.to_vec() };
    let after = start.saturating_add(key.len());
    let rest = line.get(after..).unwrap_or_default();
    let num = rest.iter().take_while(|c| c.is_ascii_digit() || **c == b'.').count();
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
                    let d2 = tail.get(d1.saturating_add(1)..).unwrap_or_default().iter().take_while(|c| c.is_ascii_digit()).count();
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
fn mask_rand(line: &[u8], prefix: &[u8], repl: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0_usize;
    let is_rand = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_';
    while let Some(rest) = line.get(i..) {
        if rest.is_empty() {
            break;
        }
        let boundary = i.checked_sub(1).and_then(|p| line.get(p)).is_none_or(|c| !c.is_ascii_alphanumeric());
        if boundary && rest.starts_with(prefix) {
            let tail = rest.get(prefix.len()..).unwrap_or_default();
            let n = tail.iter().take_while(|c| is_rand(c)).count();
            if n == 8 {
                out.extend_from_slice(repl);
                i = i.saturating_add(prefix.len()).saturating_add(8);
                continue;
            }
        }
        out.extend_from_slice(rest.get(..1).unwrap_or_default());
        i = i.saturating_add(1);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Tier B

fn tier_b(b: &[u8]) -> Vec<u8> {
    let lines = lines_with_ends(b);
    let mut out = Vec::new();
    let mut iter = lines.iter().peekable();
    let mut in_usage = false;
    while let Some(line) = iter.next() {
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
            while let Some(next) = iter.peek() {
                if next.starts_with(b" ") || next.starts_with(b"\t") {
                    iter.next();
                } else {
                    break;
                }
            }
            continue;
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

/// `[WinError 32] <localized text>: 'path'` → `[WinError 32] [TEXT]: 'path'`.
fn mask_winerror(line: &[u8]) -> Vec<u8> {
    let key = b"[WinError ";
    let Some(start) = crate::text::find_bytes(line, key) else { return line.to_vec() };
    let after = start.saturating_add(key.len());
    let rest = line.get(after..).unwrap_or_default();
    let d = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if d == 0 || rest.get(d) != Some(&b']') {
        return line.to_vec();
    }
    let text_start = after.saturating_add(d).saturating_add(1);
    let text = line.get(text_start..).unwrap_or_default();
    let end = crate::text::find_bytes(text, b": ").unwrap_or_else(|| text.len().saturating_sub(usize::from(text.ends_with(b"\n"))));
    let mut out = line.get(..text_start).unwrap_or_default().to_vec();
    out.extend_from_slice(b" [TEXT]");
    out.extend_from_slice(text.get(end..).unwrap_or_default());
    out
}

/// `pre-commit 4.6.2` / `snp 2.0.0` / `version 4.6.2` → `… [VERSION]`.
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
        let d = b.get(i..).unwrap_or_default().iter().take_while(|c| c.is_ascii_digit()).count();
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
    ("D-DIAG-04", "snp: waiting for another snp run in this worktree"),
    ("D-DIAG-04", "snp: restoring stashed changes, please wait"),
    ("D-DIAG-04", "snp: git needed the terminal during the restore and was stopped"),
    ("D-DIAG-04", "[WARNING] snp: a previous run"),
    ("D-DIAG-05", "[WARNING] snp store "),
    ("D-DIAG-05", "Removed snp 1.x cache data from "),
];

/// Strips DIAG lines from snp's stderr; returns the remaining bytes and the IDs whose lines were removed. Lines of
/// `keep` (the scenario's own ID) stay.
pub fn strip_diag(stderr: &[u8], keep: Option<&str>) -> (Vec<u8>, Vec<&'static str>) {
    let mut out = Vec::new();
    let mut ids: Vec<&'static str> = Vec::new();
    for line in lines_with_ends(stderr) {
        let hit = DIAG_LINES.iter().find(|(id, prefix)| Some(*id) != keep && line.starts_with(prefix.as_bytes()) && diag_shape(id, line));
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
            contains_bytes(line, b"hook script from ") || contains_bytes(line, b"still runs ") || contains_bytes(line, b"Replaced hook script")
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
        assert!(m.whole_line().iter().any(|r| r.from == "pre-commit 4.6.2" && r.to == "snp 2.0.0"));
        assert!(m.whole_line().iter().any(|r| r.from == "snp 4.6.2"));
        assert!(m.never().iter().any(|n| n == ".pre-commit-config.yaml"));
        let bad = "version: 1\nsubstitutions:\n  - {from: \"pre-commit-config\", to: \"x\"}\nnever: [\".pre-commit-config.yaml\"]\n";
        assert!(BrandMap::parse(bad).is_err());
        assert!(BrandMap::parse("version: 2\n").is_err());
        assert!(BrandMap::parse("version: 1\nsubstitutions: [{from: \"\", to: x}]\n").is_err());
    }

    #[test]
    fn version_line_is_a_whole_line_rule() {
        let m = map();
        assert_eq!(m.apply(b"pre-commit 4.6.2\n"), b"snp 2.0.0\n");
        assert_eq!(m.apply(b"snp 4.6.2\n"), b"snp 2.0.0\n");
        // Not a plain substring rule: inside a sentence it stays.
        assert_eq!(m.apply(b"see pre-commit 4.6.2 docs\n"), b"see pre-commit 4.6.2 docs\n");
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
        let mut corpus: Vec<Vec<u8>> = m.substitutions().iter().map(|r| format!("x {} y\n", r.from).into_bytes()).collect();
        corpus.push(b"pre-commit 4.6.2\n".to_vec());
        corpus.push(b"plain text with nothing to map\r\n".to_vec());
        corpus.push(b"An error has occurred: FatalError: x\nCheck the log at /tmp/r/store/pre-commit.log\n".to_vec());
        corpus.push(b"usage: snp [-h]\nsnp: error: argument: invalid choice: 'x'\n".to_vec());
        for t in &corpus {
            let forward = m.apply(t);
            assert_eq!(&m.invert(&forward), t, "{}", String::from_utf8_lossy(t));
        }
        for r in m.substitutions() {
            let snp = format!("{}\n", r.to).into_bytes();
            assert_eq!(m.apply(&m.invert(&snp)), snp, "{}", r.to);
        }
        assert_eq!(BrandMap::identity().apply(b"pre-commit 4.6.2\n"), b"pre-commit 4.6.2\n");
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
        let chatter = b"[INFO] Installing environment for x.\n[INFO] Once installed this environment will be reused.\nCollecting six\nnpm WARN x\nh.......Passed\n";
        assert_eq!(
            normalise(Tier::C, chatter, &BrandMap::identity()),
            b"[INFO] Installing environment for x.\n[INFO] Once installed this environment will be reused.\nh.......Passed\n"
        );
        assert_eq!(normalise(Tier::A, b"- duration: 0.03s\n", &BrandMap::identity()), b"- duration: 0.03s\n");
    }

    #[test]
    fn tier_b_reduces_semantically() {
        let tb = b"Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\n    boom()\nValueError: boom\n";
        assert_eq!(normalise(Tier::B, tb, &BrandMap::identity()), b"ValueError: boom\n");
        let usage = b"usage: pre-commit run [-h] [--color {auto,always,never}]\n                      [-c CONFIG]\npre-commit run: error: unrecognized arguments: --bogus\n";
        assert_eq!(normalise(Tier::B, usage, &BrandMap::identity()), b"usage: [USAGE]\npre-commit run: error: unrecognized arguments: --bogus\n");
        let yaml = b"==> File x.yaml\n=====> while parsing a flow node\ndid not find expected node content\n  in \"<unicode string>\", line 2, column 1\n";
        assert_eq!(normalise(Tier::B, yaml, &BrandMap::identity()), b"==> File x.yaml\n=====> while parsing a flow node\ndid not find expected node content\n");
        assert_eq!(
            normalise(Tier::B, b"PermissionError: [WinError 32] Der Prozess kann nicht: 'C:\\\\x'\n", &BrandMap::identity()),
            b"PermissionError: [WinError 32] [TEXT]: 'C:\\\\x'\n"
        );
        assert_eq!(normalise(Tier::B, b"pre-commit 4.6.2\n", &BrandMap::identity()), b"pre-commit [VERSION]\n");
    }

    #[test]
    fn store_paths_are_masked() {
        let out = mask_paths(b"/r/snp-store/patches/patch[TP] /r/store/pre-commit.log", &[b"/r/store", b"/r/snp-store"]);
        assert_eq!(out, b"[STORE]/patch[TP] [STORE]/pre-commit.log");
    }

    #[test]
    fn diag_lines_are_stripped_from_stderr_only_when_foreign() {
        let err = b"[WARNING] git 2.30.0 is older than 2.31.0, the oldest git version snp is tested with. Set SNP_ALLOW_OLD_GIT=1 to silence this warning.\nreal error\n";
        let (rest, ids) = strip_diag(err, None);
        assert_eq!(rest, b"real error\n");
        assert_eq!(ids, vec!["D-DIAG-01"]);
        let (kept, ids) = strip_diag(err, Some("D-DIAG-01"));
        assert_eq!(kept, err);
        assert!(ids.is_empty());
        let (rest, _) = strip_diag(b"[WARNING] git said something else\nRemoved nothing\n", None);
        assert_eq!(rest, b"[WARNING] git said something else\nRemoved nothing\n");
    }
}
