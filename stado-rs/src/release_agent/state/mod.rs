//! The journal this host keeps for one product: its records, the document
//! they are committed as, the logs a refusal quotes, and the status the fleet
//! reads back.

pub(crate) mod document;
pub(crate) mod evidence;
pub(crate) mod records;
pub(crate) mod status;
