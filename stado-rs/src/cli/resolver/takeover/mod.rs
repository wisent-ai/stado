//! `stado placement move --authority --to-host HOST`, run on HOST: take the
//! registry authority role, from a declared authority that answers or one
//! that does not.
//!
//! The registry names its authority with `service_directory.authority`, and
//! the directory generation is the fence: a document naming HOST under the
//! next generation is the role moved.
//!
//! While the declared authority answers, the role is handed over through it:
//! that document is committed at the authority with the compare-and-swap
//! every registry writer uses, so the authority itself from then on serves
//! a registry naming HOST, and every resolver follows on its next refresh.
//! HOST then writes it into its own last-known-good copy and the store it
//! reads, from which it serves as the authority. A rerun after the commit
//! landed finds the authority already naming HOST and finishes the rest.
//!
//! While it does not answer, HOST promotes its own last-known-good registry
//! instead: that document becomes its copy, is written into the store it
//! reads, and is handed to every other registered host's copy with `stado
//! registry push - --last-known-good`; the old authority's later answers
//! carry a lower generation and are refused as a rollback. Either way every
//! step that did not land is named in the receipt and the command fails;
//! nothing is reported as done that was not.

mod hand;

use serde_json::{json, Value};

use crate::cli::resolver::directory::document::{authority_snapshot, kept_last_good_document};
use crate::cli::resolver::directory::source::current_target;
use crate::cli::{registry, CmdError};
use crate::primitives::failure::FailureCode;
use crate::service_resolution;
use crate::targets;

/// The store version the copy records for each way the role moved: no
/// store issued it yet.
const PROMOTED_VERSION: &str = "authority-takeover";
const HANDED_VERSION: &str = "authority-handover";

fn refused(sentence: String) -> CmdError {
    CmdError::click(sentence).stating(FailureCode::Refused)
}

fn config(sentence: String) -> CmdError {
    CmdError::click(sentence).stating(FailureCode::Config)
}

/// `document` naming `to_host` as the authority run with `command`, under the
/// next directory generation, validated as a write over `document`.
pub(crate) fn promote(document: &Value, to_host: &str, command: &str) -> Result<Value, CmdError> {
    let mut promoted = document.clone();
    let authority = promoted
        .pointer_mut("/service_directory/authority")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| config("service_directory.authority is not an object".to_string()))?;
    authority.insert("target".to_string(), json!(to_host));
    authority.insert("command".to_string(), json!(command));
    service_resolution::advance_generation(&mut promoted).map_err(config)?;
    let carried = targets::validate_registry_for_write(&promoted, Some(document))
        .map_err(|error| refused(format!("the promoted registry does not validate: {error}")))?;
    if let Some(detail) = carried {
        eprintln!(
            "stado placement move --authority: the promoted registry carries a section that was \
             already invalid in the registry it replaces, unchanged: {detail}"
        );
    }
    Ok(promoted)
}

pub(crate) async fn take_authority(
    to_host: &str,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let copy = kept_last_good_document().map_err(config)?.ok_or_else(|| {
        refused(
            "this host keeps no last-known-good registry to promote; the authority role is \
             taken on a host whose resolver has read the registry"
                .to_string(),
        )
    })?;
    let local = current_target(&copy).map_err(config)?;
    if local != to_host {
        return Err(refused(format!(
            "the authority role is taken on the host that takes it: run this on {to_host}, not {local}"
        )));
    }
    let directory = service_resolution::directory(&copy)
        .map_err(config)?
        .ok_or_else(|| {
            config("this host's registry copy declares no service_directory".to_string())
        })?;
    let old = directory.authority.target.clone();
    if old == to_host {
        return Err(refused(format!("{to_host} is already the registry authority")));
    }
    let command = &own_command()?;
    let answered = authority_snapshot().await;
    if check {
        let (how, unanswered) = match &answered {
            Ok(Some(_)) => ("handed-over", None),
            Ok(None) => {
                return Err(config(
                    "this host's copy names no authority to take the role from".to_string(),
                ))
            }
            Err(error) => ("taken-over", Some(error.to_string())),
        };
        // The document the move would write, validated as it would be.
        promote(&copy, to_host, command)?;
        let report = json!({
            "state": "would_take",
            "how": how,
            "authority": to_host,
            "fenced": old,
            "authority_unanswered": unanswered,
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "{to_host} can take the registry authority from {old}: it would be {how}; \
                 nothing was written"
            );
        }
        return Ok(());
    }
    match answered {
        Ok(Some(_)) => {
            let content = handed_over(to_host, command).await?;
            finish(&content, to_host, &old, None, json_output).await
        }
        Ok(None) => Err(config(
            "this host's copy names no authority to take the role from".to_string(),
        )),
        Err(error) => {
            let promoted = promote(&copy, to_host, command)?;
            let content = serde_json::to_string_pretty(&promoted)?;
            finish(&content, to_host, &old, Some(error.to_string()), json_output).await
        }
    }
}

