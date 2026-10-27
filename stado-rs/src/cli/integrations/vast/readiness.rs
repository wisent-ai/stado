//! `stado market readiness --provider vast` — whether this machine can
//! actually earn on Vast.ai, and when it cannot, which provisioning step is
//! missing.
//!
//! It asks three authorities in order — the Skarbiec channel this host has,
//! the vault that would hold the item, and Vast.ai itself — and reports what
//! each answered, so a `403 consumer not authorized to read item field` is
//! told apart from a vault that declares no item at all. A key that resolves
//! is not the verdict: `ready` means Vast.ai accepted it and named our
//! machine.

use serde::Serialize;
use serde_json::Value;

use crate::cli::CmdError;
use crate::providers::host_credential::{HostCredentialChannel, HostCredentialReading};
use crate::providers::vast::{read_vast_api_key, VastClient, VAST_FIELD, VAST_ROLE};

/// The role whose item the bridge reads (the key is read by role, never by
/// item id), and the field on it.
const ITEM: &str = VAST_ROLE;
const FIELD: &str = VAST_FIELD;
/// The fleet service whose active host holds the vault this item lives in.
/// Asking the directory keeps "which vault" a fleet answer rather than a
/// constant that goes stale the day the vault moves.
const VAULT_SERVICE: &str = "skarbiec";

/// What the three authorities together say about earning on Vast.ai.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Verdict {
    /// Vast.ai accepted our key and named the machine.
    Ready,
    /// The key resolved and Vast.ai did not answer for our machine.
    VastRefused,
    /// The vault that would hold the credential declares no such item.
    ItemAbsent,
    /// The item exists and this consumer may not read the field.
    NotAuthorized,
    /// This host holds no Skarbiec bearer at all, so it cannot even ask.
    NoChannel,
    /// The key did not resolve and the vault could not be asked why.
    Unknown,
}

impl Verdict {
    /// The one line a text reader gets, and the sentence a failing exit
    /// carries.
    fn summary(self, report: &Readiness) -> String {
        // The vault is named by host once one was found; before that the
        // `vault_error` says why none was, and the sentence names the role.
        let vault = match report.vault_host.as_deref() {
            Some(host) => host,
            None => "the fleet vault",
        };
        match self {
            Self::Ready => match report.machine_id.as_deref() {
                Some(id) => format!("ready: Vast.ai accepts {ITEM}/{FIELD} and answers for machine {id}"),
                None => format!("ready: Vast.ai accepts {ITEM}/{FIELD} and named no machine"),
            },
            Self::VastRefused => format!(
                "not earning: {ITEM}/{FIELD} resolved and Vast.ai refused it ({})",
                report.vast_error.as_deref().unwrap_or("no reason given")
            ),
            Self::ItemAbsent => {
                format!("not provisioned: no item in the vault on {vault} plays role {ITEM}")
            }
            Self::NotAuthorized => format!(
                "not authorized: {vault} holds the item playing {ITEM} and this consumer may not read {FIELD}"
            ),
            Self::NoChannel => {
                "no Skarbiec channel on this host: it cannot ask for any credential".to_string()
            }
            Self::Unknown => format!(
                "unknown: {ITEM}/{FIELD} did not resolve and the vault could not be read ({})",
                report.vault_error.as_deref().unwrap_or("not asked")
            ),
        }
    }
}

/// One readiness reading, as printed and as Stado Desktop consumes it.
#[derive(Debug, Clone, Serialize)]
pub(super) struct Readiness {
    pub document: &'static str,
    pub verdict: Verdict,
    pub item: &'static str,
    pub field: &'static str,
    pub channel: HostCredentialChannel,
    pub skarbiec_error: Option<String>,
    pub vault_host: Option<String>,
    pub vault_item_state: Option<crate::cli::host::VaultItemState>,
    pub vault_error: Option<String>,
    pub machine_id: Option<String>,
    pub listed_gpu_cost: Option<f64>,
    pub vast_error: Option<String>,
    pub remedy: Vec<String>,
}

impl Readiness {
    /// Whether the fleet can list capacity from this host right now.
    pub(super) fn earning_possible(&self) -> bool {
        self.verdict == Verdict::Ready
    }
}

/// Ask every authority and assemble the verdict.
///
/// `vault_host` overrides the directory's answer; `ask_vault` is the escape
/// for a machine with no fleet channel, where the vault read would only time
/// out.
pub(super) async fn assess(vault_host: Option<String>, ask_vault: bool) -> Readiness {
    let reading = read_vast_api_key().await;
    let mut report = Readiness {
        document: "stado.vast-readiness.v1",
        verdict: Verdict::Unknown,
        item: ITEM,
        field: FIELD,
        channel: reading.channel.clone(),
        skarbiec_error: reading.error.clone(),
        vault_host: None,
        vault_item_state: None,
        vault_error: None,
        machine_id: None,
        listed_gpu_cost: None,
        vast_error: None,
        remedy: Vec::new(),
    };
    if let Some(key) = reading.key.clone() {
        probe_vast(&mut report, key).await;
        report.remedy = remedy(&report, &reading);
        return report;
    }
    if matches!(reading.channel, HostCredentialChannel::None { .. }) {
        report.verdict = Verdict::NoChannel;
        report.remedy = remedy(&report, &reading);
        return report;
    }
    if ask_vault {
        let vault = read_vault(ITEM, vault_host).await;
        report.vault_host = vault.host;
        report.vault_item_state = vault.state;
        report.vault_error = vault.error;
    } else {
        report.vault_error = Some("skipped by --no-vault-check".to_string());
    }
    report.verdict = match &report.vault_item_state {
        Some(state) if !state.held() => Verdict::ItemAbsent,
        Some(_) => Verdict::NotAuthorized,
        None => Verdict::Unknown,
    };
    report.remedy = remedy(&report, &reading);
    report
}

