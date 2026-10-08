//! Terminal facts and pre-commit's colour decision (`color.py`).
//!
//! Owner: WP-15.

use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};

use snp_core::env::Env;
use snp_core::output::ColorSetting;

/// Set once a colour decision returned `true` (read by [`ensure_vt`]).
static COLOR_DECIDED: AtomicBool = AtomicBool::new(false);

/// pre-commit's `use_color()`: Always/Never as given; Auto = stderr is a terminal ∧ TERM != "dumb" (Unix);
/// Windows: `winapi_util::console::Console::stderr()?.set_virtual_terminal_processing(true).is_ok()` (color.py parity).
/// `NO_COLOR` / `FORCE_COLOR` are ignored (parity, ADR-029).
///
/// `color.py:93-110` with `terminal_supports_color` of `color.py:7-57`: on Windows the VT mode of the **stderr**
/// console is enabled once, on first use, whatever the setting (pre-commit does it at import), and `auto` requires
/// that to have succeeded (a console; `NUL` and pipes fail, MSYS ptys fail as in Python, 14 P-50/P-51). `TERM` is
/// read from `env` (`os.getenv('TERM') != 'dumb'`: an unset `TERM` counts as not dumb).
pub fn decide_color(s: ColorSetting, env: &Env) -> bool {
    let supports = terminal_supports_color();
    let decision = match s {
        ColorSetting::Always => true,
        ColorSetting::Never => false,
        ColorSetting::Auto => {
            stderr_isatty() && supports && env.get("TERM").is_none_or(|t| t != OsStr::new("dumb"))
        }
    };
    if decision {
        COLOR_DECIDED.store(true, Ordering::Relaxed);
    }
    decision
}

/// `sys.stderr.isatty()`: Unix `isatty(2)`; Windows CRT `_isatty(2)` = a character device (console or `NUL`).
fn stderr_isatty() -> bool {
    #[cfg(windows)]
    {
        winapi_util::file::typ(winapi_util::HandleRef::stderr()).is_ok_and(|t| t.is_char())
    }
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal;
        std::io::stderr().is_terminal()
    }
}

/// `color.terminal_supports_color`: `True` on POSIX; on Windows whether enabling VT processing on the stderr
/// console succeeded, computed once (the import-time side effect of `color.py`).
fn terminal_supports_color() -> bool {
    #[cfg(windows)]
    {
        static SUPPORTS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *SUPPORTS.get_or_init(enable_vt_on_stderr)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// `GetConsoleMode` + `SetConsoleMode(mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING)` on the stderr handle.
#[cfg(windows)]
fn enable_vt_on_stderr() -> bool {
    winapi_util::console::Console::stderr()
        .and_then(|mut c| c.set_virtual_terminal_processing(true))
        .is_ok()
}

/// Width of the stdout terminal, if stdout is one.
///
/// `os.get_terminal_size(sys.__stdout__.fileno()).columns` as `shutil.get_terminal_size` uses it: `None` when
/// stdout is not a terminal or reports 0 columns (`size.columns or fallback[0]`), so the caller falls back to 80.
/// `COLUMNS` is the caller's (`snp_argparse::columns`). Windows: the console window width
/// (`srWindow.Right - srWindow.Left + 1`).
pub fn stdout_tty_columns() -> Option<usize> {
    #[cfg(unix)]
    {
        let ws = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
        Some(usize::from(ws.ws_col)).filter(|c| *c > 0)
    }
    #[cfg(windows)]
    {
        let info =
            winapi_util::console::screen_buffer_info(winapi_util::HandleRef::stdout()).ok()?;
        let r = info.window_rect();
        let cols = i32::from(r.right)
            .checked_sub(i32::from(r.left))?
            .checked_add(1)?;
        usize::try_from(cols).ok().filter(|c| *c > 0)
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// `sys.stdout.isatty()`.
///
/// Unix `isatty(1)`; Windows CRT `_isatty(1)` (a character device: a console, but also `NUL`).
pub fn stdout_is_tty() -> bool {
    #[cfg(windows)]
    {
        winapi_util::file::typ(winapi_util::HandleRef::stdout()).is_ok_and(|t| t.is_char())
    }
    #[cfg(not(windows))]
    {
        use std::io::IsTerminal;
        std::io::stdout().is_terminal()
    }
}

/// Lossy-UTF-8 hook output only for a real console (07 §7.5).
///
/// CPython uses `_WindowsConsoleIO` (which decodes UTF-8 lossily, 14 P-53) exactly when stdout is a console
/// (`GetConsoleMode` succeeds). Always `false` elsewhere.
pub fn stdout_is_windows_console() -> bool {
    #[cfg(windows)]
    {
        winapi_util::console::mode(winapi_util::HandleRef::stdout()).is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Windows: re-run `SetConsoleMode(ENABLE_VIRTUAL_TERMINAL_PROCESSING)` on the stderr console when the colour
/// decision was `true` (children with inherited stdio can reset it, 14 P-52); no decision change, no byte change.
/// Elsewhere a no-op.
///
/// ADR-074; prek re-enables VT before each flush for the same reason (06 §5.6). Errors are ignored.
pub fn ensure_vt() {
    #[cfg(windows)]
    if COLOR_DECIDED.load(Ordering::Relaxed) {
        let _ignored = enable_vt_on_stderr();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_and_never_are_unconditional() {
        let dumb = Env::from_pairs([("TERM", "dumb")]);
        assert!(decide_color(ColorSetting::Always, &dumb));
        assert!(!decide_color(
            ColorSetting::Never,
            &Env::from_pairs::<&str, &str>([])
        ));
        // NO_COLOR / FORCE_COLOR are ignored (ADR-029).
        let env = Env::from_pairs([("NO_COLOR", "1"), ("FORCE_COLOR", "1")]);
        assert!(decide_color(ColorSetting::Always, &env));
        assert!(!decide_color(ColorSetting::Never, &env));
        ensure_vt();
    }

    #[test]
    fn auto_follows_stderr_and_term() {
        let env = Env::from_pairs([("TERM", "xterm")]);
        assert_eq!(
            decide_color(ColorSetting::Auto, &env),
            stderr_isatty() && terminal_supports_color()
        );
        // TERM=dumb never colours in auto mode.
        assert!(!decide_color(
            ColorSetting::Auto,
            &Env::from_pairs([("TERM", "dumb")])
        ));
    }

    #[test]
    fn stdout_facts_are_consistent() {
        if stdout_tty_columns().is_some() {
            assert!(stdout_is_tty());
        }
        if !cfg!(windows) {
            assert!(!stdout_is_windows_console());
        }
    }
}
