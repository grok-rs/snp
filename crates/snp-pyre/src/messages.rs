//! Every user-visible text of the crate (ARCHITECTURE §1.3 "Messages"). The `re.error` texts are CPython
//! 3.12's (`re/_parser.py`, `re/_compiler.py`, `re/_constants.py`) and never name the tool, so they take no
//! `Brand`; the `Unsupported` texts describe snp's own limits (01 Addendum §A.6.13 D3/D5/D6).
//!
//! Owner: WP-13.

use std::fmt::Write as _;

// --- re.error: tokenizer and sequence (`re/_parser.py:230-445`, `512-891`) ---------------------------------

/// `re/_parser.py:252`.
pub(crate) const BAD_ESCAPE_END: &str = "bad escape (end of pattern)";
/// `re/_parser.py:568`.
pub(crate) const UNTERMINATED_SET: &str = "unterminated character set";
/// `re/_parser.py:677`.
pub(crate) const MIN_GT_MAX: &str = "min repeat greater than max repeat";
/// `re/_parser.py:687`.
pub(crate) const NOTHING_TO_REPEAT: &str = "nothing to repeat";
/// `re/_parser.py:690`.
pub(crate) const MULTIPLE_REPEAT: &str = "multiple repeat";
/// `re/_parser.py:720`.
pub(crate) const UNEXPECTED_END: &str = "unexpected end of pattern";
/// `re/_parser.py:755`.
pub(crate) const UNTERMINATED_COMMENT: &str = "missing ), unterminated comment";
/// `re/_parser.py:780`.
pub(crate) const UNTERMINATED_SUBPATTERN: &str = "missing ), unterminated subpattern";
/// `re/_parser.py:800`.
pub(crate) const BAD_GROUP_NUMBER: &str = "bad group number";
/// `re/_parser.py:822`.
pub(crate) const COND_TOO_MANY_BRANCHES: &str = "conditional backref with more than two branches";
/// `re/_parser.py:840`.
pub(crate) const GLOBAL_FLAGS_NOT_AT_START: &str =
    "global flags not at the start of the expression";
/// `re/_parser.py:984`.
pub(crate) const UNBALANCED_PARENTHESIS: &str = "unbalanced parenthesis";
/// `re/_parser.py:106`, `436`, `736`.
pub(crate) const OPEN_GROUP: &str = "cannot refer to an open group";
/// `re/_parser.py:108`.
pub(crate) const SAME_LOOKBEHIND: &str =
    "cannot refer to group defined in the same lookbehind subpattern";
/// `re/_parser.py:406` (`\N` without `{`).
pub(crate) const MISSING_LBRACE: &str = "missing {";

// --- re.error: inline flags (`re/_parser.py:893-951`) -------------------------------------------------------

/// `re/_parser.py:902`.
pub(crate) const FLAG_L_WITH_STR: &str = "bad inline flags: cannot use 'L' flag with a str pattern";
/// `re/_parser.py:906`.
pub(crate) const FLAG_U_WITH_BYTES: &str =
    "bad inline flags: cannot use 'u' flag with a bytes pattern";
/// `re/_parser.py:910`.
pub(crate) const FLAGS_INCOMPATIBLE: &str =
    "bad inline flags: flags 'a', 'u' and 'L' are incompatible";
/// `re/_parser.py:914`, `918`.
pub(crate) const MISSING_DASH_COLON_PAREN: &str = "missing -, : or )";
/// `re/_parser.py:918`, `930`, `944`.
pub(crate) const UNKNOWN_FLAG: &str = "unknown flag";
/// `re/_parser.py:924`.
pub(crate) const TURN_ON_GLOBAL: &str = "bad inline flags: cannot turn on global flag";
/// `re/_parser.py:928`, `930`.
pub(crate) const MISSING_FLAG: &str = "missing flag";
/// `re/_parser.py:935`.
pub(crate) const TURN_OFF_TYPE: &str = "bad inline flags: cannot turn off flags 'a', 'u' and 'L'";
/// `re/_parser.py:940`, `944`.
pub(crate) const MISSING_COLON: &str = "missing :";
/// `re/_parser.py:948`.
pub(crate) const TURN_OFF_GLOBAL: &str = "bad inline flags: cannot turn off global flag";
/// `re/_parser.py:950`.
pub(crate) const FLAG_ON_AND_OFF: &str = "bad inline flags: flag turned on and off";

