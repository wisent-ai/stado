//! `stado market readiness --provider stado --peer NAME`: whether this host
//! can lend to the peer, and when it cannot, which step is missing.
//!
//! Three authorities, in order: the Skarbiec channel this host holds, the
//! vault that would hold the item playing the peer's credential role, and
//! the peer's own store, asked with the bearer for one name of its queue. A
//! bearer that resolves is not the verdict: `ready` means the peer's store
//! answered with it.

use serde::Serialize;
use serde_json::Value;

use crate::cli::integrations::vast::read_vault;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::providers::host_credential::{read_role_field, HostCredentialChannel};
use crate::providers::peer::{declaration, remote, TOKEN_FIELD};

use super::{cmd, our_store};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    /// The peer's store answered with our bearer.
    Ready,
    /// The bearer resolved and the peer's store did not answer with it.
    PeerRefused,
    /// The vault declares no item playing the peer's role.
    ItemAbsent,
    /// The item exists and this consumer may not read its token.
    NotAuthorized,
    /// This host holds no Skarbiec bearer at all.
    NoChannel,
    /// The bearer did not resolve and the vault could not be asked why.
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
struct Readiness {
    document: &'static str,
    peer: String,
    verdict: Verdict,
    summary: String,
    role: String,
    field: &'static str,
    channel: HostCredentialChannel,
    skarbiec_error: Option<String>,
    vault_host: Option<String>,
    vault_item_state: Option<crate::cli::host::VaultItemState>,
    vault_error: Option<String>,
    peer_origin: String,
    peer_namespace: String,
    peer_queue: Option<Value>,
    peer_error: Option<String>,
    remedy: Vec<String>,
}

/// Print the reading and exit non-zero unless this host can lend.
pub(super) async fn report(
    peer: &str,
    vault_host: Option<String>,
    ask_vault: bool,
    json: bool,
) -> Result<(), CmdError> {
    let store = our_store().await?;
    let declared = declaration::require(&store, peer).await.map_err(cmd)?;
    let reading = read_role_field(&declared.credential_role, TOKEN_FIELD).await;
    let mut report = Readiness {
        document: "stado.peer-readiness.v1",
        peer: peer.to_string(),
        verdict: Verdict::Unknown,
        summary: String::new(),
        role: declared.credential_role.clone(),
        field: TOKEN_FIELD,
        channel: reading.channel.clone(),
        skarbiec_error: reading.error.clone(),
        vault_host: None,
        vault_item_state: None,
        vault_error: None,
        peer_origin: declared.origin.clone(),
        peer_namespace: declared.namespace.clone(),
        peer_queue: None,
        peer_error: None,
        remedy: Vec::new(),
    };
    if reading.key.is_some() {
        let answered = match remote::open(&declared).await {
            Ok(peer_store) => remote::queue_state(&peer_store).await,
            Err(error) => Err(error),
        };
        match answered {
            Ok(queue) => {
                report.peer_queue = Some(queue);
                report.verdict = Verdict::Ready;
            }
            Err(error) => {
                report.peer_error = Some(error.to_string());
                report.verdict = Verdict::PeerRefused;
            }
        }
    } else if matches!(reading.channel, HostCredentialChannel::None { .. }) {
        report.verdict = Verdict::NoChannel;
    } else if ask_vault {
        let vault = read_vault(&declared.credential_role, vault_host).await;
        report.vault_host = vault.host;
        report.vault_item_state = vault.state;
        report.vault_error = vault.error;
        report.verdict = match &report.vault_item_state {
            Some(state) if !state.held() => Verdict::ItemAbsent,
            Some(_) => Verdict::NotAuthorized,
            None => Verdict::Unknown,
        };
    } else {
        report.vault_error = Some("skipped by --no-vault-check".to_string());
    }
    report.summary = summary(&report);
    report.remedy = remedy(&report);
    if json {
        crate::cli::print_answer(&serde_json::to_value(&report)?, true)?;
    } else {
        print_text(&report);
    }
    if report.verdict == Verdict::Ready {
        return Ok(());
    }
    if json {
        return Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE));
    }
    Err(CmdError::click(report.summary.clone()).stating(FailureCode::Config))
}

