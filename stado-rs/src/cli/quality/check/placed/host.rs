//! `stado quality check --bundle`: the check a placed job runs on the host
//! its placement chose.

use serde_json::json;

use super::bundle;
use crate::cli::quality::{Report, Selection};
use crate::cli::CmdError;

/// Check `revision` over the repository rebuilt from the bundle at `key`, in
/// Stado's own area on this host, removed when the check ends; `purposes`
/// are named to Oko's admission as a checkout's check names them.
pub(crate) async fn check_bundle(
    key: &str,
    revision: &str,
    purposes: &[&str],
    json: bool,
) -> Result<(), CmdError> {
    let report = if json { Report::Stderr } else { Report::Stdout };
    let area = bundle::host_area(key)?;
    let checked = match crate::queue::submit::default_store("").await {
        Ok(store) => match bundle::unpack(&store, key, revision, &area).await {
            Ok(repository) => {
                crate::cli::quality::check_revision_for(
                    &repository,
                    revision,
                    report,
                    Selection::Installation,
                    purposes,
                )
                .await
            }
            Err(error) => Err(error),
        },
        Err(error) => {
            Err(CmdError::from(error).within("open the fleet store for the source bundle"))
        }
    };
    let removed = bundle::remove(&area);
    checked.map_err(|error| error.machine_readable(json))?;
    removed?;
    if json {
        println!("{}", json!({"status": "passed", "revisions": [revision]}));
    }
    Ok(())
}
