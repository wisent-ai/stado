//! `service file sync` and `service file fetch`: one file into or out of a
//! managed service's target home, byte-exact both ways.

use super::*;

pub(crate) mod fetch;
pub(crate) mod sync;

/// Replace one local path with these bytes, owner-only, whole and durably
/// ([`crate::primitives::document::persist`]). The staging file is created
/// owner-only, so a fetched secret is never readable by others on its way
/// in, as it was while a fixed `.stado-file-fetch` sibling waited for its
/// chmod.
fn write_owner_only(destination: &str, content: &[u8]) -> Result<(), CmdError> {
    crate::primitives::document::persist(std::path::Path::new(destination), content).map_err(
        |error| {
            CmdError::click(format!("cannot install {destination}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        },
    )
}
