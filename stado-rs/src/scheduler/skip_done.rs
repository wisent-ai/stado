//! Pre-dispatch filter for queued jobs whose results are already on HF.
//!
//! The wrapper short-circuits per-strategy on the box, but on GCP the VM
//! still pays for boot + pip install before discovering there is nothing
//! to do. This module catches that case at scheduler granularity: one HF
//! listing per tick, set-membership check per queued job, completed-jobs
//! get moved straight from queue to completed without ever spinning up.
//!
//! (`huggingface_hub.HfApi.list_repo_files`) becomes a reqwest walk of the
//! HF HTTP tree API (`GET /api/datasets/{repo}/tree/main?recursive=true`,
//! paginated, with `stado-huggingface/token` from Skarbiec for auth) behind
//! the injectable [`RepoFileLister`] so tests never touch the network.
//!
//! NOTE — currently DISABLED in the Python scheduler; the module is ported
//! faithfully with the scheduler.py comment preserved:
//!
//!   filter_already_done was disabled: HfApi.list_repo_files on the
//!   184k-file wisent-ai/activations repo takes 50+s, eating the 60s
//!   function timeout before any dispatch fires. Wrapper still
//!   short-circuits per-strategy on the box so the cost is only VM boot
//!   for results-already-uploaded jobs.

use std::collections::HashSet;
use std::sync::LazyLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use regex::Regex;

use crate::models::{job_state, Job};
use crate::queue::{JobStorage, StorageError};

/// Strategies the activation wrapper runs by default. Keep in sync with
/// wisent_tools/scripts/activations/extract_and_upload.VALIDATED_STRATEGIES.
pub const DEFAULT_STRATEGIES: [&str; 7] = [
    "chat_last",
    "chat_mean",
    "chat_first",
    "chat_max_norm",
    "chat_weighted",
    "mc_balanced",
    "role_play",
];
pub const DEFAULT_COMPONENT: &str = "residual_stream";
pub const HF_REPO_ID: &str = "wisent-ai/activations";
pub const HF_REPO_TYPE: &str = "dataset";

/// Skip-done failure. The HF listing is a hard dependency of the filter:
/// an HF outage must crash the scheduler (propagate) so the operator sees
/// it instead of silently scheduling duplicate extraction work over jobs
/// already on HF.
#[derive(Debug, thiserror::Error)]
pub enum SkipDoneError {
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("{0}")]
    Other(String),
}

/// Mirror wisent.core.reading.modules.utilities.data.sources.hf.hf_config.
pub fn model_to_safe_name(model: &str) -> String {
    model.replace('/', "__").replace(':', "_")
}

static TASK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"--task\s+(\S+)").expect("static regex compiles"));
static MODEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"--model\s+'([^']+)'|--model\s+"([^"]+)"|--model\s+(\S+)"#)
        .expect("static regex compiles")
});

/// Pull (model, task) out of an extract_and_upload command line.
/// Python `_parse_command`.
pub fn parse_command(cmd: &str) -> (String, String) {
    let task = TASK_RE
        .captures(cmd)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let model = MODEL_RE
        .captures(cmd)
        .map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .or_else(|| c.get(3))
                .map(|m| m.as_str())
                .unwrap_or("")
        })
        .unwrap_or("")
        .to_string();
    (model, task)
}

/// The HF listing source, injectable for tests. Production code uses
/// [`HfApiLister`].
#[async_trait]
pub trait RepoFileLister: Send + Sync {
    /// Every file path in the target repo (recursive).
    async fn list_repo_files(&self) -> Result<Vec<String>, SkipDoneError>;
}

/// reqwest-backed lister replacing `huggingface_hub.HfApi.list_repo_files`:
/// walks `GET {base}/api/datasets/{repo_id}/tree/main?recursive=true`
/// following `Link: <url>; rel="next"` pagination. The production constructor
/// resolves its bearer token from Skarbiec when the listing starts.
pub struct HfApiLister {
    pub base_url: String,
    pub repo_id: String,
    pub token: String,
    client: reqwest::Client,
}

impl HfApiLister {
    /// Production lister for [`HF_REPO_ID`]; credentials come from Skarbiec.
    pub fn from_env() -> Self {
        Self {
            base_url: "https://huggingface.co".into(),
            repo_id: HF_REPO_ID.into(),
            token: String::new(),
            client: reqwest::Client::new(),
        }
    }

