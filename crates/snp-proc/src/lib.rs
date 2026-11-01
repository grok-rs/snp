//! Process spawning (pipe/pty, stdin rules, isolated groups, output budget and spill, deadlines), process roles,
//! the child registry with origin-aware signal forwarding and descendant cleanup, `JobPool`/`Limiter`, executable
//! resolution, xargs partitioning and hook-command parsing (ARCHITECTURE §1.4.8, §3.6, §10.4).
//!
//! Owner: WP-15.

pub mod hookcmd;
mod messages;
pub mod pool;
pub mod registry;
pub mod resolve;
pub mod signals;
pub mod spawn;
#[cfg(feature = "testing")]
pub mod spawn_log;
mod trace;
pub mod tty;
pub mod xargs;
