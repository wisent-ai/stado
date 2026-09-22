//! The stable bind: who owns it, who is handed it back, what routes it, and
//! the probe that proves the release behind it answers.

pub(crate) mod answer;
pub(crate) mod legacy;
pub(crate) mod owner;
pub(crate) mod proxy;

/// The stable bind's owner, under the name every caller has always used.
pub(crate) use owner::discover;
