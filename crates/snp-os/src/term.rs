//! Terminal facts and pre-commit's colour decision (`color.py`).
//!
//! Owner: WP-15.
#![expect(clippy::todo, reason = "W0 stub, removed by WP-15")]
#![expect(unused_variables, reason = "W0 stub parameters, removed by WP-15")]

use snp_core::env::Env;
use snp_core::output::ColorSetting;

/// pre-commit's `use_color()`: Always/Never as given; Auto = stderr is a terminal ∧ TERM != "dumb" (Unix);
/// Windows: `winapi_util::console::Console::stderr()?.set_virtual_terminal_processing(true).is_ok()` (color.py parity).
/// `NO_COLOR` / `FORCE_COLOR` are ignored (parity, ADR-029).
pub fn decide_color(s: ColorSetting, env: &Env) -> bool {
    todo!()
}

/// Width of the stdout terminal, if stdout is one.
pub fn stdout_tty_columns() -> Option<usize> {
    todo!()
}

/// `sys.stdout.isatty()`.
pub fn stdout_is_tty() -> bool {
    todo!()
}

/// Lossy-UTF-8 hook output only for a real console (07 §7.5).
pub fn stdout_is_windows_console() -> bool {
    todo!()
}

/// Windows: re-run `SetConsoleMode(ENABLE_VIRTUAL_TERMINAL_PROCESSING)` on the stderr console when the colour
/// decision was `true` (children with inherited stdio can reset it, 14 P-52); no decision change, no byte change.
/// Elsewhere a no-op.
pub fn ensure_vt() {
    todo!()
}
