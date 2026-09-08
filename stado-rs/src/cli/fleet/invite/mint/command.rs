//! `stado fleet invite`: probe, mint, record, and print exactly one of the two
//! things the machine's owner can act on.

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::fleet::invite::checkpoint::base::{
    enrollment_base, resolve_invite_base, BASE_FROM_ENROLLMENT_URL, BASE_FROM_INGRESS,
};
use crate::cli::fleet::invite::checkpoint::{
    checkpoint_document, probe_checkpoint, Checkpoint, REASON_FORCED_OFFLINE,
};
use crate::cli::fleet::invite::record::store::list_invites;
use crate::cli::fleet::invite::record::{
    invite_document, invite_path, secret_digest, Invite, MODE_OFFLINE, STATUS_OPEN,
};
use crate::queue::JobStorage;

use super::offline::offline_snippet;
use super::preflight::preflight_invite_name;
use super::{
    derived_target_name, discard_minted_key, join_command, mint_id, mint_secret, parse_expiry,
};

/// `stado fleet invite [--name NAME] [--expires 24h] [--uses 1] [--offline]` —
/// mint the channel key for a machine nobody has touched yet, plus the thing
/// its owner has to run.
///
/// What that thing is depends on whether a control point can actually serve
/// `/join.sh`, which is probed here before anything is minted. Reachable: the
/// one line, exactly as before. Not reachable, for any of the three reasons
/// [`probe_checkpoint`] distinguishes: the offline fragment instead, with the
/// reason said out loud. `offline` skips the probe and takes that path on
/// purpose. The one-liner is never printed for an address that did not answer —
/// a command that cannot work is worse than no command, because the operator
/// finds out from the machine's owner.
///
/// The rest of the order is unchanged and deliberate: the key is minted before
/// the invite is recorded, because an invite whose key does not exist fails on
/// somebody else's laptop, and everything that can still refuse — recording the
/// object, building the fragment — removes that freshly minted credential item
/// again. A half-minted invite leaves nothing behind.
pub async fn invite(
    name: Option<&str>,
    expires: &str,
    uses: u64,
    offline: bool,
    as_json: bool,
) -> Result<bool, String> {
    if uses == 0 {
        return Err("--uses must be at least 1".to_string());
    }
    let lifetime = parse_expiry(expires)?;
    let document = crate::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    crate::cli::fleet::enroll::catalog::require_invite_allowed(&document)?;
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let live = list_invites(&store).await?;
    let id = mint_id()?;
    let target_name = match name {
        Some(given) => given.to_string(),
        None => derived_target_name(&id),
    };
    preflight_invite_name(&document, &live, &target_name)?;

    // The control point comes from configuration or from an entrance this
    // fleet published — never from a name compiled into this binary. A built-in
    // default would be exactly the silent guess that printed a one-liner for
    // a host nobody deployed.
    //
    // `enrollment.url` wins when it is set, because a written-down decision
    // outranks anything discovered here. Next comes the live
    // `enrollments/ingress.json`, the entrance `fleet ingress up` verified from
    // the internet — without it the one-line mode has nothing to point at on a
    // fleet with no public deployment. `api.url` stays the release/deployment
    // endpoint and is the last one tried, so a deployment that never configured
    // a separate enrollment origin behaves exactly as it did before. All three
    // empty still means `not_configured`.
    //
    // `--offline` does not consult the ingress: it probes nothing by definition,
    // and an unprobed tunnel address is not a base, it is a guess.
    let (base, base_source) = if offline {
        (enrollment_base(), BASE_FROM_ENROLLMENT_URL)
    } else {
        resolve_invite_base(&store).await
    };
    let checkpoint = if offline {
        Checkpoint {
            url: base.clone(),
            probed: false,
            reachable: false,
            reason: REASON_FORCED_OFFLINE,
            detail: "--offline was requested, so the control point was not probed".to_string(),
        }
    } else {
        probe_checkpoint(&base).await
    };
    let from_ingress = base_source == BASE_FROM_INGRESS;
    let mode = checkpoint.mode();
    // Offline mints no secret at all, rather than minting one and being trusted
    // to forget it.
    let secret = match mode {
        MODE_OFFLINE => None,
        _ => Some(mint_secret()?),
    };

    let runner = crate::deploy::production_runner();
    let (public_key, fingerprint) =
        crate::cli::fleet::key::rotate::generate_stored(&runner, &target_name).await?;
    let line = crate::cli::fleet::key::authorized_keys_line(
        &public_key,
        &crate::cli::fleet::key::item_id(&target_name),
    );
    let snippet = match mode {
        MODE_OFFLINE => match offline_snippet(&target_name, &line) {
            Ok(snippet) => Some(snippet),
            Err(detail) => {
                discard_minted_key(&target_name).await;
                return Err(format!(
                    "could not build the offline fragment ({detail}); the minted key for '{target_name}' was removed"
                ));
            }
        },
        _ => None,
    };

    let created_at = Utc::now();
    let invite = Invite {
        id: id.clone(),
        secret_sha256: secret.as_deref().map(secret_digest).unwrap_or_default(),
        target_name: target_name.clone(),
        created_at: created_at.to_rfc3339(),
        expires_at: (created_at + lifetime).to_rfc3339(),
        uses_allowed: uses,
        uses_spent: 0,
        status: STATUS_OPEN.to_string(),
        created_by: crate::providers::vast::system_hostname(),
        mode: mode.to_string(),
    };
    let recorded = store
        .create_text_if_absent(
            &invite_path(&id),
            &serde_json::to_string_pretty(&invite_document(&invite))
                .map_err(|exc| exc.to_string())?,
        )
        .await;
    match recorded {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            let detail = match recorded {
                Err(exc) => exc.to_string(),
                _ => format!("invite id {id} already exists in the store"),
            };
            discard_minted_key(&target_name).await;
            return Err(format!(
                "could not record the invite ({detail}); the minted key for '{target_name}' was removed"
            ));
        }
    }

    let token = secret.as_deref().map(|secret| format!("{id}.{secret}"));
    let command = token
        .as_deref()
        .map(|token| join_command(&checkpoint.url, token));
    let next_step = format!("stado fleet enroll {target_name} --ssh <address> --bootstrap");
    if as_json {
        let mut rendered = json!({
            "id": invite.id,
            "mode": invite.mode,
            "target_name": invite.target_name,
            "created_at": invite.created_at,
            "expires_at": invite.expires_at,
            "uses_allowed": invite.uses_allowed,
            "public_key": public_key,
            "authorized_keys_line": line,
            "checkpoint": checkpoint_document(&checkpoint),
            "base_source": base_source,
            "base_is_temporary": from_ingress,
        });
        match (&token, &command, &snippet) {
            (Some(token), Some(command), _) => {
                rendered["token"] = Value::String(token.clone());
                rendered["token_shown_once"] = Value::Bool(true);
                rendered["join_command"] = Value::String(command.clone());
            }
            (_, _, Some(snippet)) => {
                rendered["snippet"] = Value::String(snippet.clone());
                rendered["snippet_is_not_a_secret"] = Value::Bool(true);
                rendered["next_step"] = Value::String(next_step.clone());
            }
            _ => {}
        }
        if from_ingress {
            rendered["base_warning"] = Value::String(format!(
                "{} is a temporary Cloudflare quick-tunnel address published by 'stado fleet \
                 ingress'; this one-liner stops working the moment that ingress is stopped, and a \
                 restarted ingress comes back under a different address",
                checkpoint.url
            ));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    println!(
        "invite {} for target '{}' (mode: {})",
        invite.id, invite.target_name, invite.mode
    );
    match (&token, &command, &snippet) {
        (Some(token), Some(command), _) => {
            println!("token: {token}");
            println!("  this is the only time the token is shown; nothing can reprint it");
            println!("expires: {} (uses allowed: {uses})", invite.expires_at);
            println!("channel key minted: {fingerprint}");
            println!("control point: {}", checkpoint.detail);
            println!("send this one line to the machine's owner:");
            println!("  {command}");
            println!(
                "then approve the machine: stado fleet pending, stado fleet approve <hostname>"
            );
            if from_ingress {
                println!(
                    "  that address is a TEMPORARY Cloudflare quick-tunnel address, published by \
                     'stado fleet ingress'."
                );
                println!(
                    "  the one line above stops working the moment the ingress is stopped, and a"
                );
                println!(
                    "  restarted ingress comes back under a DIFFERENT address — an invitation \
                     handed out"
                );
                println!(
                    "  before a restart is dead. Keep the ingress standing until the machine has \
                     joined,"
                );
                println!("  and check it with: stado fleet ingress status");
            }
        }
        (_, _, Some(snippet)) => {
            println!(
                "no token exists for an offline invite; there is nothing here to intercept or replay"
            );
            println!("expires: {} (uses allowed: {uses})", invite.expires_at);
            println!("channel key minted: {fingerprint}");
            if checkpoint.probed {
                println!("control point: {}", checkpoint.detail);
            } else {
                println!("control point not probed: {}", checkpoint.detail);
            }
            println!("switched to the offline invite method, which needs no HTTP route at all.");
            println!();
            println!(
                "paste everything between the two markers into a terminal ON THE MACHINE BEING ADDED:"
            );
            println!("----- 8< ----- stado offline invite for '{target_name}' ----- 8< -----");
            print!("{snippet}");
            println!("----- 8< ----- end of fragment ----- 8< -----");
            println!(
                "this fragment carries only the fleet's PUBLIC key, so it is not a secret: whoever reads it gains nothing."
            );
            println!("  the private half never leaves the operator's vault.");
            println!(
                "the owner runs it and sends back the user@address it prints on its last line."
            );
            println!("when that address arrives, run: {next_step}");
        }
        // Unreachable: online carries a token and a command, offline a
        // fragment, and the mode chose one of the two before the key was minted.
        _ => {
            return Err(
                "the invite was recorded but neither mode produced anything to send".to_string(),
            );
        }
    }
    Ok(true)
}
