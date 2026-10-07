//! Local-machine install path for `stado bootstrap --local`.
//!
//! Picks the init system for the host OS and writes the one per-user Stado
//! unit, com.wisent.stado, running `stado serve` with every role the host
//! declares, so it persists across reboots without sudo or ssh. The install
//! reads this machine's separate Stado units (agent, coordinator, disk
//! cleanup, failure fixer, watchdog), merges them into that unit and retires
//! them ([`unit::host`]); no other kind is installed on its own. Units
//! ExecStart the release binaries in `~/.stado/bin/` (populated from the exact
//! immutable release exposed by the public Stado API, by
//! [`artifact::ensure_bins`] when missing). Job runtimes belong to the
//! submitted workload, not to the unit.
//!
//! Darwin: launchd plist at ~/Library/LaunchAgents/<label>.plist
//!         loaded with `launchctl bootstrap gui/<uid> <plist>`.
//! Linux : systemd --user unit at ~/.config/systemd/user/<name>.service
//!         enabled with `systemctl --user enable --now <name>`.
//!
//! One component per install stage: [`artifact`] reads the release archive and
//! places the binaries a unit ExecStarts, [`unit`] renders what is about to be
//! installed, and [`activation`] writes that file and loads the job into the
//! host's init system. This file holds what every stage names — the label
//! vocabulary, the host init system, the credential fetcher — and the two
//! entry points callers outside the module use.

pub mod activation;
pub mod artifact;
pub mod unit;

use std::sync::Arc;

use futures::future::BoxFuture;

use super::{DeployError, Runner};

use self::activation::commands::current_uid;
use self::activation::daemon::account_of;
use self::activation::execute_plan;
use self::artifact::{ensure_bins, Bins};
use self::unit::plan;

pub use self::unit::render::daemon_plist_text;
pub use self::unit::InstallPlan;

/// The label prefix every unit this fleet installs carries: a product's one
/// unit is `com.wisent.<product>`, as the catalog names it, and a declared
/// service outside the catalog is `com.wisent.<name>`. Earlier releases
/// minted `com.wisent.compute.<kind>.<name>` and `com.wisent.always-on.<name>`;
/// those are retired units, recognised by this prefix and removed, never
/// minted again.
pub const FLEET_LABEL_PREFIX: &str = "com.wisent.";
/// The prefix earlier releases minted onto a service name. A unit carrying
/// it is a retired one.
pub const RETIRED_COMPUTE_PREFIX: &str = "com.wisent.compute.";

/// Fetches the central HF write token (Python `_hf_write_token`);
/// injectable so tests never touch GCS.
pub type TokenFetcher = Arc<dyn Fn() -> BoxFuture<'static, Result<String, String>> + Send + Sync>;

/// The host init system (Python `platform.system()` mapped to the two
/// supported cases).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalOs {
    Darwin,
    Linux,
}

impl LocalOs {
    /// Python: Darwin → launchd, Linux → systemd --user, anything else
    /// raises `unsupported OS for local install: {platform.system()}`.
    pub fn detect() -> Result<Self, DeployError> {
        match std::env::consts::OS {
            "macos" => Ok(Self::Darwin),
            "linux" => Ok(Self::Linux),
            other => Err(DeployError(format!(
                "unsupported OS for local install: {}",
                python_os_name(other)
            ))
            .stating(crate::primitives::failure::FailureCode::Refused)),
        }
    }

    /// The `platform.system()` spelling used in the dry-run header.
    pub fn python_name(&self) -> &'static str {
        match self {
            Self::Darwin => "Darwin",
            Self::Linux => "Linux",
        }
    }
}

/// `platform.system()` spelling for the unsupported-OS error.
fn python_os_name(os: &str) -> &str {
    match os {
        "macos" => "Darwin",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    }
}

/// The one label for a declared unit: `com.wisent.<name>`, or an already-full
/// fleet label unchanged, so a name that carries the prefix is never prefixed
/// twice (a doubled label is a different unit, declared nowhere and running
/// anyway).
pub fn label(name: &str) -> String {
    if name.starts_with(FLEET_LABEL_PREFIX) {
        name.to_string()
    } else {
        format!("{FLEET_LABEL_PREFIX}{name}")
    }
}

