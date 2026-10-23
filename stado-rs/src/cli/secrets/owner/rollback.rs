//! Giving the vault back to its previous owner when the move failed after the
//! rotation.
//!
//! The rotation left the owner host's vault readable only by the new owner,
//! whose secret half lives on the destination, while the failed move left the
//! vault and the service on the owner host. The destination therefore rotates
//! a copy back: it takes the previous owner's public key from the owner
//! host's keyring, rotates the copy to that owner, and the rotated copy
//! replaces the owner host's vault. The owner host's own key-doctor must then
//! read it, or the restoration is reported as failed with that answer.

use serde_json::Value;

use super::plan::Plan;
use crate::deploy::Runner;

/// The vault status key-doctor gives when a key on that host opens it.
const READABLE: &str = "readable";

/// Restore the previous owner on the owner host; `Ok` carries key-doctor's
/// answer there, `Err` the step that failed and why.
pub(super) async fn restore_owner(plan: &Plan, runner: &Runner) -> Result<Value, String> {
    let public = plan
        .source
        .gpg(
            &["--armor", "--export", "--", &plan.previous.fingerprint],
            None,
            runner,
        )
        .await?;
    if public.trim().is_empty() {
        return Err(format!(
            "{}'s keyring exported no public key for the previous owner {}",
            plan.source.target.name, plan.previous.uid
        ));
    }
    plan.destination
        .gpg(&["--import"], Some(&public), runner)
        .await?;
    let rotated = plan.source.read_file(&plan.source.vault, runner).await?;
    let staged = format!("{}.restoring", plan.destination.vault);
    plan.destination
        .write_file(&staged, &rotated, runner)
        .await?;
    let restorer = plan.destination.with_vault(staged.clone());
    let rotation = restorer
        .skarbiec(&["rotate-owner", &plan.previous.uid], None, runner)
        .await;
    let restored = match rotation {
        Ok(_) => plan.destination.read_file(&staged, runner).await,
        Err(cause) => Err(cause),
    };
    let removed = plan.destination.remove_file(&staged, runner).await;
    let restored = restored?;
    removed?;
    plan.source
        .write_file(&plan.source.vault, &restored, runner)
        .await?;
    let doctor = plan.source.skarbiec(&["key-doctor"], None, runner).await?;
    match doctor.get("status").and_then(Value::as_str) {
        Some(READABLE) => Ok(doctor),
        _ => Err(format!(
            "{}'s key-doctor does not read the restored vault: {doctor}",
            plan.source.target.name
        )),
    }
}
