//! `snp_snapshot!`: insta snapshots with the `TestEnv` filters (10 §7.3.4).
//!
//! Owner: WP-18. W0 stub: the expansion is a `todo!` in the calling crate until WP-18 wires insta (this file
//! carries no `#![expect(clippy::todo)]` because the macro body is linted where it expands).

/// Snapshots `$out` (an [`crate::Output`]) with the filters of `$env` (a [`crate::TestEnv`]).
#[macro_export]
macro_rules! snp_snapshot {
    ($env:expr, $out:expr) => {
        ::core::todo!("snp_snapshot! (W0 stub, WP-18): {:?} {:?}", &$env, &$out)
    };
}
