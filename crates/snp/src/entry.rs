//! `entry()`: argv capture, panic hook, `__hazmat` raw dispatch, brand, parse, dispatch, envelope (§10.1).
//!
//! Owner: WP-33 (W0 skeleton by WP-00).
#![expect(
    clippy::todo,
    reason = "W0 stub, removed by WP-00 (binary skeleton) / WP-33"
)]

use crate::Exit;

/// The whole program except `process::exit` (§10.1): every guard and destructor has run when it returns.
pub fn entry() -> Exit {
    todo!()
}
