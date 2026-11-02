//! POSIX shell quoting and lefthook template scanning for the `run` → `entry` rules (05 §7.3, §19.5).
//!
//! Owner: WP-34.

/// Python's `shlex.quote` safe set: unchanged when non-empty and made of `[A-Za-z0-9_@%+=:,./-]`, else
/// single-quoted ([`sh_single`]).
pub(crate) fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        s.to_owned()
    } else {
        sh_single(s)
    }
}

/// `'<s>'` with every `'` written as `'\''` (pre-commit's `shlex.split` reads it back as `s`).
pub(crate) fn sh_single(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// `W(script)` of 05 §19.5: `sh -c '<script>' --`. The filenames pre-commit appends become `"$@"` (`$0` = `--`).
pub(crate) fn wrapper(script: &str) -> String {
    format!("sh -c {} --", sh_single(script))
}

/// `"<value>"` for an environment assignment that reproduces lefthook's `os.ExpandEnv` of the value while
/// command substitution and escapes stay literal. Go's `os.Expand` grammar (`getShellName`): `$NAME` / `${NAME}`
/// with a shell variable name become `${NAME}`; a special one-character name (`*#$@!?-` or a digit, braced or
/// not) and any other `${…}` look up an environment variable `sh` cannot name and become empty; `${}` and an
/// unclosed `${` are dropped; `$` before anything else (or at the end) stays a literal `$`.
pub(crate) fn dq_expand(v: &str) -> String {
    fn lit(out: &mut String, c: char) {
        if matches!(c, '\\' | '"' | '`' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    fn special(c: char) -> bool {
        "*#$@!?-".contains(c) || c.is_ascii_digit()
    }
    fn alnum(c: char) -> bool {
        c == '_' || c.is_ascii_alphanumeric()
    }
    let mut out = String::from("\"");
    let chars: Vec<char> = v.chars().collect();
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        i = i.saturating_add(1);
        if c != '$' {
            lit(&mut out, c);
            continue;
        }
        let rest = chars.get(i..).unwrap_or(&[]);
        let (name, width): (Option<String>, usize) = match rest {
            [] => (None, 0),
            ['{', n, '}', ..] if special(*n) => (Some(n.to_string()), 3),
            ['{', tail @ ..] => match tail.iter().position(|&c| c == '}') {
                Some(0) => (Some(String::new()), 2),
                Some(k) => (
                    Some(tail.get(..k).unwrap_or(&[]).iter().collect()),
                    k.saturating_add(2),
                ),
                None => (Some(String::new()), 1),
            },
            [n, ..] if special(*n) => (Some(n.to_string()), 1),
            _ => {
                let k = rest.iter().take_while(|&&c| alnum(c)).count();
                if k == 0 {
                    (None, 0)
                } else {
                    (Some(rest.get(..k).unwrap_or(&[]).iter().collect()), k)
                }
            }
        };
        i = i.saturating_add(width);
        match name {
            None => lit(&mut out, '$'),
            Some(n) if is_var_name(&n) => {
                out.push_str("${");
                out.push_str(&n);
                out.push('}');
            }
            // Bad syntax (eaten) or a name no shell variable can have: Go's `Getenv` gives "".
            Some(_) => {}
        }
    }
    out.push('"');
    out
}

/// A shell variable name (`[A-Za-z_][A-Za-z0-9_]*`).
pub(crate) fn is_var_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Shell reserved words and builtins that have no standalone executable (or whose executable cannot do what the
/// builtin does in lefthook's `sh -c`): pre-commit would fail with "Executable `exit` not found".
const SHELL_ONLY: [&str; 46] = [
    "!", ":", ".", "break", "continue", "eval", "exec", "exit", "export", "readonly", "return",
    "set", "shift", "times", "trap", "unset", "cd", "umask", "ulimit", "wait", "read", "alias",
    "unalias", "command", "getopts", "hash", "type", "source", "local", "if", "then", "else",
    "elif", "fi", "case", "esac", "for", "while", "until", "do", "done", "{", "}", "jobs", "fg",
    "bg",
];

/// 05 §19.5 "simple": none of ``| & ; < > ( ) $ ` \ " ' * ? [ ] # ~ { }``, no newline, no leading `NAME=`
/// assignment, a first word that is not a shell-only builtin or reserved word ([`SHELL_ONLY`]), and something left
/// to run. pre-commit's `shlex.split` of a simple command equals `sh`'s parse, and the program it execs is the
/// one `sh` would run.
pub(crate) fn is_simple(s: &str) -> bool {
    if s.trim().is_empty() || s.contains(|c: char| "|&;<>()$`\\\"'*?[]#~{}\n\r".contains(c)) {
        return false;
    }
    let first = s.split_whitespace().next().unwrap_or("");
    if SHELL_ONLY.contains(&first) {
        return false;
    }
    match first.split_once('=') {
        Some((name, _)) => !is_var_name(name),
        None => true,
    }
}

/// One lefthook template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tpl {
    /// `{staged_files}`.
    Staged,
    /// `{push_files}`.
    Push,
    /// `{all_files}`.
    All,
    /// `{files}`.
    Files,
    /// `{0}`, `{1}`, …
    Arg(u32),
    /// `{lefthook_job_name}`.
    JobName,
    /// Any other `{name}` (custom templates are inlined before scanning; the rest stays verbatim).
    Other(String),
}

impl Tpl {
    /// A file-list template.
    pub(crate) const fn is_file_list(&self) -> bool {
        matches!(self, Self::Staged | Self::Push | Self::All | Self::Files)
    }

    /// The template's text.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Staged => "{staged_files}".to_owned(),
            Self::Push => "{push_files}".to_owned(),
            Self::All => "{all_files}".to_owned(),
            Self::Files => "{files}".to_owned(),
            Self::Arg(n) => format!("{{{n}}}"),
            Self::JobName => "{lefthook_job_name}".to_owned(),
            Self::Other(n) => format!("{{{n}}}"),
        }
    }
}

