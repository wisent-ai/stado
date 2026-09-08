//! The conditional-write helpers every registry mutation shares: the refused
//! write as one value ([`conflict`]), the refusals a whole-document replace
//! earns on its own contents ([`guards`]), the verbatim payload upload
//! ([`upload`]), and the validated read-modify-write path ([`document`]).

pub(in crate::cli::registry) mod conflict;
pub(in crate::cli::registry) mod document;
mod guards;
pub(in crate::cli::registry) mod upload;
