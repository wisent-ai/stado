//! Artifact-type verification adapters.
//!
//! Port of `stado/artifacts/adapters/{base,__init__,activations}.py`.
//!
//! DEVIATION: Python keeps a mutable `_ADAPTERS` dict populated by
//! `register_adapter` (with an entry-points-style plugin design); Rust has
//! no entry_points, so the registry is static — [`get_adapter`] is a single
//! match/factory. Adding an adapter = add one arm returning
//! `Box<dyn ArtifactAdapter>`. The trait-object design is kept so the
//! registry and CLI stay adapter-agnostic.
//!
//! The built-in `activation-dataset` adapter verifies the HF dataset tree
//! through the Hugging Face HTTP API (`fetch_hf_tree`, reqwest +
//! `HF_TOKEN`); in Python that listing goes through `huggingface_hub`'s
//! underlying HTTP endpoint via urllib. The inventory checks themselves
//! are pure ([`ActivationDatasetAdapter::inventory_report`]) and tested
//! offline; the tree fetcher is injectable for tests.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::artifacts_models::{
    ArtifactLocation, ArtifactManifest, ArtifactProducer, ArtifactRef, VerificationReport,
};

/// Python `ArtifactAdapter` (Protocol).
#[async_trait]
pub trait ArtifactAdapter: Send + Sync {
    fn type_name(&self) -> &'static str;
    fn adapter_name(&self) -> &'static str;
    /// `full` requests the adapter's exhaustive verification; the
    /// activation adapter currently runs its inventory check either way
    /// (Python parity — `full` is accepted but unused there).
    async fn verify(&self, manifest: &ArtifactManifest, full: bool) -> VerificationReport;
}

/// The static adapter registry (Python `get_adapter` over `_ADAPTERS`).
/// `None` for artifact types with no type-specific verification — the
/// registry then falls back to the `generic-v1` report.
pub fn get_adapter(type_name: &str) -> Option<Box<dyn ArtifactAdapter>> {
    match type_name {
        "activation-dataset" => Some(Box::new(ActivationDatasetAdapter::new())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Hugging Face tree listing
// ---------------------------------------------------------------------------

/// Failure of the HF tree listing. Python reports
/// `f"{type(exc).__name__}: {exc}"`; `kind` carries the exception-style
/// label ("HTTPError" for non-2xx, "RequestError" for transport failures,
/// "RuntimeError" for a non-list body).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeFetchError {
    pub kind: &'static str,
    pub message: String,
}

impl std::fmt::Display for TreeFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// Python `urllib.parse.quote(value, safe=...)`: UTF-8 percent-encoding,
/// unreserved `A-Za-z0-9_.-~` never escaped, uppercase hex.
fn quote(value: &str, safe_slash: bool) -> String {
    let mut out = String::new();
    for &byte in value.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'-' | b'~')
            || (safe_slash && byte == b'/');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Python `_next_link`: pull `rel="next"` out of an RFC 5988 Link header.
fn next_link(header: &str) -> String {
    for part in header.split(',') {
        let bits: Vec<&str> = part.trim().split(';').collect();
        if bits.len() > 1 && bits[1..].iter().any(|bit| bit.trim() == "rel=\"next\"") {
            return bits[0]
                .trim()
                .trim_matches(|c| c == '<' || c == '>')
                .to_string();
        }
    }
    String::new()
}

/// List every file at one immutable Hugging Face dataset revision. Follows
/// `rel="next"` pagination and uses `stado-huggingface/token` from Skarbiec
/// when present.
pub async fn fetch_hf_tree(repo: &str, revision: &str) -> Result<Vec<String>, TreeFetchError> {
    let encoded_repo = quote(repo, true);
    let encoded_revision = quote(revision, false);
    let mut url = format!(
        "https://huggingface.co/api/datasets/{encoded_repo}/tree/{encoded_revision}\
         ?recursive=true&expand=false&limit=1000"
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent("stado-artifacts/1")
        .build()
        .map_err(|exc| TreeFetchError {
            kind: "RequestError",
            message: exc.to_string(),
        })?;
    let token = crate::skarbiec::read_string("stado-huggingface", "token")
        .await
        .map_err(|exc| TreeFetchError {
            kind: "AuthenticationError",
            message: exc.to_string(),
        })?
        .unwrap_or_default();

    let mut paths: Vec<String> = Vec::new();
    while !url.is_empty() {
        let mut request = client.get(&url);
        if !token.is_empty() {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|exc| TreeFetchError {
                kind: if exc.is_status() {
                    "HTTPError"
                } else {
                    "RequestError"
                },
                message: exc.to_string(),
            })?;
        let link = response
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let page: Value = response.json().await.map_err(|exc| TreeFetchError {
            kind: "RequestError",
            message: exc.to_string(),
        })?;
        let Some(items) = page.as_array() else {
            return Err(TreeFetchError {
                kind: "RuntimeError",
                message: "Hugging Face tree response is not a list".to_string(),
            });
        };
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("file") {
                if let Some(path) = item.get("path").and_then(Value::as_str) {
                    if !path.is_empty() {
                        paths.push(path.to_string());
                    }
                }
            }
        }
        url = next_link(&link);
    }
    Ok(paths)
}

/// Injectable tree fetcher (Python passes `tree_fetcher` to the adapter
/// constructor for tests).
pub type TreeFetcher = Arc<
    dyn Fn(String, String) -> BoxFuture<'static, Result<Vec<String>, TreeFetchError>> + Send + Sync,
>;

// ---------------------------------------------------------------------------
// ActivationDatasetAdapter
// ---------------------------------------------------------------------------

fn hf_location_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^hf://datasets/([^@]+)@([0-9a-fA-F]{40,64})$").expect("static regex compiles")
    })
}

