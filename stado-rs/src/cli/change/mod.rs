//! `stado change`: the submit queue, one per repository.
//!
//! A candidate change is a commit an agent made on any `main` of a
//! repository. `submit` records it; `run` takes the repository's queued
//! candidates oldest first, one at a time, writes each again on the remote
//! `main`, runs the product's declared gates on the result across the
//! fleet's hosts, classifies its risk, has it reviewed before landing when
//! high and after when low, pushes it to `main` as a fast-forward and
//! records each defect it repairs. `status`, `list` and `cancel` read and
//! withdraw candidates. [`record`] keeps the queue, [`git`] does every Git
//! step without the checkout's working tree or index, [`run`] is the pass,
//! [`push`] judges a push to `main` the queue did not make, [`verbs`] the
//! rest.

pub mod git;
pub(crate) mod push;
pub mod record;
mod run;
pub mod verbs;

use clap::Subcommand;

use crate::cli::CmdError;

#[derive(Subcommand)]
pub(crate) enum ChangeCommands {
    /// Queue a commit to land on its repository's main: the commit is
    /// written again on the remote main, checked with the product's
    /// declared gates, reviewed as its risk requires and pushed by `stado
    /// change run`.
    Submit {
        /// The repository's checkout on this machine.
        checkout: String,
        /// The commit to land: one the checkout holds, or one origin gives
        /// by its id.
        #[arg(long)]
        commit: String,
        /// An Oko defect the commit repairs; repeat for several. The first
        /// scopes the harsh review, and each is recorded repaired when the
        /// change lands.
        #[arg(long = "defect")]
        defects: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run one pass of the checkout's repository's queue: carry every
    /// queued candidate, and every one a stopped pass left running, oldest
    /// first, to landed or refused. Exits 0 when every candidate reached a
    /// verdict; a candidate the pass could not judge (the remote or the
    /// fleet did not answer, Oko's admission did not answer or did not
    /// admit its gates, the harsh review could not be run) stays queued and
    /// the pass exits non-zero with the cause.
    Run {
        /// The repository's checkout on this machine.
        checkout: String,
        /// Run the compiling gates on these registry hosts, by name; repeat
        /// for several. Without it the hosts are chosen from their capacity
        /// publications, as a placed quality check chooses them.
        #[arg(long = "host")]
        hosts: Vec<String>,
        /// Append each landed defect's integration line to
        /// `<DIR>/<product>.jsonl`.
        #[arg(long)]
        integrated: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Read one candidate by its id or a unique prefix of it.
    Status {
        /// The candidate's id, whole or a unique prefix.
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// List the candidates of the checkout's repository, or of every
    /// repository without one, oldest first.
    List {
        /// A repository's checkout on this machine; every repository
        /// without it.
        checkout: Option<String>,
        /// Only candidates in this state: queued, running, landed, refused
        /// or cancelled.
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a candidate: a queued one at once, a running one before its
    /// pass takes the next step. A landed, refused or cancelled candidate
    /// is refused.
    Cancel {
        /// The candidate's id, whole or a unique prefix.
        id: String,
        #[arg(long)]
        json: bool,
    },
}

fn print_json(value: &serde_json::Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(value));
}

pub(crate) async fn dispatch(command: ChangeCommands) -> Result<(), CmdError> {
    match command {
        ChangeCommands::Submit {
            checkout,
            commit,
            defects,
            json,
        } => {
            let candidate = verbs::submit(&checkout, &commit, &defects)
                .await
                .map_err(|error| error.machine_readable(json))?;
            answer(&[candidate], json)
        }
        ChangeCommands::Run {
            checkout,
            hosts,
            integrated,
            json,
        } => {
            let path = verbs::checkout(&checkout)
                .await
                .map_err(|error| error.machine_readable(json))?;
            let integrated = integrated.map(std::path::PathBuf::from);
            let carried = run::run(&path, &hosts, integrated.as_deref())
                .await
                .map_err(|error| error.machine_readable(json))?;
            if carried.is_empty() && !json {
                println!(
                    "no candidate of {} is queued; nothing to run",
                    path.display()
                );
                return Ok(());
            }
            answer(&carried, json)
        }
        ChangeCommands::Status { id, json } => {
            let candidate = record::find(&id).map_err(|error| error.machine_readable(json))?;
            if json {
                print_json(&serde_json::to_value(&candidate)?);
            } else {
                verbs::print(&candidate);
            }
            Ok(())
        }
        ChangeCommands::List {
            checkout,
            state,
            json,
        } => {
            let listed = verbs::list(checkout.as_deref(), state.as_deref())
                .await
                .map_err(|error| error.machine_readable(json))?;
            if listed.is_empty() && !json {
                println!("no candidate of the submit queue matches");
                return Ok(());
            }
            answer(&listed, json)
        }
        ChangeCommands::Cancel { id, json } => {
            let candidate = verbs::cancel(&id).map_err(|error| error.machine_readable(json))?;
            answer(&[candidate], json)
        }
    }
}

fn answer(candidates: &[record::Candidate], json: bool) -> Result<(), CmdError> {
    if json {
        print_json(&verbs::json(candidates)?);
    } else {
        for candidate in candidates {
            println!("{}", verbs::line(candidate));
        }
    }
    Ok(())
}
