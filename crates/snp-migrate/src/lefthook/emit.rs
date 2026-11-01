//! The output model of a converted hook and its YAML rendering (block mappings, flow lists and maps, 2-space
//! indentation as in 05 §19.11). Scalars are written plain only when both PyYAML (pre-commit, YAML 1.1) and the
//! YAML 1.2 core schema read them back as the same string; otherwise single-quoted, or double-quoted with escapes
//! when they hold control characters or line breaks.
//!
//! Owner: WP-34.

use std::fmt::Write as _;

/// A `skip` / `only` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CondOut {
    /// `true`.
    Always,
    /// A list of items (OR).
    Items(Vec<CondItem>),
}

/// One condition item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CondItem {
    /// `merge`, `rebase`, `merge-commit`.
    State(String),
    /// A map whose keys all hold (`ref`, `run`).
    Map(Vec<(String, String)>),
}

/// One emitted hook (all fields optional except id/name/language/entry/stages).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "pre-commit's boolean hook keys"
)]
pub(crate) struct OutHook {
    /// `# snp-migrate: …` comment lines above the hook.
    pub(crate) comments: Vec<String>,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) language: String,
    pub(crate) entry: String,
    pub(crate) files: Option<String>,
    pub(crate) exclude: Option<String>,
    pub(crate) types: Vec<String>,
    pub(crate) types_or: Vec<String>,
    pub(crate) exclude_types: Vec<String>,
    /// `false` is written; `true` is pre-commit's default.
    pub(crate) pass_filenames: bool,
    pub(crate) always_run: bool,
    pub(crate) stages: Vec<String>,
    pub(crate) require_serial: bool,
    pub(crate) fail_fast: bool,
    pub(crate) verbose: bool,
    pub(crate) priority: Option<u32>,
    pub(crate) stage_fixed: bool,
    pub(crate) root: Option<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) groups: Vec<String>,
    pub(crate) skip: Option<CondOut>,
    pub(crate) only: Option<CondOut>,
    pub(crate) fail_text: Option<String>,
    pub(crate) timeout: Option<String>,
    pub(crate) use_stdin: bool,
    pub(crate) fail_on_changes: Option<String>,
}

/// One `key: value` line of a hook (`value` already rendered), in emission order.
pub(crate) fn hook_fields(h: &OutHook) -> Vec<(&'static str, String)> {
    let mut f: Vec<(&'static str, String)> = vec![
        ("id", scalar(&h.id, false)),
        ("name", scalar(&h.name, false)),
        ("language", scalar(&h.language, false)),
        ("entry", scalar(&h.entry, false)),
    ];
    if let Some(v) = &h.files {
        f.push(("files", regex(v)));
    }
    if let Some(v) = &h.exclude {
        f.push(("exclude", regex(v)));
    }
    for (k, v) in [
        ("types", &h.types),
        ("types_or", &h.types_or),
        ("exclude_types", &h.exclude_types),
    ] {
        if !v.is_empty() {
            f.push((k, flow_list(v)));
        }
    }
    if !h.pass_filenames {
        f.push(("pass_filenames", "false".to_owned()));
    }
    if h.always_run {
        f.push(("always_run", "true".to_owned()));
    }
    f.push(("stages", flow_list(&h.stages)));
    for (k, v) in [
        ("require_serial", h.require_serial),
        ("fail_fast", h.fail_fast),
        ("verbose", h.verbose),
    ] {
        if v {
            f.push((k, "true".to_owned()));
        }
    }
    if let Some(p) = h.priority {
        f.push(("priority", p.to_string()));
    }
    if h.stage_fixed {
        f.push(("stage_fixed", "true".to_owned()));
    }
    if let Some(r) = &h.root {
        f.push(("root", scalar(r, false)));
    }
    if !h.env.is_empty() {
        f.push(("env", flow_map(&h.env)));
    }
    if !h.groups.is_empty() {
        f.push(("groups", flow_list(&h.groups)));
    }
    if let Some(c) = &h.skip {
        f.push(("skip", cond(c)));
    }
    if let Some(c) = &h.only {
        f.push(("only", cond(c)));
    }
    if let Some(t) = &h.fail_text {
        f.push(("fail_text", scalar(t, false)));
    }
    if let Some(t) = &h.timeout {
        f.push(("timeout", scalar(t, false)));
    }
    if h.use_stdin {
        f.push(("use_stdin", "true".to_owned()));
    }
    if let Some(v) = &h.fail_on_changes {
        f.push(("fail_on_changes", scalar(v, false)));
    }
    f
}

