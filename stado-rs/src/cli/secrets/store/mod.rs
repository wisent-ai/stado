//! The selected credential store: how this process resolves it, the
//! inventories it lists, the fields it delivers, and the grants it mints.

pub(in crate::cli::secrets) mod grants;
pub(in crate::cli::secrets) mod inventory;
pub(in crate::cli::secrets) mod items;
pub(in crate::cli::secrets) mod resolve;