// --- compile phase (`re/_compiler.py`; no position) ---------------------------------------------------------

/// `re/_compiler.py:186`.
pub(crate) const LOOKS_TOO_MUCH_BEHIND: &str = "looks too much behind";
/// `re/_compiler.py:188`.
pub(crate) const LOOKBEHIND_FIXED_WIDTH: &str = "look-behind requires fixed-width pattern";

// --- other exception classes (`re/_parser.py:671`, `953-967`; the interpreter's recursion limit) ------------

/// `OverflowError` (`re/_parser.py:671`, `675`).
pub(crate) const OVERFLOW: &str = "the repetition number is too large";
/// `RecursionError` text as the last traceback line prints it.
pub(crate) const RECURSION: &str = "maximum recursion depth exceeded";
/// `ValueError` (`re/_parser.py:961`).
pub(crate) const ASCII_AND_UNICODE: &str = "ASCII and UNICODE flags are incompatible";
/// `ValueError` (`re/_parser.py:966`).
pub(crate) const ASCII_AND_LOCALE: &str = "ASCII and LOCALE flags are incompatible";
/// `ValueError` (`re/_parser.py:957`; unreachable through the inline-flag grammar, kept for completeness).
pub(crate) const LOCALE_WITH_STR: &str = "cannot use LOCALE flag with a str pattern";
/// `ValueError` (`re/_parser.py:964`; unreachable through the inline-flag grammar, kept for completeness).
pub(crate) const UNICODE_WITH_BYTES: &str = "cannot use UNICODE flag with a bytes pattern";

// --- snp limits (Unsupported; 01 Addendum §A.6.13) ----------------------------------------------------------

/// D3: `\N{…}` with the `py-re-names` feature disabled (01 Addendum §A.5.4).
#[cfg(not(feature = "py-re-names"))]
pub(crate) const NAMES_DISABLED: &str = "\\N{...} escapes are not supported in this build";
/// D6: the backtracking engine nests at most 63 groups (fancy-regex `MAX_RECURSION`).
pub(crate) const TOO_DEEP_FOR_TIER2: &str = "the pattern nests too deeply for the backtracking engine (look-around, back-references, conditionals, \
     atomic groups, possessive repeats, or an exact `$`/`\\b` fallback need fewer than 63 nested groups)";
/// D5: the expanded pattern exceeds the engine budget.
pub(crate) const OVER_BUDGET: &str =
    "the pattern's repeat counts expand beyond the regular expression engines' size budget";
/// No helper thread for compiling a deeply nested pattern.
pub(crate) const NO_BUILD_THREAD: &str =
    "could not start a thread to compile a deeply nested pattern";

/// `bad escape %s` (`re/_parser.py:366`, `443`, `447`).
pub(crate) fn bad_escape(escape: &str) -> String {
    format!("bad escape {escape}")
}

/// `incomplete escape %s` (`re/_parser.py:326`, `386`).
pub(crate) fn incomplete_escape(escape: &str) -> String {
    format!("incomplete escape {escape}")
}

/// `octal escape value %s outside of range 0-0o377` (`re/_parser.py:359`, `428`).
pub(crate) fn octal_out_of_range(escape: &str) -> String {
    format!("octal escape value {escape} outside of range 0-0o377")
}

/// `invalid group reference %d` (`re/_parser.py:440`, `803`, `988`).
pub(crate) fn invalid_group_reference(group: &str) -> String {
    format!("invalid group reference {group}")
}

/// `bad character range %s-%s` (`re/_parser.py:611`, `616`).
pub(crate) fn bad_range(this: &str, that: &str) -> String {
    format!("bad character range {this}-{that}")
}

/// `missing <name>` (`re/_parser.py:281`, `286`): "missing group name" / "missing character name".
pub(crate) fn missing(name: &str) -> String {
    format!("missing {name}")
}

/// `missing %s, unterminated name` (`re/_parser.py:282`).
pub(crate) fn unterminated_name(terminator: char) -> String {
    format!("missing {terminator}, unterminated name")
}