/// Appends a hook as a block sequence item at `indent` spaces (`- id: …`).
pub(crate) fn write_hook(out: &mut String, h: &OutHook, indent: usize) {
    let pad: String = std::iter::repeat_n(' ', indent).collect();
    for c in &h.comments {
        writeln!(out, "{pad}# {}", one_line(c)).unwrap_or_default();
    }
    for (i, (k, v)) in hook_fields(h).iter().enumerate() {
        let lead = if i == 0 { "- " } else { "  " };
        writeln!(out, "{pad}{lead}{k}: {v}").unwrap_or_default();
    }
}

/// YAML's printable set as PyYAML's reader enforces it (`[\t\n\r\x20-\x7E\x85\xA0-\uD7FF\uE000-\uFFFD\U00010000-]`).
pub(crate) const fn yaml_printable(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='~' | '\u{85}' | '\u{A0}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}')
        || c >= '\u{10000}'
}

/// The text of a comment line: every YAML line break (`\n`, `\r`, U+0085, U+2028, U+2029) becomes a space, so a
/// value quoted in a note can never end the comment and start YAML; every other control or non-printable character
/// (and the BOM) is written as `\u{…}`, which also keeps terminal escape sequences out of the report.
pub(crate) fn one_line(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push(' '),
            '\t' => out.push(c),
            c if c.is_control() || c == '\u{feff}' || !yaml_printable(c) => {
                write!(out, "\\u{{{:x}}}", u32::from(c)).unwrap_or_default();
            }
            c => out.push(c),
        }
    }
    out
}

/// A `skip` / `only` value.
pub(crate) fn cond(c: &CondOut) -> String {
    match c {
        CondOut::Always => "true".to_owned(),
        CondOut::Items(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|i| match i {
                    CondItem::State(s) => scalar(s, true),
                    CondItem::Map(kv) => flow_map(kv),
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
    }
}

/// `[a, b]`.
pub(crate) fn flow_list(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| scalar(s, true)).collect();
    format!("[{}]", parts.join(", "))
}

/// `{K: V, …}`.
pub(crate) fn flow_map(kv: &[(String, String)]) -> String {
    let parts: Vec<String> = kv
        .iter()
        .map(|(k, v)| format!("{}: {}", scalar(k, true), scalar(v, true)))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// A regex: single-quoted unless it needs double quotes.
pub(crate) fn regex(s: &str) -> String {
    if needs_double(s) {
        double(s)
    } else {
        single(s)
    }
}

/// Whether a scalar must be double-quoted: it holds a control character, a line break, the BOM or a character
/// outside YAML's printable set (U+FFFE, U+FFFF, lone C1 controls), which only escapes can carry.
fn needs_double(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_control()
            || c == '\u{feff}'
            || c == '\u{2028}'
            || c == '\u{2029}'
            || c == '\u{85}'
            || !yaml_printable(c)
    })
}

