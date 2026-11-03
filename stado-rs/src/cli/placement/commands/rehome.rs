//! `stado placement rehome` — rewrite another account's home in the file
//! states a profile keeps on the host it runs on.

use serde_json::{json, Value};

use crate::cli::placement::candidates::{parse_registry, target};
use crate::cli::placement::transfer::{discard_staging, rehome_states};
use crate::cli::{registry, CmdError};
use crate::deploy::production_runner;

/// Rehome `profile`'s file states on `host` from `from_home`. Refused for a
/// profile the registry does not hold, a host it does not run on, and a
/// `from_home` that is not an absolute path.
pub(super) async fn rehome(
    profile_name: &str,
    host: &str,
    from_home: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    if !from_home.starts_with('/') || from_home.trim_end_matches('/').is_empty() {
        return Err(CmdError::refused(format!(
            "--from-home {from_home:?} is not an absolute home directory; give the source \
             account's home as the move's source host reported it"
        )));
    }
    let document = registry::fetch_document().await?;
    let profile = crate::placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == profile_name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the registry holds no placement profile {profile_name:?}; `stado placement \
                 profile list` names the ones it holds"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let registry = parse_registry(&document)?;
    let placed = super::moves::placed_host(&registry, &profile).map_err(CmdError::refused)?;
    if placed != host {
        return Err(CmdError::refused(format!(
            "{profile_name} runs on {placed}, not {host}; its states are rehomed on the host it runs on"
        )));
    }
    let target = target(&registry, host)?;
    let transaction = format!("rehome-{}", uuid::Uuid::new_v4().simple());
    let outcome = rehome_states(target, &profile, from_home, &transaction, &production_runner()).await;
    discard_staging(&transaction)?;
    let done = outcome?;
    if json_output {
        let states: Vec<Value> = done.iter().map(|each| each.report()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"profile": profile_name, "host": host,
                "from_home": from_home, "states": states}))?
        );
    } else {
        for each in &done {
            println!(
                "{}\t{}\t{}\t{}",
                each.location,
                each.outcome,
                each.mentions,
                each.kept.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}
