//! WP-18 acceptance beyond the catalog rows (WBS "WP-18 Acceptance"): the data-safety hazards (a)–(d), the
//! owned-repository environment, prefetch eligibility, the scripted backend, the diff fingerprint, the version policy,
//! the `--deduplicate` fallback, the hook-script classifier and the rest of the user-repo API — against real git.

#[cfg(test)]
mod exe;
#[cfg(test)]
mod fingerprint;
#[cfg(test)]
mod hazards;
#[cfg(test)]
mod hookscripts;
#[cfg(test)]
mod owned;
#[cfg(test)]
#[cfg(feature = "testing")]
mod scripted;
#[cfg(test)]
mod user_repo;
