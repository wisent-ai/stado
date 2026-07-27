//! Pre-dispatch filter for queued jobs whose results are already on HF.
//!
//! The wrapper short-circuits per-strategy on the box, but on GCP the VM
//! still pays for boot + pip install before discovering there is nothing
//! to do. This module catches that case at scheduler granularity: one HF
//! listing per tick, set-membership check per queued job, completed-jobs
//! get moved straight from queue to completed without ever spinning up.
//!
//! Port of `stado/scheduler/skip_done.py`. The HF SDK call
//! (`huggingface_hub.HfApi.list_repo_files`) becomes a reqwest walk of the
//! HF HTTP tree API (`GET /api/datasets/{repo}/tree/main?recursive=true`,
//! paginated, `HF_TOKEN` env for auth) behind the injectable
//! [`RepoFileLister`] so tests never touch the network.
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
/// following `Link: <url>; rel="next"` pagination. `HF_TOKEN` (when set)
/// goes in the `Authorization: Bearer` header.
pub struct HfApiLister {
    pub base_url: String,
    pub repo_id: String,
    pub token: String,
    client: reqwest::Client,
}

impl HfApiLister {
    /// Production lister for [`HF_REPO_ID`], token from `HF_TOKEN` env.
    pub fn from_env() -> Self {
        Self {
            base_url: "https://huggingface.co".into(),
            repo_id: HF_REPO_ID.into(),
            token: std::env::var("HF_TOKEN").unwrap_or_default(),
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
        let mut files = Vec::new();
        let mut url = Some(self.start_url());
        while let Some(u) = url {
            let mut request = self.client.get(&u);
            if !self.token.is_empty() {
                request = request.header("Authorization", format!("Bearer {}", self.token));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    struct StaticLister {
        files: Vec<String>,
    }

    #[async_trait]
    impl RepoFileLister for StaticLister {
        async fn list_repo_files(&self) -> Result<Vec<String>, SkipDoneError> {
            Ok(self.files.clone())
        }
    }

    /// Every default-strategy prefix for (safe_model, task), as file paths.
    fn all_strategy_files(safe_model: &str, task: &str) -> Vec<String> {
        DEFAULT_STRATEGIES
            .iter()
            .map(|s| {
                format!(
                    "activations/{safe_model}/{task}/{s}/{DEFAULT_COMPONENT}/layer_0.safetensors"
                )
            })
            .collect()
    }

    #[test]
    fn model_to_safe_name_mirrors_hf_config() {
        assert_eq!(
            model_to_safe_name("meta-llama/Llama-3.1-8B"),
            "meta-llama__Llama-3.1-8B"
        );
        assert_eq!(
            model_to_safe_name("org/name:revision"),
            "org__name_revision"
        );
    }

    #[test]
    fn parse_command_quoted_and_bare() {
        assert_eq!(
            parse_command("x --model meta-llama/Llama-3.1-8B --task lm-eval"),
            ("meta-llama/Llama-3.1-8B".to_string(), "lm-eval".to_string())
        );
        assert_eq!(
            parse_command("x --model 'org/q model' --task 't'"),
            ("org/q model".to_string(), "'t'".to_string()) // Python keeps task quotes
        );
        assert_eq!(
            parse_command("x --model \"org/dq\" --task t2"),
            ("org/dq".to_string(), "t2".to_string())
        );
        assert_eq!(
            parse_command("nothing here"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn prefixes_from_files_keeps_only_activations_quads() {
        let files = vec![
            "activations/org__m/task1/chat_last/residual_stream/l0.safetensors".to_string(),
            "activations/org__m/task1/chat_mean/residual_stream/l0.safetensors".to_string(),
            "activations/org__m/task1/chat_last".to_string(), // depth 3 -> not a prefix
            "README.md".to_string(),
            "other/org__m/task1/chat_last/x".to_string(), // not under activations/
        ];
        let prefixes = prefixes_from_files(&files);
        assert_eq!(
            prefixes,
            HashSet::from([
                "activations/org__m/task1/chat_last/".to_string(),
                "activations/org__m/task1/chat_mean/".to_string(),
            ])
        );
    }

    #[test]
    fn is_job_already_done_set_diff_logic() {
        let prefixes: HashSet<String> = prefixes_from_files(&all_strategy_files("org__m", "task1"));
        // All 7 default strategies present -> done.
        assert!(is_job_already_done(
            "x --model org/m --task task1",
            &prefixes
        ));
        // One strategy missing -> not done.
        let mut partial: Vec<String> = all_strategy_files("org__m", "task1");
        partial.retain(|f| !f.contains("role_play"));
        let partial = prefixes_from_files(&partial);
        assert!(!is_job_already_done(
            "x --model org/m --task task1",
            &partial
        ));
        // Different task -> not done.
        assert!(!is_job_already_done(
            "x --model org/m --task task2",
            &prefixes
        ));
        // Unparseable command -> not done (never skip what we can't check).
        assert!(!is_job_already_done("admin --restart", &prefixes));
    }

    #[tokio::test]
    async fn filter_moves_done_jobs_straight_to_completed() {
        let (_dir, store) = store();
        let lister = StaticLister {
            files: all_strategy_files("org__m", "task1"),
        };
        let done = Job::new("j-done", "x --model org/m --task task1");
        let pending = Job::new("j-pending", "x --model org/m --task task2");
        store.write_job("queue", &done).await.unwrap();
        store.write_job("queue", &pending).await.unwrap();

        let now = DateTime::parse_from_rfc3339("2026-05-19T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let logs: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let survivors = filter_already_done(
            vec![done, pending],
            &store,
            now,
            &|m| logs.lock().unwrap().push(m.into()),
            &lister,
        )
        .await
        .unwrap();

        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].job_id, "j-pending");
        assert!(store.read_job("queue", "j-done").await.unwrap().is_none());
        let moved = store
            .read_job("completed", "j-done")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.state, "completed");
        assert_eq!(
            moved.completed_at.as_deref(),
            Some("2026-05-19T12:00:00+00:00")
        );
        assert_eq!(
            moved.error.as_deref(),
            Some("skipped: all strategies present on wisent-ai/activations")
        );
        assert!(store
            .read_job("queue", "j-pending")
            .await
            .unwrap()
            .is_some());
        assert_eq!(
            logs.lock().unwrap().as_slice(),
            &["Skipped 1 jobs already complete on HF (no VM spawn)".to_string()]
        );
    }

    #[tokio::test]
    async fn empty_prefix_set_short_circuits_without_moves() {
        let (_dir, store) = store();
        let lister = StaticLister {
            files: vec!["README.md".to_string()],
        };
        let done = Job::new("j-x", "x --model org/m --task task1");
        store.write_job("queue", &done).await.unwrap();
        let survivors = filter_already_done(
            vec![done],
            &store,
            Utc::now(),
            &|m| panic!("no log expected: {m}"),
            &lister,
        )
        .await
        .unwrap();
        assert_eq!(survivors.len(), 1);
        assert!(store.read_job("queue", "j-x").await.unwrap().is_some());
    }

    #[test]
    fn next_link_parses_rfc8288_header() {
        let header = r#"<https://huggingface.co/api/datasets/x/tree/main?p=1>; rel="next""#;
        assert_eq!(
            next_link(header).as_deref(),
            Some("https://huggingface.co/api/datasets/x/tree/main?p=1")
        );
        assert_eq!(next_link(r#"<https://x/y>; rel="prev""#), None);
        assert_eq!(next_link("garbage"), None);
    }

    #[tokio::test]
    async fn hf_api_lister_walks_pages_and_sends_token() {
        // Two pages over the loopback playback server; the second page is
        // requested via the Link header URL.
        let page1 = r#"[{"type":"file","path":"activations/org__m/t1/chat_last/rs/l0"},{"type":"directory","path":"activations/org__m"}]"#;
        let page2 = r#"[{"type":"file","path":"activations/org__m/t1/chat_mean/rs/l0"}]"#;
        // First response carries the Link header; build responses after the
        // server exists (the Link target needs the port).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let recorded = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let rec = Arc::clone(&recorded);
        let base2 = base.clone();
        let (p1, p2) = (page1.to_string(), page2.to_string());
        let server = tokio::spawn(async move {
            for (i, body) in [p1, p2].into_iter().enumerate() {
                let (mut socket, _) = listener.accept().await.unwrap();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap();
                rec.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let link = if i == 0 {
                    format!("Link: <{base2}/api/datasets/wisent-ai/activations/tree/main?p=1>; rel=\"next\"\r\n")
                } else {
                    String::new()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{link}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let lister = HfApiLister {
            base_url: base,
            repo_id: HF_REPO_ID.into(),
            token: "hf_test_token".into(),
            client: reqwest::Client::new(),
        };
        let files = lister.list_repo_files().await.unwrap();
        server.await.unwrap();
        assert_eq!(
            files,
            vec![
                "activations/org__m/t1/chat_last/rs/l0".to_string(),
                "activations/org__m/t1/chat_mean/rs/l0".to_string(),
            ]
        );
        let requests = recorded.lock().unwrap();
        assert_eq!(requests.len(), 2, "two pages fetched");
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer hf_test_token"),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains("recursive=true"), "{}", requests[0]);
        assert!(requests[1].contains("p=1"), "{}", requests[1]);
    }
}
