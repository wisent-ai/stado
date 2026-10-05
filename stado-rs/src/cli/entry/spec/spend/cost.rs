//! Per-job and per-batch cost reporting, and what it projects.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum CostCommands {
    /// Read the recorded provider price book, including source errors and observation times.
    Prices {
        /// Print every quote and source record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Match recorded allocations to observed inventory and provider price quotes.
    Quote {
        /// Exact job IDs. Each result retains its own quote or refusal.
        #[arg(required = true)]
        job_ids: Vec<String>,
        /// Include complete allocation and source observations as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Summarize $ spent per target_kind and per model from completed jobs.
    Report {
        /// Print the report, with every completed job's row, as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Project total $ for a batch file using observed per-job cost.
    Estimate {
        batch_file: String,
        /// Print the projection as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show the attributed provider/owner/workload cost ledger.
    Allocation {
        #[arg(long)]
        json: bool,
    },
    /// Show current burn, month-end projection, budget, and credit runway.
    Forecast {
        #[arg(long)]
        json: bool,
    },
    /// Show active cost and resource anomalies.
    Anomalies {
        #[arg(long)]
        json: bool,
    },
    /// Show predicted versus realized savings.
    Savings {
        #[arg(long)]
        json: bool,
    },
}
