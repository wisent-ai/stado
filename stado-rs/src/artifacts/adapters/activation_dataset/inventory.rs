//! The pure, offline half of the activation-dataset verification: the
//! inventory check over an already-listed repository file set.

use std::collections::HashSet;

use regex::Regex;
use serde_json::{Map, Value};

use super::helpers::{aggregated_shard_re, py_truthy, raw_shard_re, str_list};
use super::ActivationDatasetAdapter;
use crate::artifacts_models::VerificationReport;

impl ActivationDatasetAdapter {
    /// The pure inventory check over an already-listed file set (everything
    /// after the tree fetch in Python `verify`).
    pub(super) fn inventory_report(
        &self,
        spec: &Map<String, Value>,
        files: &HashSet<String>,
    ) -> VerificationReport {
        let mut issues: Vec<String> = Vec::new();
        let models = str_list(spec, "models");
        let raw = spec.get("raw").and_then(Value::as_object);
        let aggregated = spec.get("aggregated").and_then(Value::as_object);
        if models.is_empty() {
            issues.push("activation_dataset.models must be a non-empty list".to_string());
        }
        if raw.is_none() {
            issues.push("activation_dataset.raw must be an object".to_string());
        }
        if aggregated.is_none() {
            issues.push("activation_dataset.aggregated must be an object".to_string());
        }
        let (Some(raw), Some(aggregated)) = (raw, aggregated) else {
            return self.report(false, issues, Map::new());
        };
        if !issues.is_empty() {
            return self.report(false, issues, Map::new());
        }

        let complete_markers: HashSet<&str> = files
            .iter()
            .filter(|path| path.ends_with("/_complete.json"))
            .map(String::as_str)
            .collect();
        let shard_leaves = |re: &Regex| -> HashSet<String> {
            files
                .iter()
                .filter(|path| re.is_match(path))
                .filter_map(|path| path.rsplit_once('/').map(|(dir, _)| format!("{dir}/")))
                .collect()
        };
        let raw_shard_leaves = shard_leaves(raw_shard_re());
        let aggregated_shard_leaves = shard_leaves(aggregated_shard_re());

        let mut raw_missing: Vec<String> = Vec::new();
        let mut aggregate_missing: Vec<String> = Vec::new();
        let mut pair_text_missing: Vec<String> = Vec::new();
        let mut raw_leaves = 0i64;
        let mut aggregate_leaves = 0i64;
        let require_complete = spec.get("require_complete_markers").is_none_or(py_truthy);

        let raw_root = raw
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or("raw_activations");
        let aggregated_root = aggregated
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or("activations");
        let raw_benchmarks = str_list(raw, "benchmarks");
        let raw_formats = str_list(raw, "formats");
        let aggregated_benchmarks = str_list(aggregated, "benchmarks");
        let aggregated_formats = str_list(aggregated, "formats");

        for model in &models {
            for benchmark in &raw_benchmarks {
                let pair_path = format!("pair_texts/{benchmark}.json");
                if !files.contains(&pair_path) {
                    pair_text_missing.push(pair_path);
                }
                for prompt_format in &raw_formats {
                    raw_leaves += 1;
                    let prefix = format!("{raw_root}/{model}/{benchmark}/{prompt_format}/");
                    let complete =
                        complete_markers.contains(format!("{prefix}_complete.json").as_str());
                    let shards = raw_shard_leaves.contains(&prefix);
                    if !shards || (require_complete && !complete) {
                        raw_missing.push(prefix.trim_end_matches('/').to_string());
                    }
                }
            }
            for benchmark in &aggregated_benchmarks {
                for prompt_format in &aggregated_formats {
                    aggregate_leaves += 1;
                    let prefix = format!("{aggregated_root}/{model}/{benchmark}/{prompt_format}/");
                    let complete =
                        complete_markers.contains(format!("{prefix}_complete.json").as_str());
                    let shards = aggregated_shard_leaves.contains(&prefix);
                    if !shards || (require_complete && !complete) {
                        aggregate_missing.push(prefix.trim_end_matches('/').to_string());
                    }
                }
            }
        }

        let add_missing = |label: &str, values: &[String], issues: &mut Vec<String>| {
            if values.is_empty() {
                return;
            }
            let sample = values[..values.len().min(5)].join(", ");
            let suffix = if values.len() > 5 {
                format!(" (+{} more)", values.len() - 5)
            } else {
                String::new()
            };
            issues.push(format!("missing/incomplete {label}: {sample}{suffix}"));
        };
        add_missing("raw leaves", &raw_missing, &mut issues);
        add_missing("aggregated leaves", &aggregate_missing, &mut issues);
        let mut pair_text_deduped: Vec<String> = pair_text_missing
            .iter()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        pair_text_deduped.sort();
        add_missing("pair-text mappings", &pair_text_deduped, &mut issues);

        let pair_text_benchmarks_expected: HashSet<&String> = raw_benchmarks.iter().collect();
        let pair_text_missing_set: HashSet<&String> = pair_text_missing.iter().collect();
        let summary = Map::from_iter([
            ("models".into(), Value::from(models.len() as i64)),
            ("raw_leaves_expected".into(), Value::from(raw_leaves)),
            (
                "raw_leaves_complete".into(),
                Value::from(raw_leaves - raw_missing.len() as i64),
            ),
            (
                "aggregated_leaves_expected".into(),
                Value::from(aggregate_leaves),
            ),
            (
                "aggregated_leaves_complete".into(),
                Value::from(aggregate_leaves - aggregate_missing.len() as i64),
            ),
            (
                "pair_text_benchmarks_expected".into(),
                Value::from(pair_text_benchmarks_expected.len() as i64),
            ),
            (
                "pair_text_benchmarks_complete".into(),
                Value::from(
                    pair_text_benchmarks_expected.len() as i64 - pair_text_missing_set.len() as i64,
                ),
            ),
            ("repository_files".into(), Value::from(files.len() as i64)),
            ("verification_mode".into(), Value::from("inventory")),
        ]);
        self.report(issues.is_empty(), issues, summary)
    }
}
