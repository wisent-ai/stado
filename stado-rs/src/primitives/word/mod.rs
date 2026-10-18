//! The word a closed vocabulary's variant is: its serde name, which is also
//! what reports, plans and tables print. The writer lives in the product
//! crate, which Stado and the product lifecycle both build on, so there is
//! one spelling rule for every vocabulary of either.

pub use stado_product::word::write;
