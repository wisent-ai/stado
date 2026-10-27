//! The pure, offline half of the activation-dataset verification: the
//! inventory check over an already-listed repository file set.

use std::collections::HashSet;

use regex::Regex;
use serde_json::{Map, Value};

use super::helpers::{aggregated_shard_re, raw_shard_re, DatasetSpec, Partition};
use super::ActivationDatasetAdapter;
use crate::artifacts_models::VerificationReport;

impl ActivationDatasetAdapter {
    /// The pure inventory check of a read specification over an
    /// already-listed file set (everything after the tree fetch).
    pub(super) fn inventory_report(
        &self,
        spec: &DatasetSpec,
        files: &HashSet<String>,
    ) -> VerificationReport {
        let mut issues: Vec<String> = Vec::new();
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

        // A leaf is missing when its directory holds no shard or, when the
        // specification requires it, no `_complete.json` marker.
        let missing_leaves = |partition: &Partition, shard_leaves: &HashSet<String>| {
            let mut missing = Vec::new();
            for model in &spec.models {
                for benchmark in &partition.benchmarks {
                    for prompt_format in &partition.formats {
                        let prefix =
                            format!("{}/{model}/{benchmark}/{prompt_format}/", partition.root);
                        let complete =
                            complete_markers.contains(format!("{prefix}_complete.json").as_str());
                        if !shard_leaves.contains(&prefix) || (spec.require_complete && !complete) {
                            missing.push(prefix.trim_end_matches('/').to_string());
                        }
                    }
                }
            }
            missing
        };
        let raw_missing = missing_leaves(&spec.raw, &raw_shard_leaves);
        let aggregate_missing = missing_leaves(&spec.aggregated, &aggregated_shard_leaves);
        let mut pair_text_missing: Vec<String> = spec
            .raw
            .benchmarks
            .iter()
            .map(|benchmark| format!("pair_texts/{benchmark}.json"))
            .filter(|pair_path| !files.contains(pair_path))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        pair_text_missing.sort();

        // Every missing path is named: a sample would hide which leaves an
        // operator has to produce.
        let add_missing = |label: &str, values: &[String], issues: &mut Vec<String>| {
            if !values.is_empty() {
                issues.push(format!("missing/incomplete {label}: {}", values.join(", ")));
            }
        };
        add_missing("raw leaves", &raw_missing, &mut issues);
        add_missing("aggregated leaves", &aggregate_missing, &mut issues);
        add_missing("pair-text mappings", &pair_text_missing, &mut issues);

        let leaves = |partition: &Partition| {
            (spec.models.len() * partition.benchmarks.len() * partition.formats.len()) as i64
        };
        let raw_leaves = leaves(&spec.raw);
        let aggregate_leaves = leaves(&spec.aggregated);
        let pair_text_benchmarks_expected: HashSet<&String> = spec.raw.benchmarks.iter().collect();
        let summary = Map::from_iter([
            ("models".into(), Value::from(spec.models.len() as i64)),
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
                    pair_text_benchmarks_expected.len() as i64 - pair_text_missing.len() as i64,
                ),
            ),
            ("repository_files".into(), Value::from(files.len() as i64)),
            ("verification_mode".into(), Value::from("inventory")),
        ]);
        self.report(issues.is_empty(), issues, summary)
    }
}
