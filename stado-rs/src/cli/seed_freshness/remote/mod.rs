//! The two host reads this diagnostic makes, both read-only: the vault's half
//! through Skarbiec in [`skarbiec`], and the run history's half through the
//! host's own node in [`evidence`].

pub(in crate::cli::seed_freshness) mod evidence;
pub(in crate::cli::seed_freshness) mod skarbiec;
