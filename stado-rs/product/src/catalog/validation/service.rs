//! A catalog product's `service` declaration held to its rules: an
//! installable program, its environment and scopes, no port and no unit
//! names but its own one unit.

use super::super::text;
use crate::common::slug;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashSet;

/// Every product's one service unit is this prefix and the product id.
const PRODUCT_UNIT_PREFIX: &str = "com.wisent.";

/// Hold product `id`'s `service` declaration to its rules; `kinds` are the
/// surfaces the product declares.
pub(super) fn validate(id: &str, service: &Value, kinds: &HashSet<&str>) -> Result<()> {
    if !kinds.contains("service") {
        bail!("{id}: service declaration without service surface");
    }
    if service["installable"] == true {
        text(service, "program")?;
        text(service, "summary")?;
        service["args"]
            .as_array()
            .context("service.args must be a list")?;
        if let Some(env) = service.get("env") {
            for (key, value) in env.as_object().context("service.env must be an object")? {
                if key.is_empty()
                    || key.as_bytes()[0].is_ascii_digit()
                    || !key
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                    || value
                        .as_str()
                        .is_none_or(|s| s.is_empty() || s.contains('\n'))
                {
                    bail!("{id}.service.env.{key}: expected an environment name and one non-empty line");
                }
            }
        }
        // The installed release's scope catalog: a file under the
        // account's home, because it is copied and registered there.
        if let Some(scopes) = service.get("acquisition_scopes") {
            let path = scopes
                .as_str()
                .context("service.acquisition_scopes must be a string")?;
            if !path.starts_with("$HOME/")
                || path.ends_with('/')
                || path.contains("/../")
                || path.contains('\n')
            {
                bail!("{id}.service.acquisition_scopes: {path:?} is not a file path under $HOME");
            }
        }
        // A port is a fact about one host, recorded in the service
        // directory and handed out by that host when none is; the
        // catalog names the directory key, never a number.
        if service.get("listen_port").is_some() {
            bail!(
                "{id}.service.listen_port: the catalog does not choose ports; `service \
                         ensure` reads the port from the service directory or has the host hand \
                         out a free one, so remove listen_port"
            );
        }
        if let Some(key) = service.get("directory_service") {
            let key = key
                .as_str()
                .filter(|key| !key.trim().is_empty() && !key.contains('\n'))
                .with_context(|| {
                    format!("{id}.service.directory_service must name one directory key")
                })?;
            slug(key)?;
        }
    }
    // One service per repository, named for it: a product runs one
    // service per host, labelled com.wisent.<product>.
    if let Some(unit) = service.get("unit") {
        let unit = unit.as_str().context("service.unit must be a string")?;
        let expected = format!("{PRODUCT_UNIT_PREFIX}{id}");
        if unit != expected {
            bail!(
                "{id}.service.unit: {unit} is not {expected}; a product runs one service \
                         named com.wisent.<product>"
            );
        }
    }
    // The units a product's process replaced are found on each host
    // from what they run, so the catalog names none.
    for key in ["retired_units", "role_units"] {
        if service.get(key).is_some() {
            bail!(
                "{id}.service.{key}: the catalog does not list unit names; the units a \
                         product's one process replaced are found on each host from the program \
                         they run, so remove {key}"
            );
        }
    }
    Ok(())
}