/// One template occurrence: byte span (including surrounding quotes of a quoted file template), the template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Occ {
    /// Start byte.
    pub(crate) start: usize,
    /// End byte (exclusive).
    pub(crate) end: usize,
    /// The template.
    pub(crate) tpl: Tpl,
}

/// Every `{name}` occurrence of `s` (`name` = ASCII letters, digits, `_`). A file template written as `"{t}"` or
/// `'{t}'` covers its quotes (lefthook's quote-aware replacement, `replacer.go:267-294`).
pub(crate) fn scan(text: &str) -> Vec<Occ> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(&c) = bytes.get(i) {
        if c != b'{' {
            i = i.saturating_add(1);
            continue;
        }
        let mut close = i.saturating_add(1);
        while bytes
            .get(close)
            .is_some_and(|x| x.is_ascii_alphanumeric() || *x == b'_')
        {
            close = close.saturating_add(1);
        }
        if close == i.saturating_add(1) || bytes.get(close) != Some(&b'}') {
            i = i.saturating_add(1);
            continue;
        }
        let name = text.get(i.saturating_add(1)..close).unwrap_or("");
        let tpl = match name {
            "staged_files" => Tpl::Staged,
            "push_files" => Tpl::Push,
            "all_files" => Tpl::All,
            "files" => Tpl::Files,
            "lefthook_job_name" => Tpl::JobName,
            num if num.bytes().all(|d| d.is_ascii_digit()) => num
                .parse::<u32>()
                .map_or_else(|_| Tpl::Other(num.to_owned()), Tpl::Arg),
            other => Tpl::Other(other.to_owned()),
        };
        let mut start = i;
        let mut end = close.saturating_add(1);
        if tpl.is_file_list()
            && let (Some(open_q), Some(close_q)) =
                (i.checked_sub(1).and_then(|k| bytes.get(k)), bytes.get(end))
            && (*open_q == b'"' || *open_q == b'\'')
            && open_q == close_q
        {
            start = start.saturating_sub(1);
            end = end.saturating_add(1);
        }
        out.push(Occ { start, end, tpl });
        i = end;
    }
    out
}

/// `s` with each occurrence replaced by `f(occ)` (occurrences from [`scan`] of the same string).
pub(crate) fn replace(s: &str, occs: &[Occ], mut f: impl FnMut(&Occ) -> String) -> String {
    let mut out = String::new();
    let mut at = 0_usize;
    for o in occs {
        out.push_str(s.get(at..o.start).unwrap_or(""));
        out.push_str(&f(o));
        at = o.end;
    }
    out.push_str(s.get(at..).unwrap_or(""));
    out
}

/// [`replace`] for text that becomes shell code: when the occurrence follows an unescaped backslash, that backslash
/// is dropped. lefthook substitutes first, so its backslash escapes the first character of the substituted value
/// (a no-op for an ordinary character); kept in front of `"$@"` it would escape the quote and break the script.
pub(crate) fn replace_sh(s: &str, occs: &[Occ], mut f: impl FnMut(&Occ) -> String) -> String {
    let mut out = String::new();
    let mut at = 0_usize;
    for o in occs {
        let before = s.get(at..o.start).unwrap_or("");
        let original = s.get(o.start..o.end).unwrap_or("");
        let value = f(o);
        let backslashes = before.bytes().rev().take_while(|b| *b == b'\\').count();
        if backslashes % 2 == 1 && value != original {
            out.push_str(before.get(..before.len().saturating_sub(1)).unwrap_or(""));
        } else {
            out.push_str(before);
        }
        out.push_str(&value);
        at = o.end;
    }
    out.push_str(s.get(at..).unwrap_or(""));
    out
}

