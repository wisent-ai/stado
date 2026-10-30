//! The verbs that read and change the queue without running it: `submit`
//! records a candidate, `status` and `list` read candidates, `cancel`
//! withdraws one. Each answers in text, or as JSON with `--json`, from the
//! same record.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::change::git;
use crate::cli::change::record::{self, Candidate, State, WaitsOn};
use crate::cli::CmdError;

/// The absolute top of the Git work tree holding the directory `path` names.
pub async fn checkout(path: &str) -> Result<PathBuf, CmdError> {
    let absolute = std::path::absolute(path)
        .map_err(|error| CmdError::usage(format!("cannot resolve the checkout {path}: {error}")))?;
    if !absolute.is_dir() {
        return Err(CmdError::usage(format!(
            "{} is not a directory; name the repository's checkout",
            absolute.display()
        )));
    }
    let top = git::git(&absolute, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|error| {
            CmdError::usage(format!(
                "{} is not a Git checkout: {error}",
                absolute.display()
            ))
        })?;
    Ok(PathBuf::from(top))
}

/// Record `commit` of `repository` (checkout `checkout`) as a queued
/// candidate repairing `defects`, submitted by agent session `session`, and
/// made by the review of candidate `follow_up_of` when it was.
pub fn enqueue(
    checkout: &Path,
    repository: &str,
    commit: &str,
    defects: &[String],
    (session, follow_up_of): (Option<String>, Option<String>),
) -> Result<Candidate, CmdError> {
    if let Some(open) = record::list(Some(repository))?
        .into_iter()
        .find(|candidate| {
            candidate.commit == commit && matches!(candidate.state, State::Queued | State::Running)
        })
    {
        return Err(CmdError::refused(format!(
            "{commit} is already {} as candidate {} of {repository}; `stado change status {}` \
             follows it",
            open.state.word(),
            open.id,
            open.id
        )));
    }
    let now = Utc::now();
    let mut candidate = Candidate {
        id: format!(
            "{}-{}",
            now.format("%Y%m%dT%H%M%S%3f"),
            &commit[..12.min(commit.len())]
        ),
        repository: repository.to_string(),
        checkout: checkout.to_path_buf(),
        commit: commit.to_string(),
        defects: defects.to_vec(),
        session,
        follow_up_of,
        state: State::Queued,
        submitted_at: now,
        updated_at: now,
        cancel_requested: false,
        product: None,
        rebased: None,
        risk: None,
        shards: Vec::new(),
        reviews: Vec::new(),
        landing: None,
        landed: None,
        refusal: None,
        waits_on: None,
        pass: None,
        repairs: None,
        integrated: Vec::new(),
        unrecorded: Vec::new(),
    };
    record::save(&mut candidate)?;
    Ok(candidate)
}

/// `stado change submit CHECKOUT --commit SHA [--defect ID]...`.
pub async fn submit(path: &str, commit: &str, defects: &[String]) -> Result<Candidate, CmdError> {
    if let Some(defect) = defects.iter().find(|defect| defect.trim().is_empty()) {
        return Err(CmdError::usage(format!(
            "--defect {defect:?} names no defect"
        )));
    }
    let checkout = checkout(path).await?;
    let repository = git::repository(&checkout).await?;
    let commit = git::commit(&checkout, commit).await?;
    let tip = git::fetch_main(&checkout).await?;
    if git::contains(&checkout, &commit, &tip).await? {
        return Err(CmdError::refused(format!(
            "{commit} is already on {repository}'s main ({tip}); there is nothing to land"
        )));
    }
    // A change too big to review whole is split, and a high-risk one names
    // its defect, before it takes a place.
    super::run::admit(&checkout, &commit, &tip, defects).await?;
    // Its gates are the submitting session's work, so Oko is later asked
    // about that session, whichever process runs the queue.
    let session = crate::cli::release_loop::allowed::calling_session()
        .await
        .map_err(|cause| {
            CmdError::click(format!(
                "the agent session submitting {commit} could not be read ({cause}), so whose \
                 work its gates are is not known"
            ))
        })?;
    enqueue(&checkout, &repository, &commit, defects, (session, None))
}

/// `stado change cancel ID`: a queued candidate is cancelled at once; a
/// running one is asked for, and its pass stops it before the next step.
/// Either way the request is the candidate's `.cancel` file, so a pass that
/// took the candidate meanwhile still reads it, and a running candidate's
/// record is never written here.
pub fn cancel(id: &str) -> Result<Candidate, CmdError> {
    let mut candidate = record::find(id)?;
    if !matches!(candidate.state, State::Queued | State::Running) {
        return Err(CmdError::refused(format!(
            "candidate {} is {}, so there is nothing to cancel",
            candidate.id,
            candidate.state.word()
        )));
    }
    record::request_cancel(&candidate)?;
    candidate.cancel_requested = true;
    if candidate.state == State::Queued {
        candidate.state = State::Cancelled;
        record::save(&mut candidate)?;
    }
    Ok(candidate)
}

/// `stado change list`: the candidates of `checkout`'s repository (every
/// repository's without one), in `state` when named.
pub async fn list(path: Option<&str>, state: Option<&str>) -> Result<Vec<Candidate>, CmdError> {
    let state = state.map(State::parse).transpose()?;
    let repository = match path {
        Some(path) => Some(git::repository(&checkout(path).await?).await?),
        None => None,
    };
    Ok(record::list(repository.as_deref())?
        .into_iter()
        .filter(|candidate| state.is_none_or(|state| candidate.state == state))
        .collect())
}

