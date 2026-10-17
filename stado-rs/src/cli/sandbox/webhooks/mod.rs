//! `stado sandbox webhook`: the HTTPS endpoints Boat posts sandbox lifecycle
//! events to, for the whole account the key belongs to. An agent factory
//! learns that a sandbox is ready, failed, archived, hydrated, degraded or
//! recovered without polling. `create` and `rotate` print the signing secret
//! the endpoint verifies deliveries with; Boat never shows it again.

use clap::{Args, Subcommand, ValueEnum};
use serde_json::Value;

use crate::providers::BoxProvider;

use super::agent::stated;
use super::verbs::{failure, print};
use super::CmdError;

/// A sandbox lifecycle event Boat delivers; each value is Boat's own name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WebhookEvent {
    #[value(name = "sandbox.ready")]
    Ready,
    #[value(name = "sandbox.error")]
    Error,
    #[value(name = "sandbox.archived")]
    Archived,
    #[value(name = "sandbox.hydrated")]
    Hydrated,
    #[value(name = "sandbox.degraded")]
    Degraded,
    #[value(name = "sandbox.recovered")]
    Recovered,
}

#[derive(Subcommand)]
pub enum WebhookCommands {
    /// List the account's endpoints, without their signing secrets.
    List,
    /// Register an endpoint and print its signing secret, shown only now.
    Create(WebhookCreateArgs),
    /// Show one endpoint, without its signing secret.
    Show(WebhookId),
    /// Change an endpoint's name, URL or events; its secret stays.
    Update(WebhookUpdateArgs),
    /// Delete an endpoint and its queued deliveries.
    Delete(WebhookId),
    /// Replace an endpoint's signing secret and print the new one.
    Rotate(WebhookId),
}

#[derive(Args, Debug)]
pub struct WebhookId {
    /// The webhook id Boat answered at create or list.
    pub webhook: String,
}

#[derive(Args, Debug)]
pub struct WebhookCreateArgs {
    /// The public HTTPS endpoint, on port 443, Boat posts events to.
    pub url: String,
    /// An event to deliver; repeat it or separate events with commas.
    #[arg(long = "event", value_enum, value_delimiter = ',', required = true)]
    pub events: Vec<WebhookEvent>,
    /// A name for the endpoint; without it the request names none.
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Args, Debug)]
pub struct WebhookUpdateArgs {
    /// The webhook id Boat answered at create or list.
    pub webhook: String,
    /// The new endpoint URL.
    #[arg(long)]
    pub url: Option<String>,
    /// The events to deliver from now on, replacing the ones it had.
    #[arg(long = "event", value_enum, value_delimiter = ',')]
    pub events: Vec<WebhookEvent>,
    /// The new name.
    #[arg(long)]
    pub name: Option<String>,
}

/// Boat's names of `events`, each the word its value enum declares.
fn names(events: &[WebhookEvent]) -> Result<Vec<String>, CmdError> {
    events
        .iter()
        .map(|event| match event.to_possible_value() {
            Some(value) => Ok(value.get_name().to_string()),
            None => Err(CmdError::usage(format!(
                "--event {event:?} names no event Boat delivers"
            ))),
        })
        .collect()
}

/// An optional option that, when given, must hold a value.
fn given<'a>(value: Option<&'a str>, option: &str) -> Result<Option<&'a str>, CmdError> {
    match value {
        None => Ok(None),
        Some(_) => stated(value, option).map(Some),
    }
}

pub(super) async fn dispatch(
    provider: &BoxProvider,
    command: WebhookCommands,
) -> Result<(), CmdError> {
    let client = &provider.client;
    let answer = match command {
        WebhookCommands::List => client.webhooks().await,
        WebhookCommands::Create(args) => {
            let events = names(&args.events)?;
            let events: Vec<&str> = events.iter().map(String::as_str).collect();
            let name = given(args.name.as_deref(), "--name")?;
            client.create_webhook(name, &args.url, &events).await
        }
        WebhookCommands::Show(id) => client.webhook(&id.webhook).await,
        WebhookCommands::Update(args) => {
            let events = names(&args.events)?;
            let events: Vec<&str> = events.iter().map(String::as_str).collect();
            let name = given(args.name.as_deref(), "--name")?;
            let url = given(args.url.as_deref(), "--url")?;
            client
                .update_webhook(&args.webhook, name, url, &events)
                .await
        }
        WebhookCommands::Delete(id) => client.delete_webhook(&id.webhook).await,
        WebhookCommands::Rotate(id) => client.rotate_webhook(&id.webhook).await,
    }
    .map_err(failure)?;
    print(&Value::Object(answer))
}
