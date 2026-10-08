//! Process spawning, roles, signals, child registry, `JobPool`, xargs and executable resolution (ARCHITECTURE §1.4.8).
//!
//! Owner: WP-15.

pub mod hookcmd;
pub mod pool;
pub mod registry;
pub mod resolve;
pub mod signals;
pub mod spawn;
#[cfg(feature = "testing")]
pub mod spawn_log;
pub mod xargs;
