//! The code identity the upgraded program will carry, whether it is the one
//! macOS keeps the host's privacy decisions under, and every dialog that
//! follows when it is not — said before anything is built.

use serde::Serialize;
use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::host_privacy::local_network::{decisions_on_host, LocalNetwork};
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use super::installed::Installed;

/// How far back the host's log is read for the Local Network decision macOS
/// last applied to the installed program: the window `stado host privacy
/// --since` documents with its own example, one day.
const LOCAL_NETWORK_WINDOW: &str = "1d";

/// The identity the signature will carry, and whether it changes the
/// installed one.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Identity {
    /// The code identifier: the installed program's when it carries a
    /// stable one, else the product's own.
    pub identifier: String,
    /// Absent for a host that is no Mac: nothing is signed for it.
    pub team: Option<stado_product::SigningTeam>,
    /// Whether macOS will see another program.
    pub change: bool,
    /// Why, in the words the report prints.
    pub why: String,
}

/// One question the person at a Mac will be asked, and the answer.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Dialog {
    pub host: String,
    pub when: String,
    pub says: String,
    pub answer: String,
    pub because: String,
}

/// The designated requirement the signer gives a program
/// (`designated => identifier ... and anchor apple generic and certificate
/// leaf[subject.OU] = TEAM`), spelled without quotes so `codesign`'s own
/// quoting of the two values does not tell equal requirements apart.
fn normalized(requirement: &str) -> String {
    requirement.replace('"', "")
}

/// The identity the upgrade signs with, read from the same source the
/// signature will come from.
pub(super) fn predict(
    choice: &stado_product::SigningChoice,
    installed: &Installed,
) -> Result<Identity, CmdError> {
    let product = env!("CARGO_PKG_NAME");
    let signature = &installed.program.signature;
    let identifier = match (&signature.state[..], &signature.identifier) {
        ("stable", Some(identifier)) => identifier.clone(),
        _ => stado_product::code_identifier(product, super::installed::INSTALLED)
            .map_err(|error| CmdError::declaration(format!("{error:#}")))?,
    };
    if !installed.darwin {
        return Ok(Identity {
            identifier,
            team: None,
            change: false,
            why: "the host is no Mac: no code identity is kept there, and nothing is signed"
                .to_string(),
        });
    }
    let team = choice.team().map_err(|error| {
        CmdError::declaration(format!(
            "the signing identity cannot be read before anything is built: {error:#}"
        ))
    })?;
    let expected = format!(
        "identifier {identifier} and anchor apple generic and certificate leaf[subject.OU] = {}",
        team.team
    );
    let why = match (&signature.state[..], signature.requirement.as_deref()) {
        ("stable", Some(requirement)) if normalized(requirement) == expected => {
            return Ok(Identity {
                why: format!(
                    "the installed program already carries {requirement}, the identity {} \
                     signs with",
                    team.identity
                ),
                identifier,
                team: Some(team),
                change: false,
            })
        }
        ("stable", Some(requirement)) => format!(
            "the installed program carries {requirement}; {} signs it as {expected}",
            team.identity
        ),
        (state, _) => format!(
            "the installed program's signature is {state}, which macOS keeps no lasting \
             identity for; {} signs it as {expected}",
            team.identity
        ),
    };
    Ok(Identity {
        identifier,
        team: Some(team),
        change: true,
        why,
    })
}

/// The folders the host's latest beacon measured as open to its installed
/// program, or why none were measured.
async fn measured_folders(target: &ComputeTarget) -> Result<Vec<(String, String)>, String> {
    let store = crate::cli::host::beacon_store()
        .await
        .map_err(|error| error.to_string())?;
    let report = crate::monitor::host_health::load_host_health(&store, &target.name)
        .await
        .map_err(|error| error.to_string())?;
    let Some(folders) = report
        .beacon
        .get("privacy")
        .and_then(|block| block.get("folders"))
        .and_then(Value::as_object)
    else {
        return Err("its latest beacon carries no privacy measurement".to_string());
    };
    let at = match report.beacon.get("reported_at").and_then(Value::as_str) {
        Some(at) => at.to_string(),
        None => "an unrecorded time".to_string(),
    };
    Ok(folders
        .iter()
        .filter(|(_, folder)| folder["state"] == "granted")
        .map(|(key, _)| (key.clone(), at.clone()))
        .collect())
}

