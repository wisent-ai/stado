//! The janitor's workdir keep-list must cost a listing, not a download per job.
//!
//! # What happened
//!
//! `disk_cleanup::run_cleanup_once` builds a keep-list of every job id in
//! `queue/` and `running/` before it takes the run lock, and it used
//! `JobStorage::list_jobs(prefix, 0)` to do it — which lists the prefix and
//! then DOWNLOADS every job document, ten at a time, to read `job_id` out of a
//! body whose object name already carried it. On 2026-09-03 charless-mac-mini
//! published `duration_ms: 818021` — 13.6 minutes — for a pass whose own
//! verdict was `healthy_noop` on a host with 19.8 GB free, against a policy
//! `check_interval_seconds` of 300, so passes ran effectively back to back. It
//! also had no bound of any kind: the store's HTTP client sets no timeout, so
//! however long the transport took was how long the pass took.
//!
//! A `healthy_noop` pass returns before the first cleaner, so not one of those
//! downloads was ever read.
//!
//! # What is defended here
//!
//! The cost model and the bound, not the incident. The keep-list must be
//! derivable from object NAMES — zero document downloads, whatever the prefix
//! holds — it must be a superset of the live jobs rather than a subset (an id
//! missing from a keep-list authorizes deleting the tree a live job is writing
//! into), it must match at the prefix delimiter so `queue/` never collects
//! `queue_priority/`, and it must give up inside its budget rather than
//! stalling a pass forever.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use stado::providers::local::disk_cleanup::live_job_ids_within;
use stado::queue::{BlobBackend, BlobInfo, JobStorage, LocalBackend, StorageError, VersionedText};

/// A generous budget: these tests are about what is NOT fetched, so the budget
/// must never be what makes them pass.
const AMPLE: Duration = Duration::from_secs(30);

/// A backend that counts document downloads and can be made to stall.
///
/// Everything else is delegated to the real `LocalBackend`, so the listing
/// semantics under test are the product's own and not a stub's.
struct Counting {
    inner: LocalBackend,
    downloads: Arc<AtomicUsize>,
    stall: Option<Duration>,
}

impl Counting {
    fn new(root: &std::path::Path, stall: Option<Duration>) -> (Arc<Self>, Arc<AtomicUsize>) {
        let downloads = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(Self {
            inner: LocalBackend::new(root.to_str().expect("temp root is utf-8"))
                .expect("local backend roots at the temp dir"),
            downloads: Arc::clone(&downloads),
            stall,
        });
        (backend, downloads)
    }
}

#[async_trait]
impl BlobBackend for Counting {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.inner.upload_text(path, content).await
    }
    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        self.inner.upload_bytes(path, content).await
    }
    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        self.downloads.fetch_add(1, Ordering::Relaxed);
        self.inner.download_text(path).await
    }
    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.downloads.fetch_add(1, Ordering::Relaxed);
        self.inner.download_bytes(path).await
    }
    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        self.downloads.fetch_add(1, Ordering::Relaxed);
        self.inner.download_text_versioned(path).await
    }
    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.inner.upload_text_if_absent(path, content).await
    }
    async fn upload_file_if_absent(
        &self,
        path: &str,
        source: &std::path::Path,
    ) -> Result<bool, StorageError> {
        self.inner.upload_file_if_absent(path, source).await
    }
    async fn download_to_filename(
        &self,
        path: &str,
        destination: &std::path::Path,
    ) -> Result<bool, StorageError> {
        self.downloads.fetch_add(1, Ordering::Relaxed);
        self.inner.download_to_filename(path, destination).await
    }
    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.inner
            .compare_and_swap_text(path, expected_version, content)
            .await
    }
    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        self.inner.delete(path).await
    }
    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        self.inner.exists(path).await
    }
    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        if let Some(stall) = self.stall {
            tokio::time::sleep(stall).await;
        }
        self.inner.list_paths(prefix, oldest_first).await
    }
    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        self.inner.updated_at(path).await
    }
    async fn set_metadata(
        &self,
        path: &str,
        metadata: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        self.inner.set_metadata(path, metadata).await
    }
    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        self.inner.list_blobs_with_meta(prefix).await
    }
}

