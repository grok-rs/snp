//! The terminal an `interactive` hook reads and writes (ADR-085).
//!
//! An interactive hook gets snp's own stdin when it is a terminal, else the controlling terminal opened as
//! `/dev/tty` (git runs hooks with stdin on `/dev/null`); Windows opens the console input `CONIN$` instead. Its
//! stdout and stderr are snp's own (inherited, never captured) and it stays in snp's process group: when that group
//! is the terminal's foreground group (checked here with `tcgetpgrp`) the child can read the terminal and change its
//! modes without SIGTTIN/SIGTTOU, and ^C reaches it exactly once, from the kernel (`registry::forward` never re-sends
//! a tty-generated signal to an Inherit-group child). snp never calls `tcsetpgrp`, so it can never leave the terminal
//! owned by a dead process group.
//!
//! Owner: WP-15 (ADR-085).

use std::fs::File;
use std::io;
use std::sync::Arc;

/// A usable terminal for an interactive hook.
#[derive(Clone, Debug)]
pub struct Terminal {
    /// `None`: snp's own stdin is the terminal (inherited); `Some`: `/dev/tty` / `CONIN$`, opened for the hook.
    device: Option<Arc<File>>,
}

/// Why no terminal is available.
#[derive(Debug)]
#[non_exhaustive]
pub enum NoTerminal {
    /// There is no controlling terminal / console (CI, cron, a pipe without a session terminal).
    Unavailable(io::Error),
    /// snp's process group is not the terminal's foreground group (a background job): reading would stop the hook.
    Background,
}

impl std::fmt::Display for NoTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(_) => f.write_str("no terminal is available"),
            Self::Background => {
                f.write_str("snp is not in the terminal's foreground process group")
            }
        }
    }
}

impl Terminal {
    /// The terminal of this process: snp's stdin when it is a terminal, else `/dev/tty` (Unix) / `CONIN$` (Windows).
    ///
    /// # Errors
    ///
    /// [`NoTerminal`] when none can be opened or (Unix) snp runs in a background process group.
    pub fn open() -> Result<Self, NoTerminal> {
        use std::io::IsTerminal as _;
        let device = if io::stdin().is_terminal() {
            None
        } else {
            Some(Arc::new(open_device().map_err(NoTerminal::Unavailable)?))
        };
        let t = Self { device };
        #[cfg(unix)]
        if !t.foreground() {
            return Err(NoTerminal::Background);
        }
        Ok(t)
    }

    /// The child's stdin.
    pub(crate) fn stdio(&self) -> io::Result<std::process::Stdio> {
        match &self.device {
            None => Ok(std::process::Stdio::inherit()),
            Some(f) => {
                let _lock = snp_core::spawnlock::guard();
                f.try_clone().map(std::process::Stdio::from)
            }
        }
    }

    /// Whether snp's process group is the terminal's foreground group.
    #[cfg(unix)]
    fn foreground(&self) -> bool {
        let pgrp = rustix::process::getpgrp();
        let fg = match &self.device {
            None => rustix::termios::tcgetpgrp(rustix::stdio::stdin()),
            Some(f) => rustix::termios::tcgetpgrp(&**f),
        };
        fg.is_ok_and(|g| g == pgrp)
    }

    /// The terminal's modes before the hook ran (Unix), restored after a hook that was killed.
    #[cfg_attr(
        not(unix),
        expect(clippy::unused_self, reason = "terminal modes are saved on Unix only")
    )]
    pub(crate) fn save(&self) -> Option<Saved> {
        #[cfg(unix)]
        {
            let t = match &self.device {
                None => rustix::termios::tcgetattr(rustix::stdio::stdin()),
                Some(f) => rustix::termios::tcgetattr(&**f),
            };
            t.ok().map(Saved)
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Puts back modes saved by [`Terminal::save`] (a killed prompt may leave the terminal raw or without echo).
    pub(crate) fn restore(&self, saved: &Saved) {
        #[cfg(unix)]
        {
            use rustix::termios::{OptionalActions, tcsetattr};
            let _ignored = match &self.device {
                None => tcsetattr(rustix::stdio::stdin(), OptionalActions::Now, &saved.0),
                Some(f) => tcsetattr(&**f, OptionalActions::Now, &saved.0),
            };
        }
        #[cfg(not(unix))]
        let _: (&Self, &Saved) = (self, saved);
    }
}

/// Saved terminal modes.
#[derive(Debug)]
pub(crate) struct Saved(#[cfg(unix)] rustix::termios::Termios);

/// `/dev/tty` read-write (close-on-exec: std opens with `O_CLOEXEC`).
#[cfg(unix)]
fn open_device() -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}

/// The console input buffer.
#[cfg(windows)]
fn open_device() -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
}
