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
    /// change run`. A commit that changes more lines than its product's
    /// catalog `submit.max_changed_lines` is refused here, to be split, and
    /// so is a high-risk one that names no `--defect`.
    Submit {
        /// The repository's checkout on this machine.
        checkout: String,
        /// The commit to land: one the checkout holds, or one origin gives
        /// by its id.
        #[arg(long)]
        commit: String,
        /// An Oko defect the commit repairs; repeat for several. The first
        /// scopes the harsh review, so a high-risk change must name one; each
        /// is recorded repaired when the change lands.
        #[arg(long = "defect")]
        defects: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run one pass of the checkout's repository's queue, or, without a
    /// checkout, of every repository with a candidate waiting for a pass,
    /// each from the checkout its candidate was submitted from (the resident
    /// pass the installation declares as a host routine): carry every
    /// queued candidate, and every running one whose pass is gone (its
    /// repository's pass lock is free), oldest first, to landed, refused or
    /// repaired (its review before landing repaired it, or recorded no round
    /// over what its reviewer left: what the review left is queued at the
    /// tail). Exits 0 when every candidate reached a verdict; a candidate
    /// the pass could not judge (the remote or the fleet did not answer,
    /// Oko's admission did not answer or did not admit its gates, the harsh
    /// review could not be run) stays queued with what it waits on, and the
    /// pass exits non-zero with the cause. A pass under no agent session
    /// asks Oko's admission first as the fleet's daily build, then as each
    /// candidate's submitter.
    Run {
        /// The repository's checkout on this machine; every repository with
        /// a waiting candidate without it.
        checkout: Option<String>,
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
        /// Only candidates in this state: queued, running, landed, refused,
        /// repaired or cancelled.
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a candidate: a queued one at once, a running one before its
    /// pass takes the next step. A landed, refused, repaired or cancelled
    /// candidate is refused.
    Cancel {
        /// The candidate's id, whole or a unique prefix.
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Record a review round over a queued or running candidate by the
    /// agent session this command runs in, as its harness names it (`tama
    /// sessions self`, else the transcript `PI_SESSION_FILE` names); the
    /// session that submitted the candidate is refused, and so is a command
    /// no session runs. The round is over the commit the next pass lands
    /// (the candidate written again on `main` by the last pass, else its own
    /// commit) and that commit's tree: its findings and improvements, each a
    /// file holding a JSON array. At the review before landing, each
    /// reviewer's newest round over the same tree counts: any finding refuses
    /// the candidate naming it, rounds that all found nothing admit it. Tama's
    /// harsh review runs only when no round over the tree is recorded.
    Review {
        /// The candidate's id, whole or a unique prefix.
        id: String,
        /// A file holding a JSON array of every finding, [] when none.
        #[arg(long)]
        findings: std::path::PathBuf,
        /// A file holding a JSON array of what the change suggests would
        /// further improve the codebase, [] when nothing.
        #[arg(long)]
        improvements: std::path::PathBuf,
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
            let integrated = integrated.map(std::path::PathBuf::from);
            let (carried, scope) = match checkout {
                Some(checkout) => {
                    let path = verbs::checkout(&checkout)
                        .await
                        .map_err(|error| error.machine_readable(json))?;
                    let carried = run::run(&path, &hosts, integrated.as_deref()).await;
                    (carried, path.display().to_string())
                }
                None => (
                    run::run_all(&hosts, integrated.as_deref()).await,
                    "any repository of this machine's queue".to_string(),
                ),
            };
            let carried = carried.map_err(|error| error.machine_readable(json))?;
            if carried.is_empty() && !json {
                println!("no candidate of {scope} waits for a pass; nothing to run");
                return Ok(());
            }
            answer(&carried, json)
        }
        ChangeCommands::Status { id, json } => {
            let candidate = record::find(&id).map_err(|error| error.machine_readable(json))?;
            if json {
                let mut value = serde_json::to_value(&candidate)?;
                value["waits"] = serde_json::json!(verbs::waits(&candidate));
                print_json(&value);
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
        ChangeCommands::Review {
            id,
            findings,
            improvements,
            json,
        } => {
            let (candidate, round) = verbs::review(&id, &findings, &improvements)
                .await
                .map_err(|error| error.machine_readable(json))?;
            if json {
                print_json(&serde_json::json!({
                    "candidate": candidate.id,
                    "round": serde_json::to_value(&round)?,
                }));
            } else {
                let decides = if round.findings.is_empty() {
                    "it found nothing, so the next pass admits that tree unless another \
                     reviewer's newest round over it found something"
                        .to_string()
                } else {
                    format!(
                        "it found {} finding(s), so the next pass refuses that tree naming them",
                        round.findings.len()
                    )
                };
                println!(
                    "recorded a review round by session {} over commit {} (tree {}) of \
                     candidate {}; {decides}",
                    round.reviewer_session, round.commit, round.tree, candidate.id
                );
            }
            Ok(())
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
