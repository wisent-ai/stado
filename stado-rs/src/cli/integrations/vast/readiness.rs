//! `stado vast readiness` — whether this machine can actually earn on
//! Vast.ai, and when it cannot, which provisioning step is missing.
//!
//! Every other command in this group answered the same sentence for three
//! different states. On 2026-09-20 `stado vast status` said `Skarbiec item
//! stado-vast field api_key is required` under a `403 consumer not authorized
//! to read item field`, while the fleet vault on the host serving Skarbiec
//! declared no `stado-vast` item at all: the refusal named a grant that could
//! not exist, and the operator had no command that would say so.
//!
//! This one asks all three authorities in order — the Skarbiec channel this
//! host has, the vault that would hold the item, and Vast.ai itself — and
//! reports what each answered. A key that resolves is not the verdict:
//! `ready` means Vast.ai accepted it and named our machine.

use serde::Serialize;
use serde_json::Value;

use crate::cli::CmdError;
use crate::providers::vast::{
    read_vast_api_key, VastClient, VastCredentialChannel, VastCredentialReading,
};

/// The item the bridge reads, and the field on it.
const ITEM: &str = "stado-vast";
const FIELD: &str = "api_key";
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
        let vault = report.vault_host.as_deref().unwrap_or("the fleet vault");
        match self {
            Self::Ready => format!(
                "ready: Vast.ai accepts {ITEM}/{FIELD} and answers for machine {}",
                report.machine_id.as_deref().unwrap_or("-")
            ),
            Self::VastRefused => format!(
                "not earning: {ITEM}/{FIELD} resolved and Vast.ai refused it ({})",
                report.vast_error.as_deref().unwrap_or("no reason given")
            ),
            Self::ItemAbsent => {
                format!("not provisioned: the vault on {vault} declares no {ITEM} item")
            }
            Self::NotAuthorized => format!(
                "not authorized: {vault} holds {ITEM} and this consumer may not read {FIELD}"
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
    pub channel: VastCredentialChannel,
    pub skarbiec_error: Option<String>,
    pub vault_host: Option<String>,
    pub vault_item_state: Option<String>,
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
    if matches!(reading.channel, VastCredentialChannel::None { .. }) {
        report.verdict = Verdict::NoChannel;
        report.remedy = remedy(&report, &reading);
        return report;
    }
    if ask_vault {
        read_vault(&mut report, vault_host).await;
    } else {
        report.vault_error = Some("skipped by --no-vault-check".to_string());
    }
    report.verdict = match report.vault_item_state.as_deref() {
        Some("absent") => Verdict::ItemAbsent,
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

/// Whether the vault that would hold the item declares it. The host comes
/// from the service directory unless the operator named one.
async fn read_vault(report: &mut Readiness, vault_host: Option<String>) {
    let host = match vault_host {
        Some(host) => Some(host),
        None => match crate::cli::directory::active_host(VAULT_SERVICE).await {
            Ok(Some(host)) => Some(host),
            Ok(None) => {
                report.vault_error = Some(format!(
                    "the registry declares no active host for {VAULT_SERVICE}; \
                     name one with --vault-host"
                ));
                None
            }
            Err(error) => {
                report.vault_error =
                    Some(format!("the service directory could not be read: {}", {
                        error
                    }));
                None
            }
        },
    };
    let Some(host) = host else {
        return;
    };
    report.vault_host = Some(host.clone());
    match crate::cli::host::vault_item_state(&host, ITEM).await {
        Ok(state) => report.vault_item_state = Some(state),
        Err(error) => report.vault_error = Some(error.to_string()),
    }
}

/// The exact commands that close the gap this verdict names.
fn remedy(report: &Readiness, reading: &VastCredentialReading) -> Vec<String> {
    let vault = report.vault_host.as_deref().unwrap_or("<vault-host>");
    let consumer = match &reading.channel {
        VastCredentialChannel::ControlPlane { consumer, .. } => consumer.clone(),
        VastCredentialChannel::AgentGrant { consumer, .. } => consumer.clone(),
        VastCredentialChannel::None { .. } => "<consumer>".to_string(),
    };
    let grant = format!(
        "stado credentials grant item-read --host {vault} --field {FIELD} \
         --token-file <file> {consumer} {ITEM}"
    );
    match report.verdict {
        Verdict::Ready => Vec::new(),
        Verdict::VastRefused => vec![
            format!(
                "replace the {FIELD} field of {ITEM} on {vault} with a key from console.vast.ai"
            ),
            "stado vast readiness".to_string(),
        ],
        Verdict::ItemAbsent => vec![
            format!(
                "stado credentials item put --host {vault} --type api-key {ITEM}   \
                 (payload on stdin: an api-key document carrying {FIELD})"
            ),
            grant,
            "stado vast readiness".to_string(),
        ],
        Verdict::NotAuthorized => vec![grant, "stado vast readiness".to_string()],
        Verdict::NoChannel => vec![
            "run this on the host that carries the bridge, or install that host's \
             agent grant file"
                .to_string(),
        ],
        Verdict::Unknown => vec![
            format!("stado credentials item show --host {vault} {ITEM}"),
            "stado vast readiness --vault-host <host>".to_string(),
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
        super::echo_json(&serde_json::to_value(&report)?);
    } else {
        print_text(&report, &summary);
    }
    if report.earning_possible() {
        return Ok(());
    }
    if json {
        return Err(CmdError::silent(1));
    }
    Err(CmdError::click(summary).stating(crate::primitives::failure::FailureCode::Config))
}

fn print_text(report: &Readiness, summary: &str) {
    println!("{summary}");
    println!("item:     {}/{}", report.item, report.field);
    println!("channel:  {}", report.channel.describe());
    if let Some(error) = &report.skarbiec_error {
        println!("skarbiec: {error}");
    }
    if let Some(host) = &report.vault_host {
        let state = report.vault_item_state.as_deref().unwrap_or("-");
        println!("vault:    {host} says {state}");
    }
    if let Some(error) = &report.vault_error {
        println!("vault:    {error}");
    }
    if let Some(machine) = &report.machine_id {
        println!("machine:  {machine}");
    }
    if let Some(price) = report.listed_gpu_cost {
        println!("listed:   ${price}/h");
    }
    for line in &report.remedy {
        println!("next:     {line}");
    }
}
