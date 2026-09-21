//! The `stado delivery` command surface and its dispatch.

use clap::Subcommand;

use crate::cli::delivery::qualify::qualify;
use crate::cli::delivery::records::{deliver, reported};
use crate::cli::delivery::report::{failures, pending, status};
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum DeliveryCommands {
    /// Record a pushed revision as waiting for proof. Costs no build.
    Deliver {
        /// The product: the build recipe that knows how to build and test it.
        #[arg(long)]
        product: String,
        /// The exact full commit that was pushed.
        #[arg(long)]
        revision: String,
        /// One line saying what was delivered.
        #[arg(long)]
        summary: Option<String>,
        /// The task in Oko's register this revision answers. A failed pass
        /// reopens exactly this task.
        #[arg(long)]
        task: Option<String>,
        /// The session that delivered it.
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the deliveries nobody has proven yet.
    Pending {
        #[arg(long)]
        product: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Build one head and run the product's tests once for every delivery
    /// waiting on it. This is the step that spends a build.
    Qualify {
        #[arg(long)]
        product: String,
        /// Caller-retained token; reuse it to recover the same durable pass.
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Show the qualification passes and the verdicts they wrote.
    Status {
        #[arg(long)]
        product: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List failed deliveries: what has to go back to the task that made it.
    Failures {
        /// Only the ones nobody has handed back yet.
        #[arg(long)]
        unreported: bool,
        #[arg(long)]
        json: bool,
    },
    /// Record that a failed delivery has been handed back to its task, so one
    /// failure reopens one task once.
    Reported {
        /// The delivery identifier, as `failures` prints it.
        id: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: DeliveryCommands) -> Result<(), CmdError> {
    match command {
        DeliveryCommands::Deliver {
            product,
            revision,
            summary,
            task,
            session,
            json,
        } => deliver(&product, &revision, summary, task, session, json).await,
        DeliveryCommands::Pending { product, json } => pending(product, json).await,
        DeliveryCommands::Qualify {
            product,
            run_id,
            json,
        } => qualify(&product, &run_id, json).await,
        DeliveryCommands::Status { product, json } => status(product, json).await,
        DeliveryCommands::Failures { unreported, json } => failures(unreported, json).await,
        DeliveryCommands::Reported { id, json } => reported(&id, json).await,
    }
}
