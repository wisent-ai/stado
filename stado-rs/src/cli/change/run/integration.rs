//! What a landing records: one integration line per defect (the
//! integrators' backlog shape), and Oko's record of each defect repaired by
//! the landed commit (`oko defects repaired`, as `stado ci run` does).

use std::path::Path;

use crate::cli::change::record::{Candidate, Integrated};

/// The integration line of each defect `candidate` landed, and Oko's record
/// of each repaired; a refused register write is kept in `unrecorded`.
pub async fn record(candidate: &mut Candidate, integrated: Option<&Path>) {
    let Some(landed) = candidate.landed.clone() else {
        return;
    };
    let gates: Vec<String> = candidate
        .shards
        .iter()
        .map(|shard| {
            format!(
                "{} ({}) for {} on {}{}: exit {}",
                shard.gate,
                shard.kind,
                shard.platform,
                shard.host,
                if shard.cached { " (action cache)" } else { "" },
                shard.exit_code
            )
        })
        .collect();
    let evidence = format!(
        "stado change {} rebased onto {} and landed {} after {} tip(s); gates: {}",
        candidate.id,
        candidate
            .rebased
            .as_ref()
            .map_or("", |rebased| rebased.base.as_str()),
        landed.commit,
        landed.attempts,
        gates.join("; ")
    );
    candidate.integrated = candidate
        .defects
        .iter()
        .map(|defect| Integrated {
            id: defect.clone(),
            commit: landed.commit.clone(),
            gates: "pass".to_string(),
            criterion: "not-run".to_string(),
            evidence: evidence.clone(),
            why_not_run: "the submit queue runs the product's declared gates; no journey of \
                          this defect's own criterion is declared to it"
                .to_string(),
        })
        .collect();
    let mut unrecorded = Vec::new();
    crate::cli::ci::delivery::oko::repaired(
        &candidate.defects,
        &format!("The submit queue landed candidate {} as", candidate.id),
        &candidate.repository,
        &landed.commit,
        &mut unrecorded,
    )
    .await;
    if let (Some(directory), Some(product)) = (integrated, candidate.product.as_deref()) {
        if let Err(error) = append(directory, product, &candidate.integrated) {
            unrecorded.push(error);
        }
    }
    candidate.unrecorded.extend(unrecorded);
}

/// Append `lines` to `directory/<product>.jsonl`.
fn append(directory: &Path, product: &str, lines: &[Integrated]) -> Result<(), String> {
    use std::io::Write;
    let path = directory.join(format!("{product}.jsonl"));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            format!(
                "{} was not opened to record the landing: {error}",
                path.display()
            )
        })?;
    for line in lines {
        let text = serde_json::to_string(line).map_err(|error| error.to_string())?;
        writeln!(file, "{text}")
            .map_err(|error| format!("{} was not written: {error}", path.display()))?;
    }
    Ok(())
}