/// The real consumer check: a key is a claim until Vast.ai answers for our
/// machine with it.
async fn probe_vast(report: &mut Readiness, key: String) {
    match VastClient::new(key).machine_status().await {
        Ok(status) => {
            report.machine_id = status
                .get("id")
                .map(|id| id.to_string())
                .map(|id| id.trim_matches('"').to_string());
            report.listed_gpu_cost = status.get("listed_gpu_cost").and_then(Value::as_f64);
            report.verdict = Verdict::Ready;
        }
        Err(error) => {
            report.vast_error = Some(error.to_string());
            report.verdict = Verdict::VastRefused;
        }
    }
}

/// What the vault that would hold a role's item said about it.
#[derive(Debug, Clone, Default)]
pub(crate) struct VaultReading {
    pub host: Option<String>,
    pub state: Option<crate::cli::host::VaultItemState>,
    pub error: Option<String>,
}

/// Whether the vault that would hold the item playing `role` declares it.
/// The host comes from the service directory unless the operator named one.
pub(crate) async fn read_vault(role: &str, vault_host: Option<String>) -> VaultReading {
    let mut reading = VaultReading::default();
    let host = match vault_host {
        Some(host) => Some(host),
        None => match crate::cli::directory::active_host(VAULT_SERVICE).await {
            Ok(Some(host)) => Some(host),
            Ok(None) => {
                reading.error = Some(format!(
                    "the registry declares no active host for {VAULT_SERVICE}; \
                     name one with --vault-host"
                ));
                None
            }
            Err(error) => {
                reading.error = Some(format!("the service directory could not be read: {error}"));
                None
            }
        },
    };
    let Some(host) = host else {
        return reading;
    };
    reading.host = Some(host.clone());
    let holder = match crate::cli::host::vault_role_item(&host, role).await {
        Ok(holder) => holder,
        Err(error) => {
            reading.error = Some(error.to_string());
            return reading;
        }
    };
    let Some(holder) = holder else {
        reading.state = Some(crate::cli::host::VaultItemState::Absent);
        return reading;
    };
    match crate::cli::host::vault_item_state(&host, &holder).await {
        Ok(state) => reading.state = Some(state),
        Err(error) => reading.error = Some(error.to_string()),
    }
    reading
}

/// The exact commands that close the gap this verdict names.
fn remedy(report: &Readiness, reading: &HostCredentialReading) -> Vec<String> {
    // The commands name the vault host when one was found; otherwise the
    // operator fills the placeholder from `--vault-host`.
    let vault = match report.vault_host.as_deref() {
        Some(host) => host,
        None => "<vault-host>",
    };
    let consumer = reading.channel.consumer();
    let grant = format!(
        "stado credentials grant add --host {vault} --role {ITEM} --field {FIELD} \
         --token-file <file> {consumer}"
    );
    match report.verdict {
        Verdict::Ready => Vec::new(),
        Verdict::VastRefused => vec![
            format!(
                "replace the {FIELD} field of the item playing {ITEM} on {vault} with a key from console.vast.ai"
            ),
            "stado market readiness --provider vast".to_string(),
        ],
        Verdict::ItemAbsent => vec![
            format!(
                "stado credentials item put --host {vault} --type api-key {ITEM}   \
                 (payload on stdin: an api-key document carrying {FIELD})"
            ),
            grant,
            "stado market readiness --provider vast".to_string(),
        ],
        Verdict::NotAuthorized => vec![grant, "stado market readiness --provider vast".to_string()],
        Verdict::NoChannel => vec![
            "run this on the host that carries the bridge, or install that host's \
             agent grant file"
                .to_string(),
        ],
        Verdict::Unknown => vec![
            format!("stado credentials item show --host {vault} <the item tagged stado:role:{ITEM}>"),
            "stado market readiness --provider vast --vault-host <host>".to_string(),
        ],
    }
}

/// Print the report and exit non-zero unless the fleet can earn.
pub(super) async fn report(
    vault_host: Option<String>,
    no_vault_check: bool,
    json: bool,
) -> Result<(), CmdError> {
    let report = assess(vault_host, !no_vault_check).await;
    let summary = report.verdict.summary(&report);
    if json {
        crate::cli::print_answer(&serde_json::to_value(&report)?, true)?;
    } else {
        super::readiness_text::print_text(&report, &summary);
    }
    if report.earning_possible() {
        return Ok(());
    }
    if json {
        // The document already said everything; the exit is the one every
        // failed command uses.
        return Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE));
    }
    Err(CmdError::click(summary).stating(crate::primitives::failure::FailureCode::Config))
}
