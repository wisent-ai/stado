//! The provider API calls: what the edge is made of on Azure, and what it
//! takes to remove one of those resources and know it is gone. The request
//! bodies, the verified delete and the rollback are the ones every public
//! Azure VM is built from ([`crate::providers::azure::public`]).

use super::{mutate_web, CmdError};

mod provision;
mod removal;

pub(in crate::cli::web::edge) use provision::provision;
pub(in crate::cli::web::edge) use removal::remove;

/// The `wisent_role` tag every edge resource carries.
const EDGE_ROLE_TAG: &str = "web-edge";

/// Azure's refusal, plus whatever the rollback could not remove.
fn refusal(context: &str, error: impl ToString, leftovers: &[String]) -> CmdError {
    let mut message = format!("{context}: {}", error.to_string());
    if !leftovers.is_empty() {
        message.push_str(&format!(
            "; these resources were created and could not be removed, so they are still \
             billing: {}",
            leftovers.join("; ")
        ));
    }
    CmdError::refused(message)
}
