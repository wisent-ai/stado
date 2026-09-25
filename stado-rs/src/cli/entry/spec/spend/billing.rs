//! Cross-cloud spend and credit balances. The provider notices that arrive
//! by mail are read through Skrzynka by `billing watch`; Stado has no mail
//! client of its own.

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
