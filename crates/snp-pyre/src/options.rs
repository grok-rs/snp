//! Compile options: str vs bytes patterns, haystack shape (`$` semantics) and pygrep flags.
//!
//! Owner: WP-13.

/// Pattern/haystack type.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Mode {
    /// `str` patterns: `files`/`exclude`/`--pattern`.
    Str,
    /// `bytes` patterns: pygrep.
    Bytes,
}

/// What a haystack is; only matters for `$` (§A.6.5).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Haystack {
    /// A repository path (never contains `\n` in practice).
    Path,
    /// One line of a file (pygrep line mode).
    Line,
    /// A whole file (pygrep `--multiline`).
    Blob,
}

/// Everything that changes how a pattern compiles; part of the [`crate::RegexCache`] key.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    /// str or bytes.
    pub mode: Mode,
    /// Haystack shape.
    pub haystack: Haystack,
    /// pygrep `-i` (`re.IGNORECASE`).
    pub ignore_case: bool,
    /// pygrep `--multiline` (`re.MULTILINE | re.DOTALL`).
    pub multiline_dotall: bool,
}

impl Options {
    /// Config patterns (`files`, `exclude`): str, path haystacks, no flags.
    pub const CONFIG: Self = Self {
        mode: Mode::Str,
        haystack: Haystack::Path,
        ignore_case: false,
        multiline_dotall: false,
    };
}
