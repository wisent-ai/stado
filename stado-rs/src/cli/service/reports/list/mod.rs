//! `service list`: the three fleet-wide questions about units, each answered
//! by a different party — the beacons, launchd's process table, and the
//! host's loaded jobs against the document.

use super::*;

mod undeclared;
mod unowned;

pub(crate) use undeclared::list_undeclared;
pub(crate) use unowned::list_unowned;

// ---------------------------------------------------------------------------
// Read commands
// ---------------------------------------------------------------------------

pub(crate) async fn list(json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    render_status(&rows, json, &[])
}
