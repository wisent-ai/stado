//! Building the canonical desired-v2 activation manifest out of the scope
//! TSV files, plus the small TSV readers that feed it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::artifacts_models::{ArtifactLocation, ArtifactManifest, ArtifactProducer, ArtifactRef};

// ---------------------------------------------------------------------------
// build_activation_manifest (desired-v2 import)
// ---------------------------------------------------------------------------

fn revision_is_hex_commit(revision: &str) -> bool {
    (40..=64).contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Python `_read_tsv`: `csv.DictReader(delimiter="\t")`. DEVIATION: no
/// quoting/escaping support — the desired-state TSVs are plain
/// tab-separated values; quoted fields are pathological input here.
fn read_tsv(path: &Path) -> Result<Vec<BTreeMap<String, String>>, String> {
    let content =
        std::fs::read_to_string(path).map_err(|exc| format!("{}: {exc}", path.display()))?;
    let mut lines = content.lines();
    let headers: Vec<&str> = lines
        .next()
        .unwrap_or("")
        .trim_end_matches('\r')
        .split('\t')
        .collect();
    let mut rows = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        let row: BTreeMap<String, String> = headers
            .iter()
            .copied()
            .zip(line.split('\t'))
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        rows.push(row);
    }
    Ok(rows)
}

/// Python `int(value)` with its ValueError message shape.
fn parse_int(value: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("invalid literal for int() with base 10: '{value}'"))
}

/// TSV cell lookup: missing columns read as "".
fn cell<'a>(row: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    row.get(key).map(String::as_str).unwrap_or("")
}

/// Python `list(dict.fromkeys(values))`: dedupe, keep first-seen order.
fn dedupe_ordered(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            out.push(value);
        }
    }
    out
}

