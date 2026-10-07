//! The bundled coordinator tick loop `stado serve --control-plane` runs:
//! schedule, replicate, and go again at the declared period.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::coordinator::{resolve_providers, run_tick};
use crate::queue::JobStorage;

/// A bundled coordinator that cannot start, with the reason: a declaration
/// it cannot run with, or the vault refusing the secrets a cloud coordinator
/// reads.
#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Vault(#[from] crate::skarbiec::SkarbiecError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CoordinatorMode {
    Local,
    Cloud,
}

/// Startup state crosses the supervisor thread boundary, not a non-Send future.
pub(crate) struct ResidentCoordinator {
    store: JobStorage,
    secrets: BTreeMap<String, String>,
    sleep_seconds: u64,
    with_billing: bool,
    log: fn(&str),
}

impl ResidentCoordinator {
    pub(crate) async fn prepare(
        mode: CoordinatorMode,
        store: JobStorage,
        interval: i64,
    ) -> Result<Self, ControlPlaneError> {
        // The interval the operator declared is the tick period, used as
        // given; only a period that cannot be one is refused.
        let interval = u64::try_from(interval)
            .ok()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| {
                ControlPlaneError::Config(format!(
                    "control-plane interval {interval} is not a positive number of seconds"
                ))
            })?;
        let (secrets, with_billing, log): (_, _, fn(&str)) = match mode {
            CoordinatorMode::Local => {
                if crate::capabilities::storage_adapter(store.backend_name())
                    != Some(crate::capabilities::StorageAdapter::Local)
                {
                    return Err(ControlPlaneError::Config(
                        "serve --control-plane local requires WC_STORAGE_BACKEND=local".to_string(),
                    ));
                }
                (BTreeMap::new(), false, local_log)
            }
            CoordinatorMode::Cloud => {
                let secrets = crate::coordinator::secrets_from_skarbiec().await?;
                (secrets, true, cloud_log)
            }
        };
        Ok(Self {
            store,
            secrets,
            sleep_seconds: interval,
            with_billing,
            log,
        })
    }

    pub(crate) async fn run(self) {
        coordinator_loop(
            self.store,
            self.secrets,
            self.sleep_seconds,
            self.with_billing,
            self.log,
        )
        .await;
    }
}

fn local_log(msg: &str) {
    eprintln!("[control-plane local] {msg}");
}

fn cloud_log(msg: &str) {
    eprintln!("[control-plane cloud] {msg}");
}

/// The coordinator tick: schedule, log, replicate the configured backup,
/// then wait out the declared period. Failures are logged and the loop
/// continues, so the API the same process serves stays available for
/// diagnosis. Providers are re-resolved every iteration.
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
