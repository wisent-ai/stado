//! Committing a new credential backend.

use crate::cli::CmdError;

pub(crate) async fn migrate(destination: Option<&str>) -> Result<(), CmdError> {
    let credentials = crate::credential_store::admin_credentials().map_err(CmdError::from)?;
    let report = crate::credential_store::migrate::migrate(
        destination,
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .await
    .map_err(CmdError::from)?;
    println!(
        "migrated {} credential item(s): {} -> {}",
        report.moved_items, report.source, report.destination
    );
    Ok(())
}
