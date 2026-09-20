//! `stado credentials seed-enrol --host TARGET --login-item ITEM` — put an
//! authenticator seed into a login row that has none.
//!
//! # Why this exists
//!
//! `seed-freshness` next door answers whether a login row still holds a seed
//! its account accepts, and for every Google row in this fleet the answer has
//! been the same: `declared_empty`. The consequence is not cosmetic. Brama's
//! automatic sign-in reaches Google's second factor and stops there
//! (`google_2fa_material_missing`), so every claude-code, codex and kimi
//! subscription burns out and the gateway answers
//! `subscription_reauthorization_required` to everything that asks it for a
//! model — including Oko's judge.
//!
//! Weles has been able to fix that since the enrolment trajectory was written:
//! `google_authenticator_enrol` signs in, opens Google's authenticator setup,
//! reads the setup key, confirms the first code, and writes the seed into the
//! same Skarbiec login item. What was missing was any way to ASK for it. The
//! action existed in the worker's dispatch table and no product surface named
//! it, so the seeds stayed empty and every sign-in waited on a person's phone.
//!
//! # What crosses the channel
//!
//! The item id and nothing else. The seed is read by the trajectory on the
//! host and written by it into that host's Skarbiec; it never enters this
//! process, an argument list, or the report. What comes back is the run's
//! redacted envelope, and afterwards the vault's own verdict on the row, read
//! through the same `skarbiec totp-seed-state` reader `seed-freshness` uses —
//! because a run that answered `ok` is not evidence that a seed landed.

use serde_json::json;

use crate::cli::seed_freshness::seed_states;
use crate::cli::CmdError;
use crate::deploy::weles_capture;

/// The Weles action that enrols one authenticator and stores its seed.
const ENROL_ACTION: &str = "google_authenticator_enrol";

/// Ask one host's Weles worker to enrol an authenticator for one login row,
/// then report what the vault holds afterwards.
pub async fn enrol_authenticator_seed(
    host: &str,
    login_item: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let login_item = checked_login_item(login_item)?;
    let before = seed_state_of(host, login_item).await?;
    if before.as_deref() == Some("present") {
        return report(
            json_output,
            json!({
                "host": host,
                "login_item": login_item,
                "state": "unchanged",
                "seed_state": before,
                "detail": "the row already holds a usable seed; enrolling again would replace a working second factor",
            }),
        );
    }

    let admission = weles_capture::resolve_admission(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let channel = weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let run = weles_capture::run_action(&channel, ENROL_ACTION, json!({"login_item": login_item}))
        .await;
    let after = seed_state_of(host, login_item).await?;
    match run {
        Ok(run_id) => report(
            json_output,
            json!({
                "host": host,
                "login_item": login_item,
                "state": if after.as_deref() == Some("present") { "enrolled" } else { "ran_without_a_seed" },
                "run_id": run_id,
                "seed_state": after,
                "detail": if after.as_deref() == Some("present") {
                    "Weles enrolled an authenticator and the vault now holds its seed".to_string()
                } else {
                    "the enrolment run finished and the row still holds no usable seed; read the run's own diagnostics on the host".to_string()
                },
            }),
        ),
        // A refusal is the common answer while no seed exists anywhere: the
        // first sign-in from a profile Google does not recognise is answered
        // by a push to the account owner's phone, and the trajectory says so
        // rather than pretending it failed for another reason.
        Err(error) => Err(CmdError::click(format!(
            "Weles refused to enrol an authenticator for {login_item} on {host}: {error}; the row's seed state is {}",
            after.as_deref().unwrap_or("unreadable")
        ))),
    }
}

/// What the host's vault says about this row's seed, in its own vocabulary.
async fn seed_state_of(host: &str, login_item: &str) -> Result<Option<String>, CmdError> {
    let states = seed_states(host, Some(login_item))
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    Ok(states
        .into_iter()
        .find(|(item, _)| item == login_item)
        .map(|(_, state)| state))
}

/// One vault item id, refused rather than forwarded when it could be anything
/// else: it becomes a parameter of a run on another machine.
fn checked_login_item(login_item: &str) -> Result<&str, CmdError> {
    let trimmed = login_item.trim();
    let shaped = !trimmed.is_empty()
        && trimmed.len() <= MAX_ITEM_LENGTH
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '@'));
    if !shaped {
        return Err(CmdError::click(format!(
            "`{login_item}` is not a Skarbiec item id; give the exact id `seed-freshness` prints"
        )));
    }
    Ok(trimmed)
}

/// Skarbiec's own item-id bound.
const MAX_ITEM_LENGTH: usize = 128;

fn report(json_output: bool, document: serde_json::Value) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&document).unwrap_or_default());
        return Ok(());
    }
    for (field, value) in document.as_object().into_iter().flatten() {
        println!("{field}: {value}");
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/credentials/seed_enrol.rs"]
mod tests;
