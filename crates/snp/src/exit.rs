//! The exit status `entry` hands to `main` (§10.1–§10.2).
//!
//! Owner: WP-33.

/// What `main` does with the result of [`crate::entry`]. Exhaustive (ARCHITECTURE §1.4.24).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    /// 0–255, returned through `std::process::ExitCode`.
    Code(u8),
    /// Anything else (Windows hook-impl `legacy | run` > 255, 14 A.7.4): `main` passes it to `process::exit`
    /// after every destructor of `entry` ran.
    Raw(i32),
}
