//! `stado quota` command group — READ side (`show`, `catalog`,
//! `request list`) and WRITE side (`request create`, `ticket reply`,
//! `ticket escalate`).
//!
//! Port of the `quota` group in `stado/cli.py`.
//!
//! One component per seam: [`read`] is the provider quota reads `show` and
//! `catalog` print, [`submit`] is the write side that submits the increase
//! requests and answers the Azure support tickets, [`report`] is the
//! in-flight request report `request list` prints, and [`common`] holds the
//! flag parsing, the JSON echo and the truncation all three share. The
//! command surface — the subcommand-to-function dispatch clap feeds —
//! stays here.

mod common;
mod read;
mod report;
mod submit;

use crate::cli::entry::spec::spend::quota::{
    QuotaRequestCommands, QuotaTicketCommands, TicketProvider,
};

use super::{CmdError, QuotaCommands};

use read::{catalog, show};
use report::requests;
use submit::{azure_escalate, azure_replies, request, request_all};

/// Dispatch one `quota` subcommand; `None` is the bare `quota` group,
/// which Python redirects to `quota show` with the group-level --json.
#[allow(clippy::too_many_lines)]
pub(super) async fn dispatch(json: bool, sub: &Option<QuotaCommands>) -> Result<(), CmdError> {
    match sub {
        None => show(json).await,
        Some(QuotaCommands::Show { json: sub_json }) => show(json || *sub_json).await,
        Some(QuotaCommands::Catalog {
            provider,
            json: sub_json,
        }) => catalog(provider, *sub_json).await,
        Some(QuotaCommands::Request { command }) => match command {
            QuotaRequestCommands::Create {
                accel: Some(accel),
                every_family: false,
                new_limit,
                region,
                provider,
                justification,
                email,
                json: sub_json,
            } => {
                request(
                    accel,
                    *new_limit,
                    region,
                    provider,
                    justification,
                    email,
                    *sub_json,
                )
                .await
            }
            QuotaRequestCommands::Create {
                new_limit,
                region,
                provider,
                justification,
                email,
                json: sub_json,
                ..
            } => {
                request_all(
                    *new_limit,
                    provider,
                    region,
                    justification,
                    email,
                    *sub_json,
                )
                .await
            }
            QuotaRequestCommands::List {
                provider,
                state,
                awaiting_customer,
                json: sub_json,
            } => requests(provider, state, *awaiting_customer, *sub_json).await,
        },
        Some(QuotaCommands::Ticket { command }) => match command {
            QuotaTicketCommands::Reply {
                provider: TicketProvider::Azure,
                dry_run,
                email,
            } => azure_replies(*dry_run, email).await,
            QuotaTicketCommands::Escalate {
                provider: TicketProvider::Azure,
                dry_run,
                email,
            } => azure_escalate(*dry_run, email).await,
        },
    }
}
