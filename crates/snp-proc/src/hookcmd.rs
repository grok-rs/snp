//! `lang_base.hook_cmd` with the hazmat rewrite (02 §11.3, 03 H.8.1).
//!
//! Owner: WP-15.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use snp_pycompat::shlex::{self, ShlexError};

/// Why an `entry` cannot become an argv.
#[derive(Debug)]
#[non_exhaustive]
pub enum HookCmdError {
    /// `shlex.split` raised.
    Shlex(ShlexError),
    /// The entry splits to nothing.
    Empty,
}

impl std::fmt::Display for HookCmdError {
    /// `str(exc)`: `No closing quotation` / `No escaped character`; `tuple index out of range` for an empty command
    /// (pre-commit's `IndexError` in `normexe`).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shlex(e) => std::fmt::Display::fmt(e, f),
            Self::Empty => f.write_str("tuple index out of range"),
        }
    }
}

impl std::error::Error for HookCmdError {}

/// `shlex.split(entry)`; "pre-commit hazmat …"/"snp hazmat …" (first two tokens, any quoting) → [`self_exe`, "__hazmat",
/// …]; then args (02 §11.3, 03 H.8.1). Other `pre-commit …` entries are not rewritten (parity).
///
/// `lang_base.hook_cmd` (`lang_base.py:174-178`): `(*cmd, *args)` — the args are appended verbatim (no splitting);
/// the matching is exact and case-sensitive (`pre-commit.exe`, paths and later occurrences are not rewritten).
/// [`HookCmdError::Empty`] when the entry and the args together are empty (pre-commit's `IndexError`, exit 3); an
/// empty entry with args runs the args, as pre-commit does.
pub fn hook_cmd(
    entry: &str,
    args: &[Arc<str>],
    self_exe: &Path,
) -> Result<Vec<OsString>, HookCmdError> {
    let tokens = shlex::split(entry).map_err(HookCmdError::Shlex)?;
    let mut out: Vec<OsString> = Vec::new();
    match tokens.as_slice() {
        [tool, hazmat, rest @ ..]
            if hazmat == "hazmat" && (tool == "pre-commit" || tool == "snp") =>
        {
            out.push(self_exe.as_os_str().to_os_string());
            out.push(OsString::from("__hazmat"));
            out.extend(rest.iter().map(OsString::from));
        }
        _ => out.extend(tokens.into_iter().map(OsString::from)),
    }
    out.extend(args.iter().map(|a| OsString::from(&**a)));
    if out.is_empty() {
        return Err(HookCmdError::Empty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(entry: &str, args: &[&str]) -> Result<Vec<String>, String> {
        let args: Vec<Arc<str>> = args.iter().map(|a| Arc::from(*a)).collect();
        hook_cmd(entry, &args, Path::new("/opt/snp/bin/snp"))
            .map(|v| v.into_iter().map(|s| s.into_string().unwrap()).collect())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn plain_entries_and_args() {
        assert_eq!(cmd("echo hi", &[]).unwrap(), ["echo", "hi"]);
        assert_eq!(
            cmd("echo hi", &["--flag", "a b"]).unwrap(),
            ["echo", "hi", "--flag", "a b"]
        );
        // 02 §11.3 shlex table.
        assert_eq!(
            cmd(r#"a\ b "a\$b" 'x\y'"#, &[]).unwrap(),
            ["a b", r"a\$b", r"x\y"]
        );
        assert_eq!(cmd("a\"b c\"d #x", &[]).unwrap(), ["ab cd", "#x"]);
        assert_eq!(cmd("x \\", &[]).unwrap_err(), "No escaped character");
        assert_eq!(cmd("\"abc", &[]).unwrap_err(), "No closing quotation");
    }

    #[test]
    fn hazmat_rewrite() {
        assert_eq!(
            cmd("pre-commit hazmat cd a echo -- b", &[]).unwrap(),
            ["/opt/snp/bin/snp", "__hazmat", "cd", "a", "echo", "--", "b"]
        );
        assert_eq!(
            cmd("'snp' \"hazmat\" n1 x --", &["y"]).unwrap(),
            ["/opt/snp/bin/snp", "__hazmat", "n1", "x", "--", "y"]
        );
        // Not rewritten: other subcommands, other spellings, later occurrences.
        assert_eq!(cmd("pre-commit run", &[]).unwrap(), ["pre-commit", "run"]);
        assert_eq!(
            cmd("pre-commit.exe hazmat x", &[]).unwrap(),
            ["pre-commit.exe", "hazmat", "x"]
        );
        assert_eq!(
            cmd("Pre-Commit hazmat", &[]).unwrap(),
            ["Pre-Commit", "hazmat"]
        );
        assert_eq!(
            cmd("x pre-commit hazmat", &[]).unwrap(),
            ["x", "pre-commit", "hazmat"]
        );
        assert_eq!(
            cmd("pre-commit", &["hazmat"]).unwrap(),
            ["pre-commit", "hazmat"]
        );
    }

    #[test]
    fn empty_command() {
        assert_eq!(cmd("", &[]).unwrap_err(), "tuple index out of range");
        assert_eq!(cmd("   ", &[]).unwrap_err(), "tuple index out of range");
        // (*cmd, *args): an empty entry with args runs the args.
        assert_eq!(cmd("", &["tool", "x"]).unwrap(), ["tool", "x"]);
    }
}
