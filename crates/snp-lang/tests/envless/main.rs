//! The env-less backends (WBS WP-16 acceptance: "env-less plan goldens (`docker_image` argv, `unsupported_script`
//! prefix join, fail and pygrep `RunPlan`s)"; 10 §5.10 rows for `fail`, `pygrep`, `unsupported`,
//! `unsupported_script`, `docker_image`), plus the oracle-recorded golden of their argv (shim harness against
//! pre-commit 4.7.0). The modules are declared `#[cfg(test)]` so their `#[test]` functions and helpers count as test
//! code; the plan tests need the `testing` feature (the harness).
//!
//! Owner: WP-16.

#[cfg(test)]
mod backends;
#[cfg(test)]
#[cfg(feature = "testing")]
mod oracle;
#[cfg(test)]
#[cfg(feature = "testing")]
mod plans;
