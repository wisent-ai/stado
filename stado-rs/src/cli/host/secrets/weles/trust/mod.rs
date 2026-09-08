//! The SPIS Weles receipt-trust document.

pub(in crate::cli::host) mod judge;
pub(in crate::cli::host) mod render;

use crate::targets::ComputeTarget;

/// The schema both halves of the Spis/Weles bridge require of the public
/// receipt-trust document.
const SPIS_TRUST_SCHEMA: &str = "wisent.spis-weles-receipt-trust.v1";

/// The one browser action the Spis admission binding grants.
const SPIS_TRUST_ACTION: &str = "generic_browser_task";

/// Exactly the fields the document carries. A sixth would be refused by the
/// consumer's `deny_unknown_fields` deserializer, so it is refused here first.
const SPIS_TRUST_FIELDS: &[&str] = &[
    "schema",
    "organizationId",
    "allowedAction",
    "receiptKeys",
    "keySetVersion",
];

/// The managed Skarbiec units whose own environment names the vault the
/// daemon actually serves, at the system paths the fleet installs them.
///
/// Which file is live is a property of the running daemon, not a default: a
/// host carries several vault files and `skarbiec` without
/// `SKARBIEC_VAULT_FILE` picks one that may hold nothing. Reading the unit is
/// how that question gets answered against the host rather than against a
/// guess.
const SKARBIEC_UNIT_PLISTS: &[&str] = &[
    "/Library/LaunchDaemons/com.wisent.always-on.skarbiec.plist",
    "/Library/LaunchAgents/com.wisent.always-on.skarbiec.plist",
];

/// The environment the Skarbiec daemon on this host is actually started with.
///
/// The vault is only the half that decides WHICH secrets answer. Skarbiec
/// decrypts by spawning GnuPG, so `GNUPGHOME` decides WHETHER any of them do,
/// and a read that inherits the vault without the keyring fails on the first
/// field with GnuPG's own "No such file or directory". Both come from the same
/// unit, so both are taken from it rather than one being read and the other
/// assumed.
async fn live_skarbiec_environment(
    resolved: &ComputeTarget,
    home: &str,
    runner: &crate::deploy::Runner,
) -> Result<Vec<(String, String)>, String> {
    use crate::deploy::host_channel;

    let mut units: Vec<String> = SKARBIEC_UNIT_PLISTS
        .iter()
        .map(|path| (*path).to_string())
        .collect();
    units.push(format!(
        "{home}/Library/LaunchAgents/com.wisent.skarbiec.plist"
    ));

    let extract = |unit: &str, key: &'static str| {
        let unit = unit.to_string();
        async move {
            let read = host_channel::run_program(
                resolved,
                &[
                    "/usr/bin/plutil",
                    "-extract",
                    &format!("EnvironmentVariables.{key}"),
                    "raw",
                    "-o",
                    "-",
                    unit.as_str(),
                ],
                runner,
            )
            .await
            .map_err(|error| error.to_string())?;
            Ok::<Option<String>, String>(if read.ok() {
                let declared = read.stdout.trim();
                (!declared.is_empty()).then(|| declared.to_string())
            } else {
                None
            })
        }
    };

    for unit in &units {
        let present = host_channel::remote_test(
            resolved,
            &format!("-f {}", crate::deploy::shlex_quote(unit)),
            runner,
        )
        .await
        .map_err(|error| error.to_string())?;
        if !present {
            continue;
        }
        // The unit that names a vault is the one serving this host; a unit that
        // does not is not a Skarbiec this read may borrow an environment from.
        let Some(vault) = extract(unit, "SKARBIEC_VAULT_FILE").await? else {
            continue;
        };
        let mut environment = vec![("SKARBIEC_VAULT_FILE".to_string(), vault)];
        if let Some(keyring) = extract(unit, "GNUPGHOME").await? {
            environment.push(("GNUPGHOME".to_string(), keyring));
        }
        return Ok(environment);
    }
    Err(
        "no managed Skarbiec unit on this host declares SKARBIEC_VAULT_FILE, so the live vault \
         cannot be identified and a read would silently answer from the wrong file"
            .to_string(),
    )
}
