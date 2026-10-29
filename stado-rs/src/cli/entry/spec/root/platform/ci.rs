//! `stado ci`: the forge's continuous integration and the delivery routine.

use std::num::NonZeroU64;

use clap::Subcommand;

/// `stado ci`: the forge's continuous integration, read for a catalogued
/// product, and the delivery routine that installs the declared plan.
#[derive(Subcommand)]
pub(crate) enum CiCommands {
    /// The newest run of every workflow the product's repository declares,
    /// with its conclusion and, for a run that did not pass, each failed job
    /// and the step that failed in it, and the delivery routine: its plan,
    /// the step it is installing and for how long, the refused step with
    /// every refusal and its defects, and what each step last delivered.
    /// Reads the forge's API as the identity Stado's GitHub declaration names
    /// in Skarbiec; refuses by name when Skarbiec, the declaration or the
    /// forge does not answer.
    Status {
        /// A product of the catalog (`stado product catalog` lists them).
        product: String,
        /// Read the newest run of this full commit instead of the newest run
        /// of any revision.
        #[arg(long)]
        revision: Option<String>,
        /// Print the document instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// One pass of the delivery routine, which every `stado serve` of a host
    /// that declares a delivery plan fires once the plan's cadence (`--every`)
    /// has passed since the last pass began: check every new commit of each
    /// step's `main` with the product's declared gates, recording each green
    /// or red (a red one in Oko, found in that commit), then install each
    /// step's last green commit, never `main` HEAD, in order, then — with
    /// `--push` declared — push `main` of every checkout. A refused install
    /// stops the pass, is recorded in Oko, exits non-zero, and resumes once a
    /// newer commit is green.
    Run {
        /// Print the routine's state after the pass.
        #[arg(long)]
        json: bool,
    },
    /// Whether the path that delivers repairs stands on this machine: the
    /// routine's running and refused steps, each step's main check (every
    /// commit checked, its last green one), who holds the pass lock, which
    /// product operations hold build trees, and this machine's Stado unit by
    /// its state alone: launchd's event log is not read, and the unit's
    /// `events` says so. `stalled` names every way the path is down, a red
    /// `main` among them, one sentence each; Oko orders work by it. Reads only.
    Delivery {
        /// Print the document instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// The delivery routine's declared plan.
    #[command(subcommand)]
    Plan(CiPlanCommands),
}

/// `stado ci plan`: the products the delivery routine installs, in order.
#[derive(Subcommand)]
pub(crate) enum CiPlanCommands {
    /// Declare the plan: each `--step PRODUCT:SURFACE:CHECKOUT`, in the
    /// order the routine installs them, the checkout named absolutely, the
    /// open Oko defect the installs repair, and how often a pass runs.
    /// Refused, quoting the ruling, while the rulings forbid building or
    /// installing for that purpose.
    Set {
        /// The open Oko defect the routine installs to repair; every step asks
        /// `oko admit build|install --product P --purpose DEFECT`.
        #[arg(long, value_name = "DEFECT")]
        purpose: String,
        #[arg(
            long = "step",
            value_name = "PRODUCT:SURFACE:CHECKOUT",
            required = true
        )]
        steps: Vec<String>,
        /// After every step is installed from its checkout's last green
        /// commit of `main`, push `main` of every checkout through the push
        /// gates just installed. Without it a pass pushes nothing upstream.
        #[arg(long)]
        push: bool,
        /// The seconds from the start of one pass to the start of the next,
        /// as the person declaring the plan states them: every `stado serve`
        /// that reconciles releases or coordinates on this host fires a pass
        /// once this much time has passed since the last pass began.
        #[arg(long, value_name = "SECONDS")]
        every: NonZeroU64,
        #[arg(long)]
        json: bool,
    },
    /// Print the declared plan and when its next pass is due; refused when
    /// none is declared.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Remove the declared plan: the routine installs nothing until one is
    /// declared again.
    Clear,
}