/// Commit, at the authority that answers, the registry naming `to_host`;
/// the committed document. One the authority already serves naming
/// `to_host` is taken as it is, so a rerun finishes what a commit started.
async fn handed_over(to_host: &str, command: &str) -> Result<String, CmdError> {
    let committed = std::sync::Mutex::new(None::<Value>);
    registry::commit_document(|current| {
        let names = current
            .pointer("/service_directory/authority/target")
            .and_then(Value::as_str)
            == Some(to_host);
        let next = if names { current.clone() } else { promote(current, to_host, command)? };
        let mut slot = match committed.lock() {
            Ok(slot) => slot,
            Err(poisoned) => poisoned.into_inner(),
        };
        *slot = Some(next.clone());
        Ok(next)
    })
    .await?;
    let committed = match committed.into_inner() {
        Ok(committed) => committed,
        Err(poisoned) => poisoned.into_inner(),
    };
    let committed = committed.ok_or_else(|| {
        config("the registry commit returned without reading the registry".to_string())
    })?;
    Ok(serde_json::to_string_pretty(&committed)?)
}

/// This program's own path, which resolvers run as the authority's command.
pub(crate) fn own_command() -> Result<String, CmdError> {
    let program = std::env::current_exe().map_err(|error| {
        config(format!("this program's own path, which resolvers would run here, cannot be read: {error}"))
    })?;
    program
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| config(format!("this program's path {} is not UTF-8", program.display())))
}

/// What [`install`] did: the receipt, and why it did not land everywhere.
pub(crate) struct Installed {
    pub receipt: Value,
    pub failure: Option<CmdError>,
}

/// How a registry naming a new authority is installed: `to_host` takes the
/// role from `old`; `unanswered` says why it was taken without `old`, and
/// then every other host's copy is handed it; `store_at` names the object
/// API to write the store in, instead of the store this host reads.
pub(crate) struct Handover<'a> {
    pub to_host: &'a str,
    pub old: &'a str,
    pub unanswered: Option<String>,
    pub store_at: Option<&'a str>,
}

/// Write `content`, a registry naming the new authority, into this host's
/// copy, the store, and the other hosts' copies as `handover` says. The
/// receipt is kept in the placement history either way.
pub(crate) async fn install(content: &str, handover: Handover<'_>) -> Result<Installed, CmdError> {
    let Handover { to_host, old, unanswered, store_at } = handover;
    let taken_over = unanswered.is_some();
    let document: Value = serde_json::from_str(content)?;
    let generation = document
        .pointer("/service_directory/generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| config("the promoted registry names no directory generation".to_string()))?;
    let version = if taken_over { PROMOTED_VERSION } else { HANDED_VERSION };
    targets::store_last_good(content, version)
        .map_err(|refusal| config(format!("this host's last-known-good copy refused the promoted registry: {refusal}")))?;
    // The snapshot kept of the old authority answers for no read any more.
    crate::cli::resolver::forget_recalled_registry();
    let store = hand::stored(store_at, content).await;
    let registry = targets::load_registry_from_str(content).map_err(|error| config(error.to_string()))?;
    // After a takeover every other host is handed the registry at once: each
    // is its own channel. After a hand-over the old authority serves it to
    // every resolver, and a copy would refuse it while that authority answers.
    let others: Vec<_> = registry
        .targets
        .iter()
        .filter(|target| taken_over && target.name != to_host && target.name != old)
        .collect();
    let answers = futures::future::join_all(others.iter().map(|target| hand::handed(target, content))).await;
    let mut hosts = Vec::new();
    let mut not_handed = Vec::new();
    for (target, answer) in others.iter().zip(answers) {
        match answer {
            Ok(receipt) => hosts.push(json!({"host": target.name, "receipt": receipt})),
            Err(error) => {
                not_handed.push(target.name.clone());
                hosts.push(json!({"host": target.name, "error": error}));
            }
        }
    }
    let complete = store.is_ok() && not_handed.is_empty();
    let receipt = json!({
        "state": if complete { "taken" } else { "incomplete" },
        "how": if taken_over { "taken-over" } else { "handed-over" },
        "authority": to_host,
        "fenced": old,
        "directory_generation": generation,
        "authority_unanswered": unanswered,
        "store": match &store {
            Ok(version) => json!({"version": version}),
            Err(error) => json!({"error": error}),
        },
        "hosts": hosts,
    });
    crate::cli::placement::record_placement("authority", Ok(&receipt));
    let failure = (!complete).then(|| {
        let store_said = match &store {
            Ok(version) => format!("the store was written ({version})"),
            Err(error) => format!("the store was not written ({error})"),
        };
        CmdError::click(format!(
            "the authority role moved to {to_host}, but not everywhere: {store_said}; hosts not handed the registry: [{}]",
            not_handed.join(", ")
        ))
        .stating(FailureCode::InfraDown)
    });
    Ok(Installed { receipt, failure })
}

/// [`install`], then its receipt printed.
async fn finish(content: &str, to_host: &str, old: &str, unanswered: Option<String>, json_output: bool) -> Result<(), CmdError> {
    let handover = Handover { to_host, old, unanswered, store_at: None };
    let Installed { receipt, failure } = install(content, handover).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        use crate::cli::reporting::field;
        let how = if receipt["how"] == "taken-over" { "took the registry authority from" } else { "was handed the registry authority by" };
        println!("{to_host} {how} {old} at directory generation {}", field(&receipt["directory_generation"]));
        match receipt["store"]["error"].as_str() {
            Some(error) => println!("  store not written: {error}"),
            None => println!("  store written at version {}", field(&receipt["store"]["version"])),
        }
        for row in receipt["hosts"].as_array().into_iter().flatten() {
            match row.get("error") {
                Some(error) => println!("  {}: not handed the authority: {}", field(&row["host"]), field(error)),
                None => println!("  {}: handed the authority", field(&row["host"])),
            }
        }
    }
    match failure {
        Some(failure) => Err(failure),
        None => Ok(()),
    }
}
