//! Control planes: coordinator tick loop + dashboard (+ in-process agent
//! for the local variant). Port of `stado/deploy/local_control_plane.py`
//! and `stado/deploy/cloud_control_plane.py`.
//!
//! Python runs the tick loop and the agent on daemon threads and blocks the
//! main thread in `dashboard.serve`; here the daemons get their own OS
//! thread + current-thread tokio runtime and the dashboard's accept loop
//! runs in the foreground — a dashboard failure ends the process either
//! way. (Dedicated threads instead of `tokio::spawn`: the tick chain's
//! `&dyn Fn(&str)` log parameters make its futures non-Send, which
//! `tokio::spawn` cannot accept.)

use std::collections::BTreeMap;
use std::time::Duration;

use crate::coordinator::{resolve_providers, run_tick};
use crate::dashboard::{Dashboard, DashboardError};
use crate::providers::local::agent::run_agent;
use crate::queue::{JobStorage, StorageError};

/// Control-plane startup/serve failure. The "local backend required" case
/// is Python's `RuntimeError`.
#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    #[error(transparent)]
    Dashboard(#[from] DashboardError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

/// Python `local_control_plane._log`.
fn local_log(msg: &str) {
    eprintln!("[local-control-plane] {msg}");
}

/// Python `cloud_control_plane._log`.
fn cloud_log(msg: &str) {
    eprintln!("[cloud-control-plane] {msg}");
}

fn checked_port(port: i64) -> Result<u16, ControlPlaneError> {
    u16::try_from(port).map_err(|_| ControlPlaneError::Other(format!("port out of range: {port}")))
}

/// Spawn `make_future()` on a daemon thread with its own current-thread
/// runtime (Python `threading.Thread(daemon=True, name=...)`). The future
/// is constructed INSIDE the thread, so non-Send futures (the tick chain's
/// `&dyn Fn(&str)` loggers) never cross a thread boundary.
fn spawn_daemon<F>(
    name: &str,
    make_future: impl FnOnce() -> F + Send + 'static,
) -> std::io::Result<()>
where
    F: std::future::Future<Output = ()> + 'static,
{
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("daemon runtime");
            runtime.block_on(make_future());
        })?;
    Ok(())
}

/// The coordinator tick daemon (Python `coordinator_loop` in both control
/// planes): tick, log, sleep; failures are logged and the loop continues —
/// the dashboard stays available for diagnosis. Providers are re-resolved
/// every iteration, exactly like `coordinator::run`.
async fn coordinator_loop(
    store: JobStorage,
    secrets: BTreeMap<String, String>,
    sleep_seconds: u64,
    with_billing: bool,
    log: fn(&str),
) {
    loop {
        let providers = resolve_providers();
        match run_tick(&store, &secrets, &providers, with_billing, &|msg: &str| {
            log(msg)
        })
        .await
        {
            Ok(scheduled) => log(&format!("tick scheduled={scheduled}")),
            Err(exc) => log(&format!("tick failed: {exc}")),
        }
        // Native-build poller: watch registry build recipes for new commits
        // and enqueue build jobs. Self-rate-limited to one pass per minute;
        // per-recipe failures are logged inside, never raised.
        crate::scheduler::builds::poll_build_recipes(&|msg: &str| log(msg)).await;
        match crate::queue::copy::replicate_configured_backup().await {
            Ok(Some(report)) if report.is_clean() => log("disaster-recovery replication clean"),
            Ok(Some(report)) => log(&format!(
                "disaster-recovery replication incomplete: {} object(s) failed",
                report.failed()
            )),
            Ok(None) => {}
            Err(exc) => log(&format!("disaster-recovery replication failed: {exc}")),
        }
        tokio::time::sleep(Duration::from_secs(sleep_seconds)).await;
    }
}

/// Single-device Stado control plane used by desktop onboarding (Python
/// `deploy.local_control_plane.run`): API listener, scheduler, and worker on
/// this device.
pub async fn run_local(host: &str, port: i64, interval: i64) -> Result<(), ControlPlaneError> {
    let store = JobStorage::new().await?;
    // The local storage backend is intentionally loopback-only. Selecting a
    // cloud target is required before workers on other devices can join.
    let local_storage = crate::capabilities::storage_adapter(store.backend_name())
        == Some(crate::capabilities::StorageAdapter::Local);
    if !local_storage {
        return Err(ControlPlaneError::Other(
            "local-control-plane requires WC_STORAGE_BACKEND=local".to_string(),
        ));
    }
    let tick_store = store.clone();
    spawn_daemon("stado-local-coordinator", move || {
        coordinator_loop(
            tick_store,
            BTreeMap::new(),
            interval.max(5) as u64,
            false,
            local_log,
        )
    })?;
    spawn_daemon("stado-local-agent", || async {
        // Python: threading.Thread(target=run_agent, kwargs={"kind": "local"}).
        if let Err(exc) = run_agent("", false, "local").await {
            local_log(&format!("agent exited: {exc}"));
        }
    })?;
    local_log(&format!("dashboard=http://{host}:{port}"));
    Dashboard::new(store)
        .serve_with(host, checked_port(port)?)
        .await?;
    Ok(())
}

/// Cloud-hosted Stado coordinator and authenticated API listener (Python
/// `deploy.cloud_control_plane.run`).
pub async fn run_cloud(host: &str, port: i64, interval: i64) -> Result<(), ControlPlaneError> {
    let store = JobStorage::new().await?;
    let secrets = crate::coordinator::secrets_from_skarbiec()
        .await
        .map_err(|err| ControlPlaneError::Other(err.to_string()))?;
    let tick_store = store.clone();
    spawn_daemon("stado-cloud-coordinator", move || {
        coordinator_loop(
            tick_store,
            secrets,
            interval.max(15) as u64,
            true,
            cloud_log,
        )
    })?;
    cloud_log(&format!(
        "dashboard={host}:{port} storage={}",
        store.backend_name()
    ));
    Dashboard::new(store)
        .serve_with(host, checked_port(port)?)
        .await?;
    Ok(())
}
