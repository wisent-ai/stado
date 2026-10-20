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
    match report.moved_items {
        Some(moved) => println!(
            "migrated {moved} credential item(s): {} -> {}",
            report.source, report.destination
        ),
        None => println!(
            "credentials already live in {}: the selector is kept and nothing was copied",
            report.destination
        ),
    }
    Ok(())
}
