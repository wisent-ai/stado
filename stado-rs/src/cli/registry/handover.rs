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
    let unanswered =
        match crate::cli::resolver::authority_snapshot().await {
            Ok(Some(_)) => {
                return Err(CmdError::click(
                    "the registry authority answers; a registry it would serve is pushed through \
                 it, without --last-known-good",
                )
                .stating(FailureCode::Refused))
            }
            Ok(None) => return Err(CmdError::click(
                "this host is the registry authority, or keeps no copy naming one, so there is \
                 no silent authority to hand a registry past; push it to the store, without \
                 --last-known-good",
            )
            .stating(FailureCode::Refused)),
            Err(error) => error.to_string(),
        };
    let handed = directory_generation(document).map_err(|cause| {
        CmdError::click(format!("{source}: {cause}")).stating(FailureCode::Config)
    })?;
    let kept = kept_last_good_document()
        .map_err(|cause| CmdError::click(cause).stating(FailureCode::Config))?;
    if let Some(copy) = &kept {
        let held = directory_generation(copy).map_err(|cause| {
            CmdError::click(format!("this host's last-known-good copy: {cause}"))
                .stating(FailureCode::Config)
        })?;
        if handed <= held {
            return Err(CmdError::click(format!(
                "{source} carries service directory generation {handed}, not above the {held} \
                 this host's last-known-good copy holds; a copy is never rolled back"
            ))
            .stating(FailureCode::Refused));
        }
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

fn directory_generation(document: &Value) -> Result<u64, String> {
    Ok(service_resolution::directory(document)?
        .ok_or_else(|| "the document declares no service_directory".to_string())?
        .generation)
}