/// Raw substitution of custom `templates` (`{name}` → value, unescaped like lefthook ≥ 2.0.8) and of
/// `{lefthook_job_name}` (shell-quoted). Built-in templates are kept.
pub(crate) fn inline_templates(s: &str, templates: &[(String, String)], job_name: &str) -> String {
    let occs = scan(s);
    replace(s, &occs, |o| match &o.tpl {
        Tpl::JobName => sh_quote(job_name),
        Tpl::Other(n) => templates
            .iter()
            .find(|(k, _)| k == n)
            .map_or_else(|| o.tpl.text(), |(_, v)| v.clone()),
        _ => s.get(o.start..o.end).unwrap_or("").to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("abc/d.e-f"), "abc/d.e-f");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert_eq!(
            wrapper("echo 'x' \"$@\""),
            "sh -c 'echo '\\''x'\\'' \"$@\"' --"
        );
        assert_eq!(dq_expand("$HOME/bin:${PATH}"), "\"${HOME}/bin:${PATH}\"");
        assert_eq!(
            dq_expand("a\"b`c\\d$(x) $"),
            "\"a\\\"b\\`c\\\\d\\$(x) \\$\""
        );
    }

    /// `tests/vectors/expandenv.jsonl` holds Go's `os.Expand` (the `os.ExpandEnv` grammar) of 561 values under a
    /// fixed environment (`expandenv-oracle`); every [`dq_expand`] assignment, run by `sh` with that environment,
    /// gives the same value.
    #[cfg(unix)]
    #[test]
    fn dq_expand_matches_go_expand_env() {
        use std::fmt::Write as _;
        let mut script = String::new();
        let mut want = Vec::new();
        for line in include_str!("../../tests/vectors/expandenv.jsonl").lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            let input = v["in"].as_str().unwrap();
            want.push((input.to_owned(), v["out"].as_str().unwrap().to_owned()));
            writeln!(script, "V={}; printf '%s\\037' \"$V\"", dq_expand(input)).unwrap();
        }
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&script)
            .env_clear()
            .envs([
                ("A", "x y"),
                ("H", "/home/u"),
                ("AH", "$A`q`"),
                ("x", "\\\""),
                ("A1", "1"),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let got = String::from_utf8(out.stdout).unwrap();
        let got: Vec<&str> = got.split('\u{1f}').collect();
        assert_eq!(got.len(), want.len().saturating_add(1));
        for ((input, w), g) in want.iter().zip(got) {
            assert_eq!(g, w, "{input:?} -> {}", dq_expand(input));
        }
    }

    #[test]
    fn simplicity() {
        assert!(is_simple("yarn eslint --fix"));
        assert!(is_simple("bundle exec rubocop -A --force-exclusion --"));
        assert!(!is_simple("a | b"));
        assert!(!is_simple("echo $X"));
        assert!(!is_simple("FOO=1 make"));
        assert!(is_simple("make FOO=1"));
        assert!(!is_simple("a\nb"));
        assert!(!is_simple("   "));
        assert!(!is_simple("echo {x}"));
    }

    #[test]
    fn scanning() {
        let s = "a {staged_files} \"{files}\" '{all_files}' {1} {x} {} {a b}";
        let tpls: Vec<Tpl> = scan(s).into_iter().map(|o| o.tpl).collect();
        assert_eq!(
            tpls,
            [
                Tpl::Staged,
                Tpl::Files,
                Tpl::All,
                Tpl::Arg(1),
                Tpl::Other("x".to_owned())
            ]
        );
        let occs = scan(s);
        assert_eq!(s.get(occs[1].start..occs[1].end), Some("\"{files}\""));
        assert_eq!(
            replace(s, &occs, |o| if o.tpl.is_file_list() {
                "F".to_owned()
            } else {
                "N".to_owned()
            }),
            "a F F F N N {} {a b}"
        );
        assert_eq!(
            inline_templates(
                "{wrap} {lefthook_job_name} {files} {nope}",
                &[("wrap".to_owned(), "docker run".to_owned())],
                "my job"
            ),
            "docker run 'my job' {files} {nope}"
        );
    }
}
