//! Every user-visible text snp-proc produces (ARCHITECTURE §1.3 "Messages").
//!
//! None of them names the tool except the restore notice, which is snp-only behaviour and reads the same in both
//! brands (`signals::install` has no brand); the `Executable …` texts are pre-commit's (`parse_shebang.py:46-62`)
//! and identical in both brands.
//!
//! Owner: WP-15.

use std::ffi::OsStr;

/// The one stderr line printed when signals arrive while a `RestoreShield` is alive (ARCHITECTURE §10.4).
pub(crate) const RESTORE_NOTICE: &str = "snp: restoring stashed changes, please wait\n";

/// `ExecutableNotFoundError` texts (`parse_shebang.py:46-62`): "Executable `{orig}` {what}" (no newline).
pub(crate) fn executable(orig: &OsStr, what: &str) -> String {
    format!("Executable `{}` {what}", orig.to_string_lossy())
}

/// D-PLAT-03: a `.bat`/`.cmd` program cannot receive an argument containing CR or LF (BatBadBut-safe quoting).
#[cfg_attr(
    not(any(test, windows)),
    expect(dead_code, reason = "Windows batch files only; unit-tested everywhere")
)]
pub(crate) fn batch_line_break(cmd0: &OsStr, arg: &OsStr) -> String {
    format!(
        "Executable `{}` cannot receive an argument containing a line break: {}",
        cmd0.to_string_lossy(),
        snp_pycompat::repr::repr_os(arg)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden texts (identical in the compat and the snp brand).
    #[test]
    fn goldens() {
        assert_eq!(
            RESTORE_NOTICE.as_bytes(),
            b"snp: restoring stashed changes, please wait\n"
        );
        assert_eq!(
            executable(OsStr::new("dne"), "not found"),
            "Executable `dne` not found"
        );
        assert_eq!(
            executable(OsStr::new("./x"), "is a directory"),
            "Executable `./x` is a directory"
        );
        assert_eq!(
            executable(OsStr::new("./x"), "is not executable"),
            "Executable `./x` is not executable"
        );
        assert_eq!(
            batch_line_break(OsStr::new("t.bat"), OsStr::new("a\r\nb")),
            "Executable `t.bat` cannot receive an argument containing a line break: 'a\\r\\nb'"
        );
    }
}
