//! Group-1 language backends (WBS WP-24): python, node, golang, rust, ruby — the 10 §5.10 rows of these languages
//! (pure-logic rows with injected probes; the hermetic parts of the health-check and hook tests), the shim-harness
//! goldens recorded from pre-commit 4.7.0 (`oracle`), the uv command lines and pip-config fallback triggers, the
//! health fast path, the installer switch, deps-variant isolation, and the nightly real-toolchain tests
//! (`tc_<lang>_*`, `#[ignore = "toolchain"]`). Marked `posix` (ARCHITECTURE §13): the fixtures are POSIX paths, `:`
//! `PATH`s and `bin/` layouts; the pure-logic rows also run on every OS as unit tests in `src/langs/`. The modules are
//! declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test code; everything needs the `testing`
//! feature (the harness).
//!
//! Owner: WP-24.

#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod golang;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod messages;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod node;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod oracle;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod python;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod ruby;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod rust;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod support;
#[cfg(test)]
#[cfg(feature = "testing")]
#[cfg(unix)]
mod toolchains;