/// Every dialog the upgrade will raise, on this Mac while it signs and on
/// the host after the install, with the answer that keeps the host working.
/// `notes` receives what could not be measured.
pub(super) async fn dialogs(
    target: &ComputeTarget,
    installed: &Installed,
    identity: &Identity,
    join_network: bool,
    runner: &Runner,
    notes: &mut Vec<String>,
) -> Result<Vec<Dialog>, CmdError> {
    let mut dialogs = Vec::new();
    if let Some(team) = identity.team.as_ref().filter(|team| team.operator_keychain) {
        dialogs.push(Dialog {
            host: "this Mac".to_string(),
            when: "while it signs".to_string(),
            says: format!(
                "codesign wants to access key \"{}\" in your keychain.",
                team.identity
            ),
            answer: "enter the keychain's password, then Always Allow".to_string(),
            because: format!("the key is read from {}", team.source),
        });
    }
    if !identity.change {
        return Ok(dialogs);
    }
    let program = &installed.program.path;
    let grants = target
        .privacy_grants()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let home = std::path::Path::new(&installed.home);
    let declared = |key: &str| {
        grants
            .iter()
            .find(|grant| {
                grant.folder == key && grant.program_on(home).to_string_lossy() == program.as_str()
            })
            .map(|grant| grant.reason.clone())
    };
    let measured = match measured_folders(target).await {
        Ok(measured) => measured,
        Err(why) => {
            notes.push(format!(
                "{}: the folders open to its Stado were not measured ({why}); only declared \
                 grants are listed",
                target.name
            ));
            Vec::new()
        }
    };
    for &(key, folder) in crate::targets::PRIVACY_FOLDERS {
        let because = match (
            declared(key),
            measured.iter().find(|(measured, _)| measured == key),
        ) {
            (Some(reason), _) => format!("targets[].privacy_grants declares it: {reason}"),
            (None, Some((_, at))) => {
                format!("the installed program holds this access now (beacon of {at})")
            }
            (None, None) => continue,
        };
        dialogs.push(Dialog {
            host: target.name.clone(),
            when: "the first time the new program opens the folder".to_string(),
            says: format!("\"stado\" would like to access files in your {folder} folder."),
            answer: "Allow".to_string(),
            because,
        });
    }
    let (key, _) = crate::targets::LOCAL_NETWORK;
    let decided = match decisions_on_host(
        target,
        std::slice::from_ref(program),
        LOCAL_NETWORK_WINDOW,
        runner,
    )
    .await
    {
        Ok(decisions) => decisions.into_iter().find(|decision| {
            matches!(
                decision.state,
                LocalNetwork::Granted | LocalNetwork::Pending
            )
        }),
        Err(error) => {
            notes.push(format!(
                "{}: macOS's last Local Network decision for {program} was not read: {error}",
                target.name
            ));
            None
        }
    };
    // A host's Stado dials other hosts' SSH paths and its own network's
    // peers, so the question comes whenever one of them is on the local
    // network; with no record of it, the dialog is still said, as one that
    // may come.
    let because = match (declared(key), decided, join_network) {
        (Some(reason), _, _) => format!("targets[].privacy_grants declares it: {reason}"),
        (None, Some(decision), _) => format!(
            "the installed program reached the local network, and macOS logged it {:?} at {}",
            decision.state,
            match decision.at {
                Some(at) => at,
                None => "an unlogged time".to_string(),
            }
        ),
        (None, None, true) => "--join-network starts Stado's network node, which reaches peers \
                               on the local network"
            .to_string(),
        (None, None, false) => format!(
            "macOS logged no Local Network decision for the installed program in the last \
             {LOCAL_NETWORK_WINDOW}; it asks the first time the new program reaches a device on \
             the local network, such as another host's SSH path there"
        ),
    };
    dialogs.push(Dialog {
        host: target.name.clone(),
        when: "when the new program first reaches the local network, and again when Stado's \
               network node first starts there"
            .to_string(),
        says: "\"stado\" would like to find and connect to devices on your local network."
            .to_string(),
        answer: "Allow (Don't Allow keeps every local-network path from this host refused)"
            .to_string(),
        because,
    });
    Ok(dialogs)
}
