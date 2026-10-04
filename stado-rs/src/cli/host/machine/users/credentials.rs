use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::{ComputeTarget, Registry};

use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};

pub(crate) struct CredentialHost {
    pub target: ComputeTarget,
    pub home: String,
    pub vault: String,
    pub gnupg_home: String,
}

/// Resolve credential custody from the host's own durable Stado declaration.
///
/// The remote configuration document is the declaration every service on that
/// host consumes. An absent field is never replaced with a conventional path:
/// a plausible default is precisely how two vaults can both receive real writes.
pub(crate) async fn credential_host(target: &str) -> Result<CredentialHost, CmdError> {
    let target = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let home = crate::deploy::host_channel::remote_home(&target, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let configuration = remote_config_output(&target, RemoteConfigAction::Show, &runner).await?;
    let document: Value = serde_json::from_str(&configuration).map_err(|error| {
        CmdError::click(format!(
            "{}: the declared Stado configuration could not be read: {error}",
            target.name
        ))
    })?;
    let declared = document
        .pointer("/resolved/skarbiec_vault_file")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no vault authority; add it to secrets.skarbiec.vault_file",
                target.name
            ))
        })?;
    let vault = declared
        .strip_prefix("$HOME/")
        .map(|tail| format!("{home}/{tail}"))
        .unwrap_or_else(|| declared.to_string());
    let environment = crate::deploy::host_channel::run_command(
        &target,
        "printf '%s\n' \"${GNUPGHOME:-$HOME/.gnupg}\"",
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !environment.ok() {
        return Err(CmdError::click(format!(
            "{}: GNUPGHOME could not be resolved from the host environment",
            target.name
        )));
    }
    let gnupg_home = environment.stdout.trim().to_string();
    if gnupg_home.is_empty() {
        return Err(CmdError::click(format!(
            "{}: GNUPGHOME is empty; declare it in the host environment",
            target.name
        )));
    }
    Ok(CredentialHost {
        target,
        home,
        vault,
        gnupg_home,
    })
}

/// Resolve Skarbiec through the host's release agent, using the same observed
/// active-binary contract consumed by Weles.
///
/// A product/target with no release-control policy may still use the historical
/// `$HOME/.stado/bin/skarbiec` install. Once the policy exists, however, desired
/// state and executable files are not evidence that a release is active: a
/// quarantined candidate leaves both behind. The host's installed Stado must
/// identify and validate the exact process, proxy target, manifest identity,
/// immutable directory and executable that are actually active.
pub(crate) async fn release_managed_skarbiec(
    resolved: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
    home: &str,
) -> Result<String, CmdError> {
    let legacy = format!("{home}/.stado/bin/skarbiec");
    // The authority first, then the last-known-good copy the host channel
    // already resolves hosts from. Credential custody is how a broken bearer
    // is repaired, and the authority is served by the object API, which
    // authorizes through that same bearer: a stado bearer that no longer
    // matches its grant turns the object API into 503 'object authorization
    // unavailable', and every grant and token command then dies here, so
    // the fleet's identity could be restored only by hand on the vault host.
    let document = match crate::cli::registry::fetch_document().await {
        Ok(document) => document,
        Err(authority) => last_good_registry().map_err(|copy| {
            CmdError::click(format!(
                "the registry could not be read to find {}'s Skarbiec: authority: {authority}; \
                 last-known-good copy: {copy}",
                resolved.name
            ))
        })?,
    };
    let control = crate::release_control::control(&document).map_err(CmdError::click)?;
    let Some(control) = control else {
        return Ok(legacy);
    };
    let Some(policy) = control.products.get("skarbiec") else {
        return Ok(legacy);
    };
    let Some(target) = policy.targets.get(resolved.name.as_str()) else {
        return Ok(legacy);
    };

    let stado = format!("{home}/.stado/bin/stado");
    let invocation = [
        stado.as_str(),
        "release",
        "active-binary",
        "skarbiec",
        "--target",
        resolved.name.as_str(),
        "--json",
    ];
    let output = crate::deploy::host_channel::run_program(resolved, &invocation, runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: release-controlled Skarbiec has no available active binary: {}",
            resolved.name,
            crate::deploy::host_channel::last_error_line(&output, "active release unavailable")
        )));
    }
    let active: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{}: Stado active-binary returned unreadable JSON: {error}",
            resolved.name
        ))
    })?;
    let field = |name: &str| {
        active
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{}: Stado active-binary omitted {name:?}",
                    resolved.name
                ))
            })
    };
    let state = field("state")?;
    let product = field("product")?;
    let active_target = field("target")?;
    let version = field("version")?;
    let platform = field("platform")?;
    let artifact_sha256 = field("artifact_sha256")?;
    let manifest_sha256 = field("manifest_sha256")?;
    let path = field("path")?;
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    };
    if state != "active"
        || product != "skarbiec"
        || active_target != resolved.name
        || platform != target.platform
        || !digest(artifact_sha256)
        || !digest(manifest_sha256)
    {
        return Err(CmdError::click(format!(
            "{}: Stado active-binary returned an invalid Skarbiec identity",
            resolved.name
        )));
    }
    let expected = crate::release_control::release_directory(policy, target, version, platform)
        .join(&policy.binary);
    if !std::path::Path::new(path).is_absolute() || std::path::Path::new(path) != expected {
        return Err(CmdError::click(format!(
            "{}: Stado active-binary returned path {path:?}, expected {}",
            resolved.name,
            expected.display()
        )));
    }
    Ok(path.to_string())
}

/// The last-known-good registry document this machine cached, as the host
/// channel resolves hosts from when the authority does not answer.
fn last_good_registry() -> Result<Value, String> {
    let path = crate::targets::registry_last_good_path()
        .ok_or_else(|| "last-known-good registry path is unavailable".to_string())?;
    let bytes =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid registry JSON: {error}", path.display()))
}

/// Where `host user create` reads the registry from. Named for what the
/// source is, not for the storage provider that happens to hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum RegistrySource {
    /// The canonical registry in whichever store `WC_STORAGE_BACKEND` selects.
    Remote,
    /// The registry file bundled with this binary.
    Local,
    /// The canonical registry, then the bundled file when the store does not answer.
    Auto,
}

pub(super) async fn load_registry_by_source(source: RegistrySource) -> Result<Registry, CmdError> {
    match source {
        RegistrySource::Remote => crate::targets::fetch_registry_remote()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
        RegistrySource::Local => {
            crate::targets::load_bundled_registry().map_err(|exc| CmdError::click(exc.to_string()))
        }
        RegistrySource::Auto => crate::targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string())),
    }
}
