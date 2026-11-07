//! How a candidate's shards read where the queue shows them: `stado change
//! status`, `stado change list`, a landing's integration line and the wait
//! and refusal a reviewer reads. One rendering each, so a gate the operator's
//! rulings left out reads the same everywhere and never as one that passed.

use super::{Candidate, Shard};

/// What a reviewer does for `candidate` at `commit`, as the review's
/// refusal and the review-round wait both say it.
pub fn reviewer_asked(candidate: &Candidate, commit: &str) -> String {
    format!(
        "a reviewer reads `git -C {} show {commit}` and, from its own session's shell, records \
         `stado change review {} --findings FILE --improvements FILE`",
        candidate.checkout.display(),
        candidate.id
    )
}

/// The refusal that holds `candidate` for a recorded round over `commit`
/// (tree `tree`): Tama is not installed (`tama_missing`, where it was looked
/// for), or a gate the rulings left out keeps it from Tama, whose review
/// reads only the candidate's own message; `None` when Tama runs it.
pub fn held(
    candidate: &Candidate,
    commit: &str,
    tree: &str,
    tama_missing: Option<String>,
) -> Option<String> {
    let unchecked = unchecked(&candidate.shards);
    let why = match tama_missing {
        Some(missing) => {
            format!("Tama, whose harsh review would run one, is not installed here ({missing})")
        }
        None if !unchecked.is_empty() => {
            "a gate the rulings left out keeps it from Tama, whose review reads only its \
             message"
                .to_string()
        }
        None => return None,
    };
    Some(format!(
        "no review round over commit {commit} (tree {tree}) of candidate {} is recorded by a \
         session that is not its author ({}), and {why}; {}, and the next pass reads \
         it{unchecked}",
        candidate.id,
        candidate.session.as_deref().unwrap_or("operator"),
        reviewer_asked(candidate, commit)
    ))
}

/// `shard` as `stado change status` and a landing's integration line say
/// it: the gate that ran, its host, exit and time, or the gate the rulings
/// left out with the ruling.
pub fn line(shard: &Shard) -> String {
    let named = format!("{} ({}) for {}", shard.gate, shard.kind, shard.platform);
    match &shard.left_out {
        Some(ruling) => format!("{named}: not run, left out: {ruling}"),
        None => {
            let cached = if shard.cached { ", action cache" } else { "" };
            let (host, exit, seconds) = (&shard.host, shard.exit_code, shard.seconds);
            format!("{named} on {host}: exit {exit} after {seconds}s{cached}")
        }
    }
}

/// What a reviewer is told of a candidate whose gates the rulings left out:
/// each such gate, and that whether it compiles is the review's to judge
/// from the code; empty when every gate ran.
pub fn unchecked(shards: &[Shard]) -> String {
    let left: Vec<String> = shards
        .iter()
        .filter(|shard| shard.left_out.is_some())
        .map(line)
        .collect();
    if left.is_empty() {
        return String::new();
    }
    format!(
        "; no lint or test gate ran on it ({}), so the reviewer judges whether it compiles by \
         reading it",
        left.join("; ")
    )
}

/// What `stado change list` adds for a candidate whose gates the rulings
/// left out: those gates, and where their rulings are read; empty when
/// every gate ran.
pub fn left_out_note(shards: &[Shard], id: &str) -> String {
    let left: Vec<String> = shards
        .iter()
        .filter(|shard| shard.left_out.is_some())
        .map(|shard| format!("{} ({}) for {}", shard.gate, shard.kind, shard.platform))
        .collect();
    if left.is_empty() {
        return String::new();
    }
    format!(
        "; gates left out: {}, see `stado change status {id}`",
        left.join(", ")
    )
}
