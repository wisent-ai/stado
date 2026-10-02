//! The Supabase adapter of `stado database destroy`: the inverse of
//! `create --provider supabase`. The hosted project the credential item names
//! by `project_ref` is deleted through the management API, with every row it
//! holds; `destroy` asks for `--delete-project` before it gets here.

use crate::cli::CmdError;

/// Delete the project `item` names. A project the API no longer knows is
/// reported as already absent, so a destroy that stopped after this step can
/// be run again.
pub(in crate::cli::database) async fn delete_project(
    item: &str,
) -> Result<(String, &'static str), CmdError> {
    let reference = super::field(item, "project_ref").await?;
    let token = super::token().await?;
    let path = format!("/projects/{reference}");
    let (status, text) = super::answer(reqwest::Method::DELETE, &path, &token, None).await?;
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok((reference, "already absent"));
    }
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Supabase DELETE {path} answered {status}: {text}"
        )));
    }
    Ok((reference, "deleted"))
}
