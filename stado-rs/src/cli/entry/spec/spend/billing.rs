//! Cross-cloud spend and credit balances, and the Gmail mailbox the
//! provider's side of those conversations arrives in.

use clap::Subcommand;

use crate::cli::billing;

#[derive(Subcommand)]
pub(crate) enum BillingCommands {
    /// Read the last billing snapshot published by the coordinator.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Query billing providers now and publish a fresh snapshot.
    Refresh {
        #[arg(long)]
        json: bool,
    },
    /// Foreground billing watchdog: poll, evaluate credit balance AND
    /// account health, and alert on transitions. Deliberately runnable
    /// outside the cloud it monitors (see `cli/billing.rs` module docs).
    Watch {
        /// Poll interval as a duration string: 45s, 5m, 2h, 1d.
        #[arg(long, default_value = "5m", value_parser = billing::parse_interval)]
        interval: std::time::Duration,
        /// Evaluate once and exit instead of looping.
        #[arg(long)]
        once: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum MailCommands {
    /// Search Gmail and list categorized message metadata.
    Search {
        /// Gmail search expression, for example: from:microsoft.com azure.
        #[arg(long, default_value = "")]
        query: String,
        /// Maximum messages to read.
        #[arg(long, default_value_t = default_mail_results())]
        max_results: usize,
        #[arg(long)]
        json: bool,
    },
    /// Aggregate categories, financial amounts, dates, links, and required actions.
    Analyze {
        /// Gmail search expression.
        #[arg(long, default_value = "")]
        query: String,
        /// Maximum messages to read.
        #[arg(long, default_value_t = default_mail_results())]
        max_results: usize,
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn default_mail_results() -> usize {
    usize::try_from(u8::BITS).expect("u8 bit width fits usize")
}
