mod activation;
pub(super) use activation::Activation;

use crate::{
    catalog::text,
    common::{capture, checked, stado},
    state::ProductState,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The word a `host_config` value writes where the installed service's own
/// address goes. The service's port is the one its host handed out and the
/// service directory recorded during `service ensure`, so the address is read
/// from the directory after that, never written in the catalog.
const SERVICE_URL_PLACEHOLDER: &str = "$STADO_SERVICE_URL";

/// Reconcile a resident product's native unit through the prepared candidate
/// while the predecessor still occupies its installed path. The candidate's
/// own catalog, rather than the installed binary's old catalog, owns argv.
pub fn migrate_host_unit(
    product: &Value,
    home: &std::path::Path,
    plan: &crate::install::plan::Prepared,
) -> Result<Option<String>> {
    let id = text(product, "id")?;
    if id != "stado" {
        return Ok(None);
    }
    let label = text(&product["service"], "unit")?;
    let native = [
        home.join("Library/LaunchAgents")
            .join(format!("{label}.plist")),
        std::path::PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist")),
        home.join(".config/systemd/user")
            .join(format!("{label}.service")),
        std::path::PathBuf::from("/etc/systemd/system").join(format!("{label}.service")),
    ];
    let mut present = false;
    for path in native {
        present |= path
            .try_exists()
            .with_context(|| format!("checking native unit {}", path.display()))?;
    }
    if !present {
        return Ok(None);
    }
    let candidate = prepared_binary(plan, id)?;
    let self_host = checked(std::process::Command::new(candidate).args([
        "registry",
        "self",
        "--local",
        "--name-only",
    ]))?;
    let host = String::from_utf8(self_host.stdout)?;
    let host = host.trim();
    if host.is_empty() {
        bail!("the installed {label} unit cannot be migrated: registry self --local named no host");
    }
    let outcome = checked(std::process::Command::new(candidate).args([
        "service",
        "ensure",
        id,
        "--host",
        host,
        "--reason",
        "converge the product unit to the candidate release command line",
        "--json",
    ]))?;
    eprintln!(
        "{id}: migrated {label} on {host} before replacing its executable: {}",
        String::from_utf8_lossy(&outcome.stdout).trim()
    );
    Ok(Some(host.to_owned()))
}

fn prepared_binary<'a>(
    plan: &'a crate::install::plan::Prepared,
    product: &str,
) -> Result<&'a std::path::Path> {
    plan.placements
        .iter()
        .find(|placed| {
            !placed.symbolic
                && placed
                    .destination
                    .file_name()
                    .is_some_and(|name| name == product)
        })
        .map(|placed| placed.source.as_path())
        .with_context(|| {
            format!(
                "{product}: prepared installation has no executable to check against managed units"
            )
        })
}

/// Parse the native unit's actual arguments with the prepared executable
/// before any installed bytes are replaced. Migration above has already
/// rewritten a catalog-owned unit while the old executable still answers.
pub fn admit_units(plan: &crate::install::plan::Prepared, product: &str, host: &str) -> Result<()> {
    let candidate = prepared_binary(plan, product)?;
    let output = checked(
        std::process::Command::new(candidate)
            .args(["quality", "units", "--candidate"])
            .arg(candidate)
            .args(["--program", product, "--host", host]),
    )?;
    eprintln!(
        "{product}: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}

pub fn ensure(product: &Value, recipe: &Value, host: &str) -> Result<Value> {
    if product["service"]["installable"] != true {
        bail!(
            "{} has no installable managed-service declaration",
            text(product, "id")?
        );
    }
    let output = checked(stado().args([
        "service",
        "ensure",
        text(product, "id")?,
        "--host",
        host,
        "--reason",
        "installed from the canonical Wisent product catalog",
        "--json",
    ]))?;
    let receipt: Value = serde_json::from_slice(&output.stdout)
        .context("managed service ensure returned invalid JSON")?;
    if let Some(configuration) = recipe.get("host_config") {
        for (key, value) in configuration
            .as_object()
            .context("host_config must be an object")?
        {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            let value = if value.contains(SERVICE_URL_PLACEHOLDER) {
                value.replace(SERVICE_URL_PLACEHOLDER, &service_url(product, host)?)
            } else {
                value
            };
            checked(stado().args(["host", "config", "set", host, key, &value]))?;
        }
    }
    // `stado service ensure` already retired, on this host, every unit that
    // runs this product's program under another label and withdrew their
    // declarations in the write that recorded this unit; it fails when one
    // could not be retired.
    let observed = observe(product, host)?;
    if observed["ready"] != true {
        bail!("service activation did not establish port ownership: {observed}");
    }
    Ok(json!({"activation": receipt, "observed": observed}))
}

/// The address the service directory hands `host` for the product's service.
fn service_url(product: &Value, host: &str) -> Result<String> {
    let id = text(product, "id")?;
    let output = checked(stado().args([
        "service",
        "directory",
        "connect",
        id,
        "--target",
        host,
        "--no-verify",
        "--json",
    ]))?;
    let answer: Value = serde_json::from_slice(&output.stdout)
        .context("service directory connect returned invalid JSON")?;
    answer["url"].as_str().map(str::to_owned).with_context(|| {
        format!("the service directory gives {host} no address for {id}: {answer}")
    })
}

pub fn observe(product: &Value, host: &str) -> Result<Value> {
    let id = text(product, "id")?;
    let output = capture(stado().args(["service", "serving", id, "--host", host, "--json"]))?;
    let payload = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    let ready = output.status.success()
        && payload.as_array().is_some_and(|rows| {
            !rows.is_empty() && rows.iter().all(|row| row["serving"] == "serving")
        });
    Ok(
        json!({"ready": ready, "operation": "stado service serving", "host": host, "exit_status": output.status.code(),
        "observed": payload, "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr),
        "source_attestation": "port ownership does not establish the remote executable's source revision"}),
    )
}

pub fn remove(product: &Value, state: &ProductState, host: &str) -> Result<()> {
    if state
        .host
        .as_deref()
        .is_some_and(|recorded| recorded != host)
    {
        bail!("service removal host differs from its recorded installation");
    }
    let unit = product["service"]["unit"]
        .as_str()
        .or_else(|| {
            state
                .extra
                .get("service")
                .and_then(|v| v["activation"]["unit"].as_str())
        })
        .context("service receipt and catalog do not identify the managed unit to remove")?;
    checked(stado().args(["service", "remove", unit, "--host", host, "--json"]))?;
    Ok(())
}