    fn start_url(&self) -> String {
        format!(
            "{}/api/datasets/{}/tree/main?recursive=true&expand=false",
            self.base_url, self.repo_id
        )
    }
}

#[async_trait]
impl RepoFileLister for HfApiLister {
    async fn list_repo_files(&self) -> Result<Vec<String>, SkipDoneError> {
        let token = if self.token.is_empty() {
            crate::skarbiec::read_string("stado-huggingface", "token")
                .await
                .map_err(|err| SkipDoneError::Other(err.to_string()))?
                .unwrap_or_default()
        } else {
            self.token.clone()
        };
        let mut files = Vec::new();
        let mut url = Some(self.start_url());
        while let Some(u) = url {
            let mut request = self.client.get(&u);
            if !token.is_empty() {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            let response = request.send().await?;
            let next = response
                .headers()
                .get(reqwest::header::LINK)
                .and_then(|v| v.to_str().ok())
                .and_then(next_link);
            let response = response.error_for_status()?;
            let entries: Vec<serde_json::Value> = response.json().await?;
            for entry in entries {
                if entry.get("type").and_then(serde_json::Value::as_str) != Some("file") {
                    continue;
                }
                if let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) {
                    files.push(path.to_string());
                }
            }
            url = next;
        }
        Ok(files)
    }
}

/// URL of the `rel="next"` link in an RFC 8288 Link header, if present.
fn next_link(header: &str) -> Option<String> {
    for part in header.split(',') {
        let mut segments = part.split(';');
        let Some(target) = segments.next() else {
            continue;
        };
        let target = target.trim();
        if !target.starts_with('<') || !target.ends_with('>') {
            continue;
        }
        let is_next = segments.any(|seg| seg.trim() == r#"rel="next""#);
        if is_next {
            return Some(target[1..target.len() - 1].to_string());
        }
    }
    None
}

/// Set of unique 'activations/<safe_model>/<task>/<strategy>/' prefixes.
/// Python `fetch_hf_done_prefixes` (the set-building half).
pub fn prefixes_from_files(files: &[String]) -> HashSet<String> {
    let mut prefixes = HashSet::new();
    for f in files {
        let parts: Vec<&str> = f.split('/').collect();
        if parts.len() >= 4 && parts[0] == "activations" {
            prefixes.insert(format!(
                "{}/{}/{}/{}/",
                parts[0], parts[1], parts[2], parts[3]
            ));
        }
    }
    prefixes
}

/// Set of unique done-prefixes, from the injectable lister. Python
/// `fetch_hf_done_prefixes`.
pub async fn fetch_hf_done_prefixes(
    lister: &dyn RepoFileLister,
) -> Result<HashSet<String>, SkipDoneError> {
    Ok(prefixes_from_files(&lister.list_repo_files().await?))
}

/// True if every default strategy for this (model, task) has files in HF.
/// Python `is_job_already_done`.
pub fn is_job_already_done(command: &str, prefixes: &HashSet<String>) -> bool {
    let (model, task) = parse_command(command);
    if model.is_empty() || task.is_empty() {
        return false;
    }
    let safe = model_to_safe_name(&model);
    DEFAULT_STRATEGIES
        .iter()
        .all(|strategy| prefixes.contains(&format!("activations/{safe}/{task}/{strategy}/")))
}

/// Move every queued job whose results are already on HF straight to
/// completed, returning the surviving still-to-run list. log_fn receives
/// a single summary line. Python `filter_already_done`.
pub async fn filter_already_done(
    queued: Vec<Job>,
    store: &JobStorage,
    now_utc: DateTime<Utc>,
    log_fn: &dyn Fn(&str),
    lister: &dyn RepoFileLister,
) -> Result<Vec<Job>, SkipDoneError> {
    let prefixes = fetch_hf_done_prefixes(lister).await?;
    if prefixes.is_empty() {
        return Ok(queued);
    }
    let mut survivors = Vec::new();
    let mut skipped = 0usize;
    for mut job in queued {
        if is_job_already_done(&job.command, &prefixes) {
            job.state = job_state::COMPLETED.into();
            job.completed_at = Some(crate::models::isoformat_utc(now_utc));
            job.error = Some("skipped: all strategies present on wisent-ai/activations".into());
            store.move_job(&job, "queue", "completed").await?;
            skipped += 1;
        } else {
            survivors.push(job);
        }
    }
    if skipped > 0 {
        log_fn(&format!(
            "Skipped {skipped} jobs already complete on HF (no VM spawn)"
        ));
    }
    Ok(survivors)
}