/// A job document at `{prefix}/{job_id}.json`, written the way the store
/// writes one.
fn job_body(job_id: &str, state: &str) -> String {
    serde_json::json!({
        "job_id": job_id,
        "state": state,
        "command": "true",
        "created_at": "2026-09-03T00:00:00+00:00",
    })
    .to_string()
}

struct Fleet {
    _dir: tempfile::TempDir,
    store: JobStorage,
    downloads: Arc<AtomicUsize>,
}

async fn fleet(stall: Option<Duration>) -> Fleet {
    let dir = tempfile::tempdir().expect("temp store root");
    let (backend, downloads) = Counting::new(dir.path(), stall);
    let store = JobStorage::with_backend(backend, "local");
    Fleet {
        _dir: dir,
        store,
        downloads,
    }
}

/// The whole cost model: ids come from names, so the number of document
/// downloads is zero however many jobs the prefixes hold.
#[tokio::test]
async fn the_keep_list_downloads_no_job_documents() {
    let fleet = fleet(None).await;
    let mut expected = Vec::new();
    for index in 0..64 {
        let queued = format!("job-q{index:04x}");
        let running = format!("job-r{index:04x}");
        fleet
            .store
            .upload_text(
                &format!("queue/{queued}.json"),
                &job_body(&queued, "queued"),
            )
            .await
            .expect("seed queued job");
        fleet
            .store
            .upload_text(
                &format!("running/{running}.json"),
                &job_body(&running, "running"),
            )
            .await
            .expect("seed running job");
        expected.push(queued);
        expected.push(running);
    }
    fleet.downloads.store(0, Ordering::Relaxed);

    let ids = live_job_ids_within(&fleet.store, AMPLE)
        .await
        .expect("a readable store yields a keep-list");

    let mut found = ids.clone();
    found.sort();
    expected.sort();
    assert_eq!(found, expected, "every seeded job id must be on the list");
    assert_eq!(
        fleet.downloads.load(Ordering::Relaxed),
        0,
        "the keep-list must not download a single job document"
    );
}

/// The keep-list must be a SUPERSET of the live jobs. A job whose blob is
/// mid-transition is still a job with a live workdir, and `list_jobs` — the
/// function this used to call — drops exactly those.
#[tokio::test]
async fn a_job_mid_transition_keeps_its_place_on_the_list() {
    let fleet = fleet(None).await;
    fleet
        .store
        .upload_text(
            "running/job-fenced.json",
            &job_body("job-fenced", "transition-fence-abc"),
        )
        .await
        .expect("seed a mid-transition job");

    let ids = live_job_ids_within(&fleet.store, AMPLE)
        .await
        .expect("a readable store yields a keep-list");

    assert!(
        ids.iter().any(|id| id == "job-fenced"),
        "a mid-transition job must stay on the keep-list: {ids:?}"
    );
}

/// `queue/` is a string prefix of `queue_priority/` and the store answers
/// `starts_with`. A keep-list that collects the priority index pays for the
/// whole index and reports ids that are not job ids.
#[tokio::test]
async fn the_priority_index_is_not_mistaken_for_the_queue() {
    let fleet = fleet(None).await;
    fleet
        .store
        .upload_text("queue/job-real.json", &job_body("job-real", "queued"))
        .await
        .expect("seed queued job");
    fleet
        .store
        .upload_text(
            "queue_priority/99999999-2026-job-real.json",
            "{\"job_id\": \"job-real\", \"priority\": 0}",
        )
        .await
        .expect("seed priority marker");

    let ids = live_job_ids_within(&fleet.store, AMPLE)
        .await
        .expect("a readable store yields a keep-list");

    assert_eq!(
        ids,
        vec!["job-real".to_string()],
        "only the queue blob's id belongs on the list"
    );
}

/// The bound. An unreadable keep-list is already modelled — `None`, and
/// `queue_workdirs` then deletes nothing — so a store that will not answer
/// must expire inside the budget instead of holding the pass open.
#[tokio::test]
async fn a_stalled_store_expires_inside_its_budget() {
    let fleet = fleet(Some(Duration::from_secs(600))).await;
    let started = std::time::Instant::now();

    let ids = live_job_ids_within(&fleet.store, Duration::from_millis(120)).await;

    assert!(
        ids.is_none(),
        "a keep-list that could not be built must be None, never a partial list"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the read must give up on its budget, not on the transport: took {:?}",
        started.elapsed()
    );
}
