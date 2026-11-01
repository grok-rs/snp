//! POSIX shell quoting and lefthook template scanning for the `run` → `entry` rules (05 §7.3, §19.5).
//!
//! Owner: WP-34.

/// Python's `shlex.quote` safe set: unchanged when non-empty and made of `[A-Za-z0-9_@%+=:,./-]`, else
/// single-quoted ([`sh_single`]).
pub(crate) fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s
            .chars()
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

/// `"<value>"` for an environment assignment that keeps lefthook's `$VAR` / `${VAR}` expansion (`os.ExpandEnv`)
/// while command substitution and escapes stay literal.
pub(crate) fn dq_expand(v: &str) -> String {
    let mut out = String::from("\"");
    let chars: Vec<char> = v.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\\' | '"' | '`' => {
                out.push('\\');
                out.push(c);
            }
            '$' => {
                let next = chars.get(i.saturating_add(1)).copied();
                let expands = next.is_some_and(|n| n == '{' || n == '_' || n.is_ascii_alphanumeric());
                if !expands {
                    out.push('\\');
                }
                out.push('$');
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A shell variable name (`[A-Za-z_][A-Za-z0-9_]*`).
pub(crate) fn is_var_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// 05 §19.5 "simple": none of ``| & ; < > ( ) $ ` \ " ' * ? [ ] # ~ { }``, no newline, no leading `NAME=`
/// assignment, and something left to run. pre-commit's `shlex.split` of a simple command equals `sh`'s parse.
pub(crate) fn is_simple(s: &str) -> bool {
    if s.trim().is_empty() || s.contains(|c: char| "|&;<>()$`\\\"'*?[]#~{}\n\r".contains(c)) {
        return false;
    }
    let first = s.split_whitespace().next().unwrap_or("");
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
pub(crate) fn scan(s: &str) -> Vec<Occ> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0_usize;
    while let Some(&c) = b.get(i) {
        if c != b'{' {
            i = i.saturating_add(1);
            continue;
        }
        let mut j = i.saturating_add(1);
        while b.get(j).is_some_and(|x| x.is_ascii_alphanumeric() || *x == b'_') {
            j = j.saturating_add(1);
        }
        if j == i.saturating_add(1) || b.get(j) != Some(&b'}') {
            i = i.saturating_add(1);
            continue;
        }
        let name = s.get(i.saturating_add(1)..j).unwrap_or("");
        let tpl = match name {
            "staged_files" => Tpl::Staged,
            "push_files" => Tpl::Push,
            "all_files" => Tpl::All,
            "files" => Tpl::Files,
            "lefthook_job_name" => Tpl::JobName,
            n if n.bytes().all(|d| d.is_ascii_digit()) => match n.parse::<u32>() {
                Ok(v) => Tpl::Arg(v),
                Err(_) => Tpl::Other(n.to_owned()),
            },
            n => Tpl::Other(n.to_owned()),
        };
        let mut start = i;
        let mut end = j.saturating_add(1);
        if tpl.is_file_list()
            && let (Some(q), Some(r)) = (i.checked_sub(1).and_then(|k| b.get(k)), b.get(end))
            && (*q == b'"' || *q == b'\'')
            && q == r
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
        assert_eq!(wrapper("echo 'x' \"$@\""), "sh -c 'echo '\\''x'\\'' \"$@\"' --");
        assert_eq!(dq_expand("$HOME/bin:${PATH}"), "\"$HOME/bin:${PATH}\"");
        assert_eq!(dq_expand("a\"b`c\\d$(x) $"), "\"a\\\"b\\`c\\\\d\\$(x) \\$\"");
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
            [Tpl::Staged, Tpl::Files, Tpl::All, Tpl::Arg(1), Tpl::Other("x".to_owned())]
        );
        let occs = scan(s);
        assert_eq!(&s[occs[1].start..occs[1].end], "\"{files}\"");
        assert_eq!(
            replace(s, &occs, |o| if o.tpl.is_file_list() { "F".to_owned() } else { "N".to_owned() }),
            "a F F F N N {} {a b}"
        );
        assert_eq!(
            inline_templates("{wrap} {lefthook_job_name} {files} {nope}", &[("wrap".to_owned(), "docker run".to_owned())], "my job"),
            "docker run 'my job' {files} {nope}"
        );
    }
}
