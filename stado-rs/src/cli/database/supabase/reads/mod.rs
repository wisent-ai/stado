//! What Stado reads of the Supabase projects its declared databases live
//! in: the bill ([`cost`]) and whether each project runs ([`status`]).

pub(in crate::cli::database) mod cost;
pub(crate) mod status;

use std::collections::BTreeMap;

use crate::cli::CmdError;

/// The declared databases whose item names a Supabase project, as
/// database name to project reference, and the items that could not be read
/// to tell, each with its cause.
async fn hosted_projects() -> Result<(BTreeMap<String, String>, Vec<String>), CmdError> {
    let mut hosted = BTreeMap::new();
    let mut unread = Vec::new();
    for (name, declaration) in super::super::declared_databases()? {
        match crate::credential_store::read_declared_string(declaration.item(), "project_ref").await
        {
            Ok(Some(reference)) => {
                hosted.insert(name.clone(), reference);
            }
            Ok(None) => {}
            Err(error) => unread.push(format!("{}: {error}", declaration.item())),
        }
    }
    Ok((hosted, unread))
}
