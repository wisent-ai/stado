//! What a freshness answer is made of: the vault's vocabulary and one reduced
//! sign-in attempt in [`inputs`], the eight outcomes and their repairs in
//! [`outcome`], and the decision between them in [`classify`].

pub(in crate::cli::seed_freshness) mod classify;
pub(in crate::cli::seed_freshness) mod inputs;
pub(in crate::cli::seed_freshness) mod outcome;