/// `bad character in group name %r` / `%a` (`re/_parser.py:306`, `309`); `quoted` is the Python repr.
pub(crate) fn bad_group_name(quoted: &str) -> String {
    format!("bad character in group name {quoted}")
}

/// `unknown group name %r` (`re/_parser.py:733`, `795`).
pub(crate) fn unknown_group_name(quoted: &str) -> String {
    format!("unknown group name {quoted}")
}

/// `redefinition of group name %r as group %d; was group %d` (`re/_parser.py:94`).
pub(crate) fn redefinition(quoted: &str, gid: u32, ogid: u32) -> String {
    format!("redefinition of group name {quoted} as group {gid}; was group {ogid}")
}

/// `undefined character name %r` (`re/_parser.py:351`, `411`).
pub(crate) fn undefined_name(quoted: &str) -> String {
    format!("undefined character name {quoted}")
}

/// `unknown extension ?…` (`re/_parser.py:746`, `769`, `849`); `rest` is everything after `?`.
pub(crate) fn unknown_extension(rest: &str) -> String {
    format!("unknown extension ?{rest}")
}

/// `Possible nested set at position %d` (`re/_parser.py:560`).
pub(crate) fn nested_set(pos: usize) -> String {
    format!("Possible nested set at position {pos}")
}

/// `Possible set difference|intersection|symmetric difference|union at position %d` (`re/_parser.py:578`, `605`).
pub(crate) fn set_operation(kind: &str, pos: usize) -> String {
    format!("Possible set {kind} at position {pos}")
}

/// `internal: unsupported template operator %r` (`re/_compiler.py:105`).
pub(crate) fn template_operator(opcode: &str) -> String {
    format!("internal: unsupported template operator {opcode}")
}
/// D5: the tier-1 engine refused the translated pattern (a size limit).
pub(crate) fn engine_limit(detail: &str) -> String {
    format!("the pattern is too large for the regular expression engines ({detail})")
}

/// `msg.encode('ascii', 'backslashreplace').decode('ascii')` (`re/_parser.py:301`): how bytes patterns render
/// their messages.
pub(crate) fn backslashreplace(msg: &str) -> String {
    let mut out = String::new();
    for c in msg.chars() {
        let v = u32::from(c);
        if c.is_ascii() {
            out.push(c);
        } else if v <= 0xFF {
            write!(out, "\\x{v:02x}").unwrap_or_default();
        } else if v <= 0xFFFF {
            write!(out, "\\u{v:04x}").unwrap_or_default();
        } else {
            write!(out, "\\U{v:08x}").unwrap_or_default();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden texts (01 Addendum §A.4, §A.6.2): the exact CPython 3.12 strings.
    #[test]
    fn goldens() {
        assert_eq!(bad_escape("\\z"), "bad escape \\z");
        assert_eq!(incomplete_escape("\\x4"), "incomplete escape \\x4");
        assert_eq!(
            octal_out_of_range("\\477"),
            "octal escape value \\477 outside of range 0-0o377"
        );
        assert_eq!(invalid_group_reference("2"), "invalid group reference 2");
        assert_eq!(bad_range("\\w", "."), "bad character range \\w-.");
        assert_eq!(missing("group name"), "missing group name");
        assert_eq!(unterminated_name('>'), "missing >, unterminated name");
        assert_eq!(bad_group_name("'1a'"), "bad character in group name '1a'");
        assert_eq!(unknown_group_name("'a'"), "unknown group name 'a'");
        assert_eq!(
            redefinition("'a'", 2, 1),
            "redefinition of group name 'a' as group 2; was group 1"
        );
        assert_eq!(undefined_name("'NOPE'"), "undefined character name 'NOPE'");
        assert_eq!(unknown_extension("<a"), "unknown extension ?<a");
        assert_eq!(nested_set(1), "Possible nested set at position 1");
        assert_eq!(
            set_operation("difference", 2),
            "Possible set difference at position 2"
        );
        assert_eq!(
            template_operator("MAX_REPEAT"),
            "internal: unsupported template operator MAX_REPEAT"
        );
        assert_eq!(
            backslashreplace("name '\u{e9}\u{3b1}\u{1f600}'"),
            "name '\\xe9\\u03b1\\U0001f600'"
        );
    }
}