/// The one unit Stado runs under on a host, as the catalog names it; every
/// Stado role (serve, agent, coordinator, release, health, sync) is a role of
/// this unit.
pub fn stado_unit() -> Result<String, DeployError> {
    let product = crate::deploy::service_catalog::lookup("stado")
        .map_err(|message| {
            DeployError(message).stating(crate::primitives::failure::FailureCode::Config)
        })?
        .ok_or_else(|| {
            DeployError("the service catalog does not declare Stado".to_string())
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    Ok(product.unit.unwrap_or(product.name))
}

/// The systemd unit name for a label, or an already-suffixed name unchanged.
///
/// The mirror of [`label`] one suffix over, and it is here for the same reason.
/// [`label`] stopped the fleet minting its own prefix onto a name that already
/// carried it; nothing stopped `.service` being appended to a name that already
/// ended in it, which produces a `….service.service` label in the registry
/// and on the unit path. A doubled suffix is a DIFFERENT unit name, so systemd
/// was asked for a unit nobody had written, the declaration reported `missing`
/// with `observed: never`, and the resolver was declared on a host where it had
/// never once existed.
pub fn systemd_unit(label: &str) -> String {
    if label.ends_with(SYSTEMD_SUFFIX) {
        label.to_string()
    } else {
        format!("{label}{SYSTEMD_SUFFIX}")
    }
}

/// The one suffix a `systemd --user` unit name carries.
pub const SYSTEMD_SUFFIX: &str = ".service";

/// Central write-scoped Hugging Face token from
/// `stado-huggingface/write_token` in Skarbiec. Missing credentials,
/// authorization and transport failures are explicit; there is no alternate
/// credential source.
pub async fn fetch_hf_write_token() -> Result<String, DeployError> {
    crate::skarbiec::read_string("huggingface", "write_token")
        .await
        .map_err(DeployError::from)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            DeployError("Skarbiec item stado-huggingface field write_token is required".into())
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })
}

/// Production [`TokenFetcher`] over [`fetch_hf_write_token`].
pub fn production_hf_fetcher() -> TokenFetcher {
    Arc::new(|| Box::pin(async { fetch_hf_write_token().await.map_err(|exc| exc.message) }))
}

/// [`crate::deploy::service::requires_daemon_domain`] for THIS machine, for a
/// caller that holds no registry of its own. `run_bootstrap` does hold one and
/// answers from it; loading a second document there would let two passes over
/// the same host disagree.
///
/// A registry that cannot be read falls back to the per-login domain: an
/// unreadable document is not a statement that this host is always-on, and
/// guessing `system` would put a plist where an unprivileged install cannot
/// remove it.
pub async fn this_host_requires_daemon_domain() -> bool {
    let Ok(registry) = crate::targets::load_registry_auto().await else {
        return false;
    };
    registry
        .lookup_self(&crate::providers::vast::system_hostname())
        .ok()
        .flatten()
        .is_some_and(crate::deploy::service::requires_daemon_domain)
}

/// Install a persistent local service. Credentials remain in Skarbiec; the
/// service unit receives only Skarbiec connection metadata and non-secret
/// runtime configuration.
pub async fn install_local(
    name: &str,
    kind: &str,
    dry_run: bool,
    daemon_domain: bool,
    runner: &Runner,
    _hf_fetch: &TokenFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let os = LocalOs::detect()?;
    let home = crate::config_file::expand_tilde("~");
    let daemon = if daemon_domain && os == LocalOs::Darwin {
        Some(account_of(&home)?)
    } else {
        None
    };
    let bins = Bins::resolve(&home);
    let install_plan = plan(name, kind, os, &home, &bins, "", daemon.clone())?;
    if dry_run {
        for line in install_plan.dry_run_lines() {
            echo(&line);
        }
        return Ok(());
    }
    ensure_bins(&home, echo).await?;
    if kind == "host" {
        // The host unit replaces this machine's separate Stado units; the
        // merge reads them all before anything is written.
        let component_plan = |component: &str, _label: &str| {
            plan(name, component, os, &home, &bins, "", daemon.clone())
        };
        return unit::host::install(install_plan, &home, &component_plan, runner, echo).await;
    }
    execute_plan(&install_plan, &home, current_uid(), runner, echo).await
}