/// Construct the canonical desired-v2 manifest from scope TSV files
/// (Python `build_activation_manifest` in `adapters/activations.py`).
///
/// Errors carry the Python `ValueError` message text (the CLI prints them
/// verbatim, exit 1); file-read failures carry the I/O error.
pub fn build_activation_manifest(
    repo: &str,
    revision: &str,
    desired_state_dir: &Path,
    run_id: &str,
    job_ids: &[String],
    version: &str,
) -> Result<ArtifactManifest, String> {
    if !revision_is_hex_commit(revision) {
        return Err("revision must be an immutable 40-64 character hexadecimal commit".to_string());
    }
    let model_rows = read_tsv(&desired_state_dir.join("model_scope.tsv"))?;
    let target_rows =
        read_tsv(&desired_state_dir.join("activation_expected_pair_targets_refined.tsv"))?;
    let format_rows = read_tsv(&desired_state_dir.join("activation_format_scope.tsv"))?;
    let raw_rows = read_tsv(&desired_state_dir.join("raw_reduced_benchmark_scope.tsv"))?;
    let canonical_benchmarks: Vec<String> =
        std::fs::read_to_string(desired_state_dir.join("activation_benchmarks_canonical.txt"))
            .map_err(|exc| {
                format!(
                    "{}: {exc}",
                    desired_state_dir
                        .join("activation_benchmarks_canonical.txt")
                        .display()
                )
            })?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect();

    let targets_by_benchmark: HashMap<&str, &BTreeMap<String, String>> = target_rows
        .iter()
        .map(|row| (cell(row, "benchmark"), row))
        .collect();
    let missing_targets: Vec<&str> = canonical_benchmarks
        .iter()
        .map(String::as_str)
        .filter(|benchmark| !targets_by_benchmark.contains_key(benchmark))
        .collect();
    if !missing_targets.is_empty() {
        return Err(format!(
            "canonical benchmarks missing target metadata: {}",
            missing_targets[..missing_targets.len().min(10)].join(", ")
        ));
    }

    let mut models: Vec<String> = model_rows
        .iter()
        .filter(|row| cell(row, "in_scope") == "yes")
        .map(|row| cell(row, "model_slug").to_string())
        .collect();
    models.sort();
    let aggregated_benchmarks: Vec<String> = canonical_benchmarks
        .iter()
        .filter(|benchmark| {
            targets_by_benchmark
                .get(benchmark.as_str())
                .is_some_and(|row| cell(row, "status") == "ok")
        })
        .cloned()
        .collect();
    let aggregated_formats = dedupe_ordered(
        format_rows
            .iter()
            .map(|row| cell(row, "activation_collection_format").to_string()),
    );
    let mut raw_benchmarks: Vec<String> = raw_rows
        .iter()
        .filter(|row| cell(row, "raw_scope") == "keep_all_formats")
        .map(|row| cell(row, "benchmark").to_string())
        .collect();
    raw_benchmarks.sort();
    let raw_formats = dedupe_ordered(
        format_rows
            .iter()
            .map(|row| cell(row, "prompt_construction_strategy").to_string()),
    );
    if models.is_empty()
        || aggregated_benchmarks.is_empty()
        || aggregated_formats.is_empty()
        || raw_benchmarks.is_empty()
    {
        return Err("desired-state TSVs produced an empty activation scope".to_string());
    }

    let version = if version.is_empty() {
        format!("desired-v2-{}", revision[..12].to_lowercase())
    } else {
        version.to_string()
    };
    let mut expected_pairs = Map::new();
    for benchmark in &aggregated_benchmarks {
        let row = targets_by_benchmark[benchmark.as_str()];
        expected_pairs.insert(
            benchmark.clone(),
            Value::from(parse_int(cell(row, "expected_pairs"))?),
        );
    }
    let partitions = json!({
        "activation_dataset": {
            "models": models,
            "require_complete_markers": true,
            "raw": {
                "root": "raw_activations",
                "benchmarks": raw_benchmarks,
                "formats": raw_formats,
            },
            "aggregated": {
                "root": "activations",
                "benchmarks": aggregated_benchmarks,
                "expected_pairs": expected_pairs,
                "formats": aggregated_formats,
            },
        }
    });
    let Value::Object(partitions) = partitions else {
        unreachable!("json! object literal is an object")
    };

    let ref_ = ArtifactRef::new("activation-dataset", "wisent-ai", "activations", &version)
        .map_err(|exc| exc.message)?;
    let mut manifest = ArtifactManifest::new(ref_, "Wisent activation database — desired state v2");
    manifest.description =
        "Pinned residual-stream activation dataset for steering experiments.".to_string();
    manifest.producer = ArtifactProducer {
        run_id: run_id.to_string(),
        job_ids: job_ids.to_vec(),
        ..ArtifactProducer::default()
    };
    manifest.locations = vec![ArtifactLocation {
        role: "primary".to_string(),
        uri: format!("hf://datasets/{repo}@{}", revision.to_lowercase()),
        storage: "huggingface".to_string(),
        immutable_revision: revision.to_lowercase(),
        sha256: String::new(),
        size_bytes: None,
        file_count: None,
    }];
    manifest.schemas = ["raw-activations", "aggregated-activations", "pair-texts"]
        .iter()
        .map(|name| {
            Map::from_iter([
                (String::from("name"), json!(name)),
                (String::from("version"), json!(1)),
            ])
        })
        .collect();
    manifest.summary = Map::from_iter([
        ("models".into(), Value::from(models.len() as i64)),
        (
            "raw_benchmarks".into(),
            Value::from(raw_benchmarks.len() as i64),
        ),
        (
            "raw_prompt_formats".into(),
            Value::from(raw_formats.len() as i64),
        ),
        (
            "aggregated_benchmarks".into(),
            Value::from(aggregated_benchmarks.len() as i64),
        ),
        (
            "aggregated_formats".into(),
            Value::from(aggregated_formats.len() as i64),
        ),
        (
            "aggregated_benchmarks_canonical".into(),
            Value::from(canonical_benchmarks.len() as i64),
        ),
        (
            "aggregated_benchmarks_blocked".into(),
            Value::from((canonical_benchmarks.len() - aggregated_benchmarks.len()) as i64),
        ),
        ("component".into(), Value::from("residual_stream")),
    ]);
    manifest.partitions = partitions;
    manifest.labels = BTreeMap::from([
        ("domain".to_string(), "activation-steering".to_string()),
        ("desired_state".to_string(), "v2".to_string()),
    ]);
    Ok(manifest)
}