/// One line of a candidate: its id, state, commit, where it got to and,
/// while it has no verdict, what it waits on.
pub fn line(candidate: &Candidate) -> String {
    let landed = candidate
        .landed
        .as_ref()
        .map_or(String::new(), |landed| format!(" landed {}", landed.commit));
    let risk = candidate
        .risk
        .as_ref()
        .map_or(String::new(), |risk| format!(" risk {}", risk.level));
    let refusal = candidate
        .refusal
        .as_deref()
        .and_then(|refusal| refusal.lines().next())
        .map_or(String::new(), |first| format!(": {first}"));
    let waits = waits(candidate).map_or(String::new(), |waits| format!("\n  {waits}"));
    format!(
        "{} {} {} {}{risk}{landed}{refusal}{waits}",
        candidate.id,
        candidate.state.word(),
        candidate.repository,
        candidate.commit
    )
}

/// What `candidate` waits on and the command that unblocks it, while it
/// has no verdict: none for a landed, refused, repaired or cancelled one.
pub fn waits(candidate: &Candidate) -> Option<String> {
    let checkout = candidate.checkout.display();
    let run = format!("stado change run {checkout}");
    let product = candidate.product.as_deref().unwrap_or("its product");
    let said = match candidate.state {
        State::Landed | State::Refused | State::Repaired | State::Cancelled => return None,
        State::Running => match record::pass_running(&candidate.repository) {
            Ok(true) => format!(
                "carried by a pass{}; `stado change status {}` follows it",
                candidate
                    .pass
                    .as_ref()
                    .map_or(String::new(), |pass| format!(
                        " (pid {}, since {})",
                        pass.pid, pass.started_at
                    )),
                candidate.id
            ),
            Ok(false) => format!(
                "waits on: a pass; the pass that carried it{} is gone (killed, or its machine \
                 restarted) and holds no lock of {}; unblock: `{run}`, which resumes it, or the \
                 resident pass of the queue",
                candidate
                    .pass
                    .as_ref()
                    .map_or(String::new(), |pass| format!(" (pid {})", pass.pid)),
                candidate.repository
            ),
            Err(error) => format!(
                "waits on: a pass whose lock cannot be read ({error}); repair the queue's \
                 directory, then `{run}`"
            ),
        },
        State::Queued => match candidate.waits_on {
            None => format!(
                "waits on: the next pass of the queue; unblock: `{run}`, or the resident pass"
            ),
            Some(WaitsOn::Remote) => format!(
                "waits on: the remote; origin of {checkout} did not answer a fetch or a push \
                 (the refusal names it); unblock: make `git -C {checkout} fetch origin main` \
                 answer, then `{run}`"
            ),
            Some(WaitsOn::Admission) => format!(
                "waits on: Oko's admission of its gates; no ruling of the operator admits \
                 compile or test of {product} now, neither for the fleet's daily build nor for \
                 its submitter (the refusal quotes the ruling that decided); unblock: a ruling \
                 of the operator that admits them (`oko rulings constraints` lists what stands), \
                 then `{run}` from no agent session"
            ),
            Some(WaitsOn::Gate) => format!(
                "waits on: a host for its gates; no host of its platforms could run them (the \
                 refusal names each host's verdict); unblock: `{run} --host HOST`, or bring a \
                 host of that platform back to publishing capacity"
            ),
            Some(WaitsOn::ReviewRound) => format!(
                "waits on: a harsh review round over its tree by a reviewer that is not its \
                 author; the reviewer was not run, or was stopped before `tama reviews harsh \
                 record`; unblock: `{run}` from no agent session (the resident pass), so the \
                 end of a session never stops its reviewer"
            ),
        },
    };
    Some(said)
}

/// Everything one candidate holds, as `status` prints it.
pub fn print(candidate: &Candidate) {
    println!("{}", line(candidate));
    println!("  checkout {}", candidate.checkout.display());
    println!("  defects {}", candidate.defects.join(", "));
    if let Some(rebased) = &candidate.rebased {
        println!(
            "  rebased {} commit(s) onto {} as {}",
            rebased.commits, rebased.base, rebased.commit
        );
    }
    if let Some(risk) = &candidate.risk {
        println!(
            "  {} risk ({} files, {} lines): {}",
            risk.level,
            risk.changed_files,
            risk.changed_lines,
            risk.reasons.join("; ")
        );
    }
    for shard in &candidate.shards {
        let cached = if shard.cached { ", action cache" } else { "" };
        println!(
            "  gate {} ({}) for {} on {}: exit {} after {}s{cached}",
            shard.gate, shard.kind, shard.platform, shard.host, shard.exit_code, shard.seconds
        );
    }
    for review in &candidate.reviews {
        println!(
            "  review {} of {}: {}",
            review.when, review.tree, review.verdict
        );
    }
    if let Some(repairs) = &candidate.repairs {
        println!(
            "  repairs {} (tree {}) queued as candidate {}",
            repairs.commit, repairs.tree, repairs.queued_as
        );
    }
    if let Some(refusal) = &candidate.refusal {
        println!("  refusal: {refusal}");
    }
    for line in &candidate.unrecorded {
        println!("  unrecorded: {line}");
    }
}

/// The candidates as JSON, each with `waits`: what it waits on and the
/// command that unblocks it, null at a verdict.
pub fn json(candidates: &[Candidate]) -> Result<Value, CmdError> {
    let mut listed = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let mut value = serde_json::to_value(candidate)?;
        value["waits"] = json!(waits(candidate));
        listed.push(value);
    }
    Ok(json!({ "candidates": listed }))
}
