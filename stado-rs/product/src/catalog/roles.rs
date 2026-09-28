//! A service's `role_units`: units a role of the one process replaced where
//! the host switches that role on, retired by the same rules as
//! `retired_units` and keyed by their flag.

use super::validation::unit_label;
use anyhow::{bail, Result};
use serde_json::Value;
use std::collections::HashMap;

/// Check `service`'s role units for product `id` and record each as retired
/// by it in `retired_units`. A readiness names who proves the handoff: only
/// the resolver publishes the state a listener handoff reads
/// (`resolver-state`, `--resolver`), and only the host process's API start
/// records the takeover of the API listener's units (`api-takeover`,
/// `--api`).
pub(super) fn validate<'a>(
    id: &'a str,
    service: &'a Value,
    retired_units: &mut HashMap<&'a str, &'a str>,
) -> Result<()> {
    for role in service["role_units"].as_array().into_iter().flatten() {
        let unit = role["unit"].as_str().filter(|unit| unit_label(unit));
        let flag = role["flag"]
            .as_str()
            .filter(|flag| flag.strip_prefix("--").is_some_and(|name| !name.is_empty()));
        let (Some(unit), Some(flag)) = (unit, flag) else {
            bail!("{id}.service.role_units: expected {{unit: <label>, flag: --<argument>}}");
        };
        if role.get("readiness").is_some_and(|value| {
            !matches!(
                (value.as_str(), flag),
                (Some("resolver-state"), "--resolver") | (Some("api-takeover"), "--api")
            )
        }) {
            bail!(
                "{id}.service.role_units: {unit}: readiness is resolver-state for --resolver \
                 or api-takeover for --api"
            );
        }
        if let Some(owner) = retired_units.insert(unit, id) {
            bail!("{id}.service.role_units: {unit} is already retired by {owner}");
        }
    }
    Ok(())
}
