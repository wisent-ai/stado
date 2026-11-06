//! `stado placement take` — the operator half of a profile take-over: read
//! the registry, resolve the profile, run it on this host, keep the receipt
//! in the placement history and print it.

use crate::cli::{registry, CmdError};
use crate::placement;

pub(super) async fn take(
    requested: &[String],
    reason: &str,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let profile =
        placement::profile_for_services(&document, requested).map_err(|error| match error {
            placement::ProfileLookupError::Request(detail) => CmdError::usage(detail),
            placement::ProfileLookupError::NoMatch(detail) => CmdError::missing(detail),
            placement::ProfileLookupError::Declaration(detail) => CmdError::declaration(detail),
        })?;
    crate::cli::placement::candidates::ensure_profile_lifecycle_mutable(&profile)?;
    // The vault's owner key does not move with its ciphertext, here as in a
    // move: a vault taken without it lands where no key opens it.
    let vault = crate::cli::secrets::VAULT_SERVICE;
    if profile.services.iter().any(|service| service == vault) {
        return Err(CmdError::refused(format!(
            "placement profile {} carries {vault}, the fleet vault, whose owner key a take cannot \
             rotate; move it with stado credentials vault move once its host answers",
            profile.name
        )));
    }
    let here = crate::cli::resolver::current_target(&document).map_err(CmdError::declaration)?;
    let taken = crate::cli::placement::transfer::take_profile(
        document, generation, profile, &here, reason, check,
    )
    .await;
    if !check {
        crate::cli::placement::history::record("take", taken.as_ref());
    }
    let receipt = taken?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    let (profile, from, to) = (
        &receipt["profile"],
        &receipt["from_host"],
        &receipt["to_host"],
    );
    match receipt["state"].as_str() {
        Some("would_take") => println!(
            "{to} can take {profile} from {from}, which does not answer; nothing was started \
             or written"
        ),
        _ => println!(
            "{to} took {profile} from {from}, which does not answer (registry generation {})",
            receipt["registry_generation"]
        ),
    }
    for (state, age) in receipt["state_age"].as_object().into_iter().flatten() {
        match age["modified"].as_str() {
            Some(modified) => println!(
                "  {state}: {to}'s copy was last written {modified} ({}s ago); what {from} \
                 took after that is not in it",
                age["seconds_ago"]
            ),
            None => println!("  {state}: {to}'s copy holds no file"),
        }
    }
    for state in receipt["not_recovered"].as_array().into_iter().flatten() {
        println!("  not recovered: {state}, which {to} did not hold");
    }
    for unit in receipt["unretired_source"].as_array().into_iter().flatten() {
        println!("  still on {from}: {unit}; retire it there before {from} serves again");
    }
    Ok(())
}