fn vault_name(report: &Readiness) -> String {
    match &report.vault_host {
        Some(host) => host.clone(),
        None => "the fleet vault".to_string(),
    }
}

fn summary(report: &Readiness) -> String {
    let role = &report.role;
    match report.verdict {
        Verdict::Ready => format!(
            "ready: the store of {} at {} answers with the bearer in {role}/{TOKEN_FIELD}",
            report.peer, report.peer_origin
        ),
        Verdict::PeerRefused => format!(
            "not lending: {role}/{TOKEN_FIELD} resolved and the store of {} did not answer with \
             it ({})",
            report.peer,
            report.peer_error.as_deref().unwrap_or("no reason given")
        ),
        Verdict::ItemAbsent => format!(
            "not provisioned: no item in the vault on {} plays role {role}",
            vault_name(report)
        ),
        Verdict::NotAuthorized => format!(
            "not authorized: {} holds the item playing {role} and this consumer may not read \
             {TOKEN_FIELD}",
            vault_name(report)
        ),
        Verdict::NoChannel => {
            "no Skarbiec channel on this host: it cannot ask for any credential".to_string()
        }
        Verdict::Unknown => format!(
            "unknown: {role}/{TOKEN_FIELD} did not resolve and the vault could not be read ({})",
            report.vault_error.as_deref().unwrap_or("not asked")
        ),
    }
}

fn remedy(report: &Readiness) -> Vec<String> {
    let vault = match &report.vault_host {
        Some(host) => host.clone(),
        None => "<vault-host>".to_string(),
    };
    let role = &report.role;
    let peer = &report.peer;
    let again = format!("stado market readiness --provider stado --peer {peer}");
    let grant = format!(
        "stado credentials grant add --host {vault} --role {role} --field {TOKEN_FIELD} \
         --token-file <file> {}",
        report.channel.consumer()
    );
    match report.verdict {
        Verdict::Ready => Vec::new(),
        Verdict::PeerRefused => vec![
            format!(
                "ask the operator of {peer} for a bearer of namespace {} on {}, and store it \
                 with `stado credentials item put --host {vault} --role {role} --type token` \
                 (the bearer on stdin)",
                report.peer_namespace, report.peer_origin
            ),
            format!("stado market peer edit {peer} --origin URL   (when the origin moved)"),
            again,
        ],
        Verdict::ItemAbsent => vec![
            format!(
                "stado credentials item put --host {vault} --role {role} --type token   \
                 (the bearer {peer} issued, on stdin)"
            ),
            grant,
            again,
        ],
        Verdict::NotAuthorized => vec![grant, again],
        Verdict::NoChannel => vec![
            "run this on the host that lends, or install that host's agent grant file".to_string(),
        ],
        Verdict::Unknown => vec![format!("{again} --vault-host <host>")],
    }
}

fn print_text(report: &Readiness) {
    println!("{}", report.summary);
    println!("item:     {}/{}", report.role, report.field);
    println!("channel:  {}", report.channel.describe());
    if let Some(error) = &report.skarbiec_error {
        println!("skarbiec: {error}");
    }
    if let Some(host) = &report.vault_host {
        match &report.vault_item_state {
            Some(state) => println!("vault:    {host} says {state}"),
            None => println!("vault:    {host} said nothing about the item"),
        }
    }
    if let Some(error) = &report.vault_error {
        println!("vault:    {error}");
    }
    println!(
        "peer:     {} namespace {}",
        report.peer_origin, report.peer_namespace
    );
    if let Some(queue) = &report.peer_queue {
        println!("queue:    {queue}");
    }
    if let Some(error) = &report.peer_error {
        println!("refused:  {error}");
    }
    for line in &report.remedy {
        println!("next:     {line}");
    }
}
