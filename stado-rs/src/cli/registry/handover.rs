//! `stado registry push PATH|- --last-known-good`: hand this host a registry
//! while its registry authority does not answer.
//!
//! Every registry write goes to the authority, so while the authority is
//! down nothing could tell a host's resolver about a registry that names
//! another authority. This writes the document into the host's
//! last-known-good copy instead, and the host's resolver adopts it on its
//! next refresh that its authority does not answer. Only a document whose
//! service-directory generation is above the copy's is taken, so a copy is
//! never rolled back this way, and an authority that answers is refused: a
//! registry it would serve goes through it.

use serde_json::Value;

use crate::cli::resolver::kept_last_good_document;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::service_resolution;
use crate::targets;

/// The store version a handed-over copy records: no store issued it.
const HANDED_OVER_VERSION: &str = "handed-over";

/// Write `payload`, already parsed as `document` and validated, into this
/// host's last-known-good copy.
pub(in crate::cli::registry) async fn push_last_known_good(
    document: &Value,
    payload: &str,
    source: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let unanswered = match crate::cli::resolver::authority_snapshot().await {
        Ok(Some(_)) => {
            return Err(CmdError::click(
                "the registry authority answers; a registry it would serve is pushed through \
                 it, without --last-known-good",
            )
            .stating(FailureCode::Refused))
        }
        Ok(None) => joining(document)?,
        Err(error) => error.to_string(),
    };
    let handed = directory_generation(document).map_err(|cause| {
        CmdError::click(format!("{source}: {cause}")).stating(FailureCode::Config)
    })?;
    let kept = kept_last_good_document()
        .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))?;
    // A copy with no service directory (a standalone Stado's) holds no
    // generation to roll back.
    let held = kept
        .as_ref()
        .map(service_resolution::directory)
        .transpose()
        .map_err(|cause| {
            CmdError::click(format!("this host's last-known-good copy: {cause}"))
                .stating(FailureCode::Config)
        })?
        .flatten()
        .map(|directory| directory.generation);
    if let Some(held) = held.filter(|held| handed <= *held) {
        return Err(CmdError::click(format!(
            "{source} carries service directory generation {handed}, not above the {held} \
             this host's last-known-good copy holds; a copy is never rolled back"
        ))
        .stating(FailureCode::Refused));
    }
    // The copy is the document this write replaces, so it tells a fault the
    // copy already held from one this document brings, as a store push does.
    let pre_existing =
        targets::validate_registry_for_write(document, kept.as_ref()).map_err(|error| {
            CmdError::click(format!("{source}: {error}")).stating(FailureCode::Refused)
        })?;
    crate::cli::registry::write::document::warn_scoped_validation(pre_existing);
    targets::store_last_good(payload, HANDED_OVER_VERSION).map_err(|refusal| {
        CmdError::click(format!(
            "this host's last-known-good copy refused {source}: {refusal}"
        ))
        .stating(FailureCode::Config)
    })?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "state": "handed-over",
                "directory_generation": handed,
                "authority_unanswered": unanswered,
            }))?
        );
    } else {
        println!(
            "handed {source} to this host's last-known-good copy at service directory \
             generation {handed}; the authority did not answer: {unanswered}"
        );
    }
    Ok(())
}

/// Why a host whose copy names no remote authority may still take `document`:
/// it keeps no copy, or a copy that declares no service directory (a
/// standalone Stado being attached to a fleet), and `document` names another
/// host as its authority and this host as one of its targets. Such a host
/// has no authority to ask, which is the state `fleet attach` ends. A host
/// that is the authority of its copy, or that `document` itself names as
/// authority or does not list, is refused.
fn joining(document: &Value) -> Result<String, CmdError> {
    let refused = |why: String| {
        CmdError::click(format!(
            "{why}, so there is no silent authority to hand a registry past; push it to the \
             store, without --last-known-good"
        ))
        .stating(FailureCode::Refused)
    };
    let kept = crate::cli::resolver::kept_last_good_document()
        .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))?;
    let copy_has_directory = kept
        .as_ref()
        .map(service_resolution::directory)
        .transpose()
        .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))?
        .flatten()
        .is_some();
    if copy_has_directory {
        return Err(refused(
            "this host is the registry authority of its last-known-good copy".to_string(),
        ));
    }
    let authority = service_resolution::directory(document)
        .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))?
        .ok_or_else(|| refused("the document declares no service directory".to_string()))?
        .authority
        .target;
    let this_host = crate::cli::resolver::current_target(document)
        .map_err(|cause| refused(format!("the document does not list this host: {cause}")))?;
    if authority == this_host {
        return Err(refused(format!(
            "the document names this host {this_host} its registry authority"
        )));
    }
    Ok(format!(
        "this host {this_host} kept no fleet registry, so it had no authority to ask; the \
         document names {authority}"
    ))
}

fn directory_generation(document: &Value) -> Result<u64, String> {
    Ok(service_resolution::directory(document)?
        .ok_or_else(|| "the document declares no service_directory".to_string())?
        .generation)
}
