//! `stado artifact ...` — port of the artifact command group in
//! `stado/cli.py` (import/list/show/resolve/publish/alias/verify/lineage).
//!
//! Output formats match the click implementation: `--json` is
//! `json.dumps(value, indent=2, sort_keys=True)` except `resolve` and
//! `alias set`, which use Python's default (", " / ": ") separators;
//! errors surface as click `ClickException`s ("Error: {CODE}: {message}",
//! exit 1), and a failed `verify` exits 1 after printing the report.

use std::path::Path;

use serde_json::{Map, Value};

use crate::artifacts::adapters::build_activation_manifest;
use crate::artifacts::registry::{ArtifactRegistry, RegistryError};
use crate::artifacts_models::{ArtifactManifest, ArtifactRef};

use super::{ArtifactAliasCommands, ArtifactCommands, ArtifactImportCommands, CmdError};

/// Python `_artifact_call`: ArtifactError → `Error: {code}: {message}`
/// (exit 1); storage failures print their bare message.
fn artifact_error(exc: RegistryError) -> CmdError {
    match exc {
        RegistryError::Artifact(err) => CmdError::click(format!("{}: {}", err.code, err.message)),
        RegistryError::Storage(err) => CmdError::click(err.to_string()),
    }
}

impl From<RegistryError> for CmdError {
    fn from(exc: RegistryError) -> Self {
        artifact_error(exc)
    }
}

impl From<crate::artifacts_models::ArtifactError> for CmdError {
    fn from(err: crate::artifacts_models::ArtifactError) -> Self {
        artifact_error(err.into())
    }
}

async fn registry() -> Result<ArtifactRegistry, CmdError> {
    ArtifactRegistry::new()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}