fn raw_shard_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"/layer_\d+_chunk_\d+\.safetensors$").expect("static regex compiles")
    })
}

fn aggregated_shard_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/layer_\d+\.safetensors$").expect("static regex compiles"))
}

/// Python `bool(value)` on a JSON value (for `require_complete_markers`).
fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// Python `value.get(key, [])` iterated as a list of strings. Non-array
/// values degrade to empty (Python would iterate dict keys / string chars
/// — pathological input the Rust port refuses to emulate; noted deviation).
fn str_list(map: &Map<String, Value>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Python `ActivationDatasetAdapter` (`type_name = "activation-dataset"`,
/// `adapter_name = "activation-dataset-v1"`).
pub struct ActivationDatasetAdapter {
    tree_fetcher: TreeFetcher,
}

impl ActivationDatasetAdapter {
    /// Production adapter: lists the pinned revision over the HF HTTP API.
    pub fn new() -> Self {
        Self {
            tree_fetcher: Arc::new(|repo, revision| {
                Box::pin(async move { fetch_hf_tree(&repo, &revision).await })
            }),
        }
    }

    /// Adapter with an injected tree fetcher (tests, offline fixtures).
    pub fn with_fetcher(tree_fetcher: TreeFetcher) -> Self {
        Self { tree_fetcher }
    }

    /// Python `_location`: the (repo, lowercase revision) behind the
    /// primary `hf://datasets/<repo>@<commit>` location, or `None` when the
    /// URI shape or `immutable_revision` cross-check fails.
    fn location(manifest: &ArtifactManifest) -> Option<(String, String)> {
        let primary = manifest
            .locations
            .iter()
            .find(|item| item.role == "primary")?;
        let captures = hf_location_re().captures(&primary.uri)?;
        let (repo, revision) = (&captures[1], &captures[2]);
        // Python compares case-sensitively against the captured revision.
        if !primary.immutable_revision.is_empty() && primary.immutable_revision != revision {
            return None;
        }
        Some((repo.to_string(), revision.to_lowercase()))
    }

    /// The pure inventory check over an already-listed file set (everything
    /// after the tree fetch in Python `verify`).
    fn inventory_report(
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

    fn report(
        &self,
        passed: bool,
        issues: Vec<String>,
        summary: Map<String, Value>,
    ) -> VerificationReport {
        VerificationReport {
            adapter: self.adapter_name().to_string(),
            passed,
            issues,
            summary,
        }
    }
}

impl Default for ActivationDatasetAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ArtifactAdapter for ActivationDatasetAdapter {
    fn type_name(&self) -> &'static str {
        "activation-dataset"
    }

    fn adapter_name(&self) -> &'static str {
        "activation-dataset-v1"
    }

    async fn verify(&self, manifest: &ArtifactManifest, _full: bool) -> VerificationReport {
        let Some((repo, revision)) = Self::location(manifest) else {
            return self.report(
                false,
                vec!["primary location must be hf://datasets/<repo>@<40-64 hex commit>".into()],
                Map::new(),
            );
        };
        let spec = manifest
            .partitions
            .get("activation_dataset")
            .and_then(Value::as_object);
        let Some(spec) = spec else {
            return self.report(
                false,
                vec!["partitions.activation_dataset specification is required".into()],
                Map::new(),
            );
        };
        // Python validates models/raw/aggregated BEFORE the network call;
        // a cheap pre-check avoids listing the repo for a malformed spec.
        let structural = {
            let mut issues = Vec::new();
            if str_list(spec, "models").is_empty() {
                issues.push("activation_dataset.models must be a non-empty list".to_string());
            }
            if !spec.get("raw").is_some_and(Value::is_object) {
                issues.push("activation_dataset.raw must be an object".to_string());
            }
            if !spec.get("aggregated").is_some_and(Value::is_object) {
                issues.push("activation_dataset.aggregated must be an object".to_string());
            }
            issues
        };
        if !structural.is_empty() {
            return self.report(false, structural, Map::new());
        }
        let files: HashSet<String> = match (self.tree_fetcher)(repo, revision).await {
            Ok(files) => files.into_iter().collect(),
            Err(exc) => {
                return self.report(
                    false,
                    vec![format!(
                        "could not list pinned Hugging Face revision: {exc}"
                    )],
                    Map::new(),
                );
            }
        };
        self.inventory_report(spec, &files)
    }
}

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
