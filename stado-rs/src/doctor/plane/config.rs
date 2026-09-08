//! The resolved deployment configuration, as one preflight row.

use crate::config;
use crate::config_file;
use crate::doctor::{Check, Findings, Status};
use crate::queue::copy::Endpoint;

// ---------------------------------------------------------------------------
// 1. Config
// ---------------------------------------------------------------------------

pub(in crate::doctor) const CONFIG_ID: &str = "config";
pub(in crate::doctor) const CONFIG_TITLE: &str = "Config";
pub(in crate::doctor) const CONFIG_REMEDY: &str =
    "set provider preference, explicit provider fences and storage locators in the Stado \
     deployment config; `stado config show` prints the resolved set";

/// Resolved backend, providers, storage locator and the config file
/// actually in use.
pub(in crate::doctor) fn check_config() -> Check {
    let backend = config::wc_storage_backend();
    let mut findings = Findings::default();

    let endpoint = Endpoint::configured_primary();
    let locator = endpoint.describe();
    let config_file = match config_file::config_path() {
        Ok(Some(path)) => path.display().to_string(),
        Ok(None) => "none (env and built-in defaults only)".to_string(),
        Err(err) => format!("unreadable: {err}"),
    };
    findings.note(
        Status::Pass,
        format!(
            "backend={backend} {locator} providers=[{}] disabled=[{}] config_file={config_file}",
            config::wc_providers().join(","),
            config::wc_disabled_providers().join(",")
        ),
    );

    if crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backend,
    )
    .is_none()
    {
        findings.note(
            Status::Fail,
            format!("WC_STORAGE_BACKEND={backend:?} is not a backend this build can construct"),
        );
        let choices =
            crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Storage)
                .collect::<Vec<_>>()
                .join(", ");
        findings.remedy(format!("set storage.backend to one of {choices} in config"));
    }

    if let Some(variant) = crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backend,
    ) {
        for field in variant.config.iter().filter(|field| field.required) {
            if endpoint.locator_value(field.key).is_none_or(str::is_empty) {
                findings.note(
                    Status::Fail,
                    format!(
                        "{} is unresolved while {:?} primary storage is selected",
                        field.env, variant.id
                    ),
                );
                findings.remedy(format!(
                    "set {} in the Stado deployment config ({})",
                    field.path, field.env
                ));
            }
        }
    }

    if config::wc_providers().is_empty() {
        findings.note(
            Status::Fail,
            "WC_PROVIDERS resolved to an empty list".to_string(),
        );
        findings.remedy("configure at least one unfenced provider in preferred order");
    }

    findings.into_check(CONFIG_ID, CONFIG_TITLE, CONFIG_REMEDY)
}
