//! The three ledgers a decision leaves behind: `feedback` is what the
//! placement actually did, `savings` is what it claimed to save and the
//! measurement that checked the claim, and `adoptions` is which pre-existing
//! resource this control plane took responsibility for. Each record is
//! written once, immutably, and named for the thing it answers.

pub(in crate::autonomy::storage) mod adoptions;
pub(in crate::autonomy::storage) mod feedback;
pub(in crate::autonomy::storage) mod savings;
