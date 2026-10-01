//! Cross-cloud spend and credit balances. The provider notices that arrive
//! by mail are read through Skrzynka by `billing watch`; Stado has no mail
//! client of its own.

use clap::Subcommand;

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
    /// One billing watchdog pass: refresh, evaluate credit balance AND
    /// account health, and alert on transitions. Deliberately runnable
    /// outside the cloud it monitors; whatever schedule runs it (`stado
    /// schedule create`, cron on another machine) is its cadence.
    Watch {
        #[arg(long)]
        json: bool,
    },
}