/// Recursively sort object keys (Python `sort_keys=True`).
fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let btree: std::collections::BTreeMap<String, Value> = map
                .iter()
                .map(|(key, value)| (key.clone(), sorted(value)))
                .collect();
            Value::Object(btree.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Python `json.dumps(value, indent=2, sort_keys=True)` (ensure_ascii).
fn json_pretty_sorted(value: &Value) -> String {
    let pretty =
        serde_json::to_string_pretty(&sorted(value)).expect("JSON serialization is infallible");
    crate::models::ensure_ascii(&pretty)
}

/// Python `json.dumps(value, sort_keys=True)` — default separators
/// (", " / ": "), ensure_ascii.
fn json_sorted(value: &Value) -> String {
    crate::queue::python_json_dumps(&sorted(value)).expect("JSON serialization is infallible")
}

fn parse_ref(value: &str) -> Result<ArtifactRef, CmdError> {
    Ok(ArtifactRef::parse(value)?)
}

/// Python `_artifact_labels`: KEY=VALUE pairs, preserving the raw strings.
fn parse_labels(values: &[String]) -> Result<Vec<(String, String)>, CmdError> {
    let mut labels = Vec::new();
    for value in values {
        let Some((key, item)) = value.split_once('=') else {
            return Err(CmdError::click(format!(
                "label must be KEY=VALUE: '{value}'"
            )));
        };
        if key.is_empty() {
            return Err(CmdError::click("label key cannot be empty"));
        }
        labels.push((key.to_string(), item.to_string()));
    }
    Ok(labels)
}

pub(super) async fn dispatch(sub: ArtifactCommands) -> Result<(), CmdError> {
    match sub {
        ArtifactCommands::Import(ArtifactImportCommands::Activations {
            repo,
            revision,
            desired_state_dir,
            run_id,
            job_ids,
            version,
            alias,
            full,
            json,
        }) => {
            import_activations(
                &repo,
                &revision,
                &desired_state_dir,
                &run_id,
                &job_ids,
                &version,
                &alias,
                full,
                json,
            )
            .await
        }
        ArtifactCommands::List {
            type_name,
            namespace,
            name,
            label,
            json,
        } => list(&type_name, &namespace, &name, &label, json).await,
        ArtifactCommands::Show { r#ref, json } => show(&r#ref, json).await,
        ArtifactCommands::Resolve { r#ref, json } => resolve(&r#ref, json).await,
        ArtifactCommands::Publish {
            manifest_path,
            verify: _,
            no_verify,
            full,
            json,
        } => {
            // Python default is --verify; --no-verify flips it off.
            publish(&manifest_path, !no_verify, full, json).await
        }
        ArtifactCommands::Alias(ArtifactAliasCommands::Set {
            target_ref,
            alias,
            expected_previous,
            json,
        }) => alias_set(&target_ref, &alias, expected_previous.as_deref(), json).await,
        ArtifactCommands::Verify { r#ref, full, json } => verify(&r#ref, full, json).await,
        ArtifactCommands::Lineage { r#ref, json } => lineage(&r#ref, json).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn import_activations(
    repo: &str,
    revision: &str,
    desired_state_dir: &str,
    run_id: &str,
    job_ids: &[String],
    version: &str,
    aliases: &[String],
    full: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // click.Path(exists=True, file_okay=False): usage error, exit 2.
    let dir = Path::new(desired_state_dir);
    if !dir.is_dir() {
        return Err(CmdError {
            message: Some(format!(
                "Invalid value for '--desired-state-dir': Directory '{desired_state_dir}' does not exist."
            )),
            code: 2,
        });
    }
    let manifest = build_activation_manifest(repo, revision, dir, run_id, job_ids, version)
        .map_err(CmdError::click)?;
    let registry = registry().await?;
    let published = registry.publish(&manifest, true, full).await?;
    let mut alias_refs = Vec::with_capacity(aliases.len());
    for alias in aliases {
        let alias_ref = registry.set_alias(&published.ref_, alias, None, "").await?;
        alias_refs.push(alias_ref.to_string());
    }
    if as_json {
        let mut value = published.to_dict();
        value
            .as_object_mut()
            .expect("to_dict is an object")
            .insert("aliases_created".into(), Value::from(alias_refs));
        println!("{}", json_pretty_sorted(&value));
    } else {
        println!("{}", published.ref_);
        for alias_ref in &alias_refs {
            println!("{alias_ref} -> {}", published.ref_);
        }
    }
    Ok(())
}

async fn list(
    type_name: &str,
    namespace: &str,
    name: &str,
    label: &[String],
    as_json: bool,
) -> Result<(), CmdError> {
    let registry = registry().await?;
    let manifests = registry
        .list(type_name, namespace, name, &parse_labels(label)?)
        .await?;
    if as_json {
        let items: Vec<Value> = manifests.iter().map(ArtifactManifest::to_dict).collect();
        println!("{}", json_pretty_sorted(&Value::Array(items)));
        return Ok(());
    }
    if manifests.is_empty() {
        println!("(no artifacts found)");
        return Ok(());
    }
    println!("{:<76} {:<20} {:<8} ALIASES", "REF", "CREATED", "VERIFY");
    for manifest in &manifests {
        let aliases = registry.aliases_for(&manifest.ref_).await?;
        let aliases = if aliases.is_empty() {
            "-".to_string()
        } else {
            aliases.join(",")
        };
        let ref_str: String = manifest.ref_.to_string().chars().take(75).collect();
        let created: String = manifest.created_at.chars().take(19).collect();
        let result = if manifest.verification.result.is_empty() {
            "-"
        } else {
            manifest.verification.result.as_str()
        };
        println!("{ref_str:<76} {created:<20} {result:<8} {aliases}");
    }
    Ok(())
}

async fn show(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
    let registry = registry().await?;
    let manifest = registry.resolve_manifest(&parse_ref(r#ref)?).await?;
    let aliases = registry.aliases_for(&manifest.ref_).await?;
    if as_json {
        let mut value = manifest.to_dict();
        let map = value.as_object_mut().expect("to_dict is an object");
        map.insert(
            "aliases".into(),
            Value::Array(aliases.into_iter().map(Value::from).collect()),
        );
        map.insert("requested_ref".into(), Value::from(r#ref));
        println!("{}", json_pretty_sorted(&value));
        return Ok(());
    }
    println!("Artifact:     {}", manifest.ref_.coordinate());
    println!("Version:      {}", manifest.ref_.version);
    println!(
        "Aliases:      {}",
        if aliases.is_empty() {
            "-".to_string()
        } else {
            aliases.join(", ")
        }
    );
    println!("Title:        {}", manifest.title);
    let result = if manifest.verification.result.is_empty() {
        "-"
    } else {
        manifest.verification.result.as_str()
    };
    println!("Verification: {result}");
    for location in &manifest.locations {
        println!("Location:     [{}] {}", location.role, location.uri);
    }
    if !manifest.producer.run_id.is_empty() {
        println!("Run:          {}", manifest.producer.run_id);
    }
    if !manifest.summary.is_empty() {
        println!("Summary:");
        println!(
            "{}",
            json_pretty_sorted(&Value::Object(manifest.summary.clone()))
        );
    }
    Ok(())
}

async fn resolve(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
    let registry = registry().await?;
    let resolved = registry.resolve(&parse_ref(r#ref)?).await?;
    if as_json {
        let value = Value::Object(Map::from_iter([
            ("requested_ref".into(), Value::from(r#ref)),
            ("resolved_ref".into(), Value::from(resolved.to_string())),
        ]));
        println!("{}", json_sorted(&value));
    } else {
        println!("{resolved}");
    }
    Ok(())
}

/// A release is a claim about the fleet, so it cannot rest on a store only one
/// machine can read.
///
/// `stado://` resolves through whichever object store this host is configured
/// with, and the default is a directory on this disk. Publishing a fleet
/// coordinate backed by that store does not fail -- it succeeds, and produces a
/// version every other machine reports as absent. The store is the operator's
/// choice and stays that way; what is refused here is only the combination of a
/// fleet coordinate with a store that cannot answer for the fleet.
fn fleet_visible(manifest: &ArtifactManifest) -> Result<(), CmdError> {
    let fleet_scheme = manifest
        .locations
        .iter()
        .any(|location| location.uri.starts_with("stado://"));
    if !fleet_scheme {
        return Ok(());
    }
    let backend = crate::config::wc_storage_backend();
    // Ask the store how far it carries. Every storage adapter declares this, so
    // a backend added later is classified by whoever adds it rather than by
    // whether its name happens to be matched here.
    let reach = crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backend,
    )
    .and_then(|variant| match variant.adapter {
        crate::capabilities::RuntimeAdapter::Storage(adapter) => Some(adapter.reach()),
        _ => None,
    });
    match reach {
        Some(crate::capabilities::StorageReach::Fleet) => Ok(()),
        Some(crate::capabilities::StorageReach::Device) => Err(CmdError::click(format!(
            "{} publishes a stado:// coordinate while this host's object store is {backend:?}, \
             which answers only for this machine: every other host would report the release \
             absent. Select a store that answers for the fleet, or give the manifest a \
             location the fleet can already reach.",
            manifest.ref_
        ))),
        None => Err(CmdError::click(format!(
            "{} publishes a stado:// coordinate, and this host's object store {backend:?} is \
             not a storage backend this build knows, so how far it carries cannot be \
             established",
            manifest.ref_
        ))),
    }
}

async fn publish(
    manifest_path: &str,
    verify: bool,
    full: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // click.Path(exists=True, dir_okay=False): usage error, exit 2.
    let path = Path::new(manifest_path);
    if !path.is_file() {
        return Err(CmdError {
            message: Some(format!(
                "Invalid value for 'MANIFEST_PATH': File '{manifest_path}' does not exist."
            )),
            code: 2,
        });
    }
    let manifest = ArtifactManifest::from_json(&std::fs::read_to_string(path)?)?;
    fleet_visible(&manifest)?;
    let registry = registry().await?;
    let published = registry.publish(&manifest, verify, full).await?;
    if as_json {
        println!("{}", json_pretty_sorted(&published.to_dict()));
    } else {
        println!("{}", published.ref_);
    }
    Ok(())
}

async fn alias_set(
    target_ref: &str,
    alias: &str,
    expected_previous: Option<&str>,
    as_json: bool,
) -> Result<(), CmdError> {
    let registry = registry().await?;
    let alias_ref = registry
        .set_alias(&parse_ref(target_ref)?, alias, expected_previous, "")
        .await?;
    let resolved = registry.resolve(&alias_ref).await?;
    if as_json {
        let value = Value::Object(Map::from_iter([
            ("alias_ref".into(), Value::from(alias_ref.to_string())),
            ("resolved_ref".into(), Value::from(resolved.to_string())),
        ]));
        println!("{}", json_sorted(&value));
    } else {
        println!("{alias_ref} -> {resolved}");
    }
    Ok(())
}

async fn verify(r#ref: &str, full: bool, as_json: bool) -> Result<(), CmdError> {
    let registry = registry().await?;
    let report = registry.verify(&parse_ref(r#ref)?, full).await?;
    if as_json {
        let value = serde_json::to_value(&report)?;
        println!("{}", json_pretty_sorted(&value));
    } else {
        println!(
            "{} ({})",
            if report.passed { "PASSED" } else { "FAILED" },
            report.adapter
        );
        for issue in &report.issues {
            println!("- {issue}");
        }
        if !report.summary.is_empty() {
            println!(
                "{}",
                json_pretty_sorted(&Value::Object(report.summary.clone()))
            );
        }
    }
    if !report.passed {
        return Err(CmdError::silent(1));
    }
    Ok(())
}

async fn lineage(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
    let registry = registry().await?;
    let manifest = registry.resolve_manifest(&parse_ref(r#ref)?).await?;
    let aliases = registry.aliases_for(&manifest.ref_).await?;
    let producer = Map::from_iter([
        (
            "run_id".into(),
            Value::from(manifest.producer.run_id.clone()),
        ),
        (
            "job_ids".into(),
            Value::Array(
                manifest
                    .producer
                    .job_ids
                    .iter()
                    .cloned()
                    .map(Value::from)
                    .collect(),
            ),
        ),
        ("repo".into(), Value::from(manifest.producer.repo.clone())),
        (
            "commit".into(),
            Value::from(manifest.producer.commit.clone()),
        ),
        ("host".into(), Value::from(manifest.producer.host.clone())),
    ]);
    let dependencies: Vec<Value> = manifest
        .dependencies
        .iter()
        .map(|r| Value::from(r.to_string()))
        .collect();
    let value = Value::Object(Map::from_iter([
        ("ref".into(), Value::from(manifest.ref_.to_string())),
        ("producer".into(), Value::Object(producer)),
        ("dependencies".into(), Value::Array(dependencies)),
        (
            "aliases".into(),
            Value::Array(aliases.iter().cloned().map(Value::from).collect()),
        ),
    ]));
    if as_json {
        println!("{}", json_pretty_sorted(&value));
        return Ok(());
    }
    fn or_dash(text: &str) -> &str {
        if text.is_empty() {
            "-"
        } else {
            text
        }
    }
    println!("Artifact: {}", manifest.ref_);
    println!("Run:      {}", or_dash(&manifest.producer.run_id));
    println!(
        "Jobs:     {}",
        or_dash(&manifest.producer.job_ids.join(", "))
    );
    println!(
        "Source:   {}@{}",
        or_dash(&manifest.producer.repo),
        or_dash(&manifest.producer.commit)
    );
    let inputs: Vec<String> = manifest
        .dependencies
        .iter()
        .map(ToString::to_string)
        .collect();
    println!("Inputs:   {}", or_dash(&inputs.join(", ")));
    Ok(())
}
