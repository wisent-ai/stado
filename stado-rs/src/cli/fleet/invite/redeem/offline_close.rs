//! The offline mode's redemption: registering the invited name is the only
//! thing that can ever spend an invite which carries no secret.

use crate::cli::fleet::invite::record::store::list_invites;
use crate::cli::fleet::invite::record::{MODE_OFFLINE, STATUS_REVOKED, STATUS_SPENT};
use crate::queue::JobStorage;

use super::mark_spent;

/// Close the offline invite a fresh registration satisfied, if there is one.
///
/// Registering the name IS the redemption of an offline invite: it has no
/// secret and no route, so nothing else can ever spend it, and the operator
/// only gets to `fleet enroll` because the fragment installed the key and the
/// owner sent the address back. A revoked invite is left alone — revocation is
/// a deliberate refusal that enrolling the name by hand must not undo — and the
/// transition itself is [`mark_spent`], the very one `approve` drives for an
/// online invite.
///
/// Returns the id it closed, so the caller can say which one.
pub async fn close_offline_for_target(name: &str) -> Result<Option<String>, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let found = list_invites(&store).await?;
    let Some((invite, _)) = found.iter().find(|(invite, _)| {
        invite.mode == MODE_OFFLINE
            && invite.target_name == name
            && invite.status != STATUS_REVOKED
            && invite.status != STATUS_SPENT
    }) else {
        return Ok(None);
    };
    mark_spent(&store, &invite.id).await?;
    Ok(Some(invite.id.clone()))
}
