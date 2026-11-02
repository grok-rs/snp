//! The group-2 language backends (WBS WP-25): docker, conda, coursier, dart, dotnet, haskell, julia, lua, perl, r,
//! swift.
//!
//! * `oracle` — the shim-harness golden logs (argv/cwd/env of every install and run command, plus the bytes of the
//!   temporary R scripts and `nuget.config` files) compared with the logs recorded from pre-commit 4.7.0 running the
//!   same scenarios with fake tools on `PATH` (04b §5.12);
//! * `docker` — tags (ADR-035), the per-run build step (D-PERF-04), rootless detection (D-CRASH-21), the mountinfo and
//!   inspect parsers with pre-commit's verbatim fixtures, bind-path translation;
//! * `langs` — the per-language rules: conda installer selection, coursier/haskell errors, dart pubspec reading and
//!   executable-name validation, dotnet nuspec parsing and its size cap, julia's ignored version, the lua version probe
//!   memoised per run, perl quoting, r entry validation, script literal and health fast path, swift;
//! * `pubspec` — the dart pubspec reader replayed against the PyYAML-generated `testdata/group2/pubspec` vectors;
//! * `support` — the `check_support` texts of every group-2 language in both brands;
//! * `toolchain` — real-toolchain tests (`tc_<lang>_*`, `#[ignore = "toolchain"]`, run nightly where the tool exists).
//!
//! The modules are declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test code; everything
//! needs the `testing` feature (the shim harness).
//!
//! Owner: WP-25.
#![cfg_attr(
    windows,
    allow(
        dead_code,
        unused_imports,
        reason = "most cases are posix-only (sh scripts, modes, signals): on Windows their shared helpers go unused"
    )
)]

#[cfg(test)]
#[cfg(feature = "testing")]
mod docker;
#[cfg(test)]
#[cfg(feature = "testing")]
mod langs;
#[cfg(test)]
#[cfg(feature = "testing")]
mod oracle;
#[cfg(test)]
#[cfg(feature = "testing")]
mod pubspec;
#[cfg(test)]
#[cfg(feature = "testing")]
mod support;
#[cfg(test)]
#[cfg(feature = "testing")]
mod toolchain;
#[cfg(test)]
#[cfg(feature = "testing")]
mod util;
