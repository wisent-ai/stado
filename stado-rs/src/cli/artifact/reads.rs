//! The artifact reads: `list`, `show`, `resolve` and `lineage`.
//!
//! None of these mutate the registry; each renders either the Python `--json`
//! payload or the aligned human table the click implementation prints.

use serde_json::{Map, Value};

use crate::artifacts_models::ArtifactManifest;

use crate::cli::CmdError;

use super::format::{json_pretty_sorted, json_sorted, parse_labels, parse_ref};
use super::registry;

pub(super) async fn list(
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

pub(super) async fn show(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
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

pub(super) async fn resolve(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
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

pub(super) async fn lineage(r#ref: &str, as_json: bool) -> Result<(), CmdError> {
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