fn single(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn double(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if needs_double(c.encode_utf8(&mut [0; 4])) => {
                let n = u32::from(c);
                if n <= 0xff {
                    write!(out, "\\x{n:02X}").unwrap_or_default();
                } else {
                    write!(out, "\\u{n:04X}").unwrap_or_default();
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether `s` can be written as a plain scalar (in a flow collection when `flow`).
fn plain_ok(s: &str, flow: bool) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    if s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) {
        return false;
    }
    if !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return false;
    }
    let second = s.chars().nth(1);
    let indicator_ok = match first {
        '-' => second.is_some_and(|c| c != ' ' && !(flow && ",[]{}".contains(c))),
        c => !",[]{}#&*!|>'\"%@`?:".contains(c),
    };
    if !indicator_ok || s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    if flow && s.contains([',', '[', ']', '{', '}']) {
        return false;
    }
    if s == "-" || s.starts_with("---") || s.starts_with("...") {
        return false;
    }
    snp_yaml::resolve_plain(s, snp_yaml::Schema::PyYaml11Safe) == snp_yaml::ImplicitTag::Str
        && snp_yaml::resolve_plain(s, snp_yaml::Schema::Yaml12Core) == snp_yaml::ImplicitTag::Str
}

/// A string scalar.
pub(crate) fn scalar(s: &str, flow: bool) -> String {
    if plain_ok(s, flow) {
        s.to_owned()
    } else if needs_double(s) {
        double(s)
    } else {
        single(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reads_back(rendered: &str, want: &str) {
        let doc = format!("k: {rendered}\nl: [{rendered}]\n");
        let v = snp_yaml::load(&doc, &snp_yaml::Limits::default()).unwrap();
        let m = v.as_map().unwrap();
        let mut it = m.iter();
        let k = it.next().unwrap().1;
        assert_eq!(k.as_str(), Some(want), "{doc:?}");
        let l = it.next().unwrap().1.as_array().unwrap();
        assert_eq!(l[0].as_str(), Some(want), "{doc:?}");
    }

    #[test]
    fn scalars() {
        assert_eq!(scalar("yarn eslint --fix", false), "yarn eslint --fix");
        assert_eq!(scalar("--parallel", true), "--parallel");
        assert_eq!(
            scalar("Run `yarn test` locally", false),
            "Run `yarn test` locally"
        );
        assert_eq!(scalar("5m", false), "5m");
        assert_eq!(scalar("yes", false), "'yes'");
        assert_eq!(scalar("1", false), "'1'");
        assert_eq!(scalar("1_000", false), "'1_000'");
        assert_eq!(scalar("null", false), "'null'");
        assert_eq!(scalar("a: b", false), "'a: b'");
        assert_eq!(scalar("a,b", true), "'a,b'");
        assert_eq!(scalar("a,b", false), "a,b");
        assert_eq!(scalar("", false), "''");
        assert_eq!(scalar("it's", false), "it's");
        assert_eq!(scalar("'x'", false), "'''x'''");
        assert_eq!(scalar("a\nb", false), "\"a\\nb\"");
        assert_eq!(scalar("sh -c 'x' --", false), "sh -c 'x' --");
        for s in [
            "yes", "1", "a: b", "a\nb", "'x'", "", "~", "x #y", "-", "- a", "?x", "\u{7}", "é",
            "{a}", "*a", "=", "<<",
        ] {
            reads_back(&scalar(s, true), s);
            reads_back(&regex(s), s);
        }
    }

    #[test]
    fn hook_layout() {
        let h = OutHook {
            comments: vec!["snp-migrate: warning: x".to_owned()],
            id: "eslint".to_owned(),
            name: "eslint".to_owned(),
            language: "system".to_owned(),
            entry: "yarn eslint --fix".to_owned(),
            files: Some("^(?i:(?:.*\\.(?:js|ts)))$".to_owned()),
            pass_filenames: true,
            stages: vec!["pre-commit".to_owned()],
            require_serial: true,
            priority: Some(0),
            stage_fixed: true,
            env: vec![("A".to_owned(), "--x".to_owned())],
            skip: Some(CondOut::Items(vec![
                CondItem::State("merge".to_owned()),
                CondItem::Map(vec![("ref".to_owned(), "main".to_owned())]),
            ])),
            ..OutHook::default()
        };
        let mut s = String::new();
        write_hook(&mut s, &h, 6);
        assert_eq!(
            s,
            "      # snp-migrate: warning: x\n      - id: eslint\n        name: eslint\n        language: system\n        entry: yarn eslint --fix\n        files: '^(?i:(?:.*\\.(?:js|ts)))$'\n        stages: [pre-commit]\n        require_serial: true\n        priority: 0\n        stage_fixed: true\n        env: {A: --x}\n        skip: [merge, {ref: main}]\n"
        );
    }
}
