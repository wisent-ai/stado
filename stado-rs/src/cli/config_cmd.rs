//! `stado config SUB` — configuration lifecycle commands:
//! show | validate | init | migrate.

use serde_json::{Map, Value};

use crate::config;
use crate::config_file;

use super::CmdError;

pub fn run(sub: &str, key: Option<&str>, value: Option<&str>) -> Result<(), CmdError> {
    match sub {
        "init" => init(),
        "migrate" => migrate(),
        "validate" => validate(),
        "show" => show(),
        "set" => match (key, value) {
            (Some(key), Some(value)) => set(key, value),
            _ => Err(CmdError::click(
                "config set needs a dotted key and a value, e.g. \
                 stado config set alerts.channels '[\"resend\"]'",
            )),
        },
        other => Err(CmdError::click(format!(
            "unknown config subcommand: {other} (show|validate|init|migrate|set)"
        ))),
    }
}

/// `config set KEY VALUE`: change one dotted key in the config file.
///
/// Enabling an alert channel, pointing a URL at a live listener, or naming a
/// destination used to mean hand-editing the deployment's JSON. The document
/// is validated before it is written and the write is atomic, so a rejected
/// value leaves the running configuration exactly as it was.
fn set(key: &str, raw: &str) -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&original)?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }
    // A bare word is what an operator types for a string value; anything that
    // parses as JSON keeps its type, so lists and booleans need no quoting
    // dance.
    let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::from(raw));

    let mut cursor = &mut document;
    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| CmdError::click("config set needs a non-empty key"))?;
    for segment in parents {
        let object = cursor
            .as_object_mut()
            .ok_or_else(|| CmdError::click(format!("{key}: {segment} is not an object")))?;
        cursor = object
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let object = cursor
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("{key}: parent is not an object")))?;
    let previous = object.insert((*last).to_string(), parsed.clone());

    let problems = config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "{key} rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }

    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!(
        "{key}: {} -> {} ({})",
        previous.unwrap_or(Value::Null),
        parsed,
        path.display()
    );
    Ok(())
}

fn initialize_local_registry(home: &std::path::Path) -> Result<(), CmdError> {
    let storage_root = home.join(".stado").join("local-storage");
    std::fs::create_dir_all(&storage_root)?;
    let registry_path = storage_root.join(crate::targets::REGISTRY_BLOB);
    if registry_path.exists() {
        return Ok(());
    }

    let hostname = crate::providers::vast::system_hostname();
    let identity = crate::targets::normalize_hostname(&hostname);
    let target_name = identity
        .split('.')
        .next()
        .unwrap_or(identity.as_str())
        .to_string();
    let hostnames = if target_name == identity {
        Vec::new()
    } else {
        vec![identity]
    };
    let policy_unit = crate::providers::local::disk_cleanup::STATE_VERSION;
    let registry = serde_json::json!({
        "schema_version": crate::targets::REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [{
            "name": target_name,
            "kind": "local",
            "hostnames": hostnames,
            "slots": policy_unit,
            "disk_cleanup": {
                "mode": "off",
                "check_interval_seconds": i64::try_from(
                    crate::constants::MIN_RUNTIME_BEFORE_YIELD_S
                ).expect("cleanup interval fits i64"),
                "low_free_gb": policy_unit,
                "target_free_gb": policy_unit.saturating_add(policy_unit),
                "max_bytes_per_pass": crate::providers::local::disk_cleanup::GIB,
                "max_items_per_pass": policy_unit,
                "max_scan_items": policy_unit,
                "cleaners": {}
            }
        }]
    });
    crate::targets::validate_registry(&registry).map_err(|error| {
        CmdError::click(format!("generated local registry is invalid: {error}"))
    })?;
    let body = format!("{}\n", serde_json::to_string_pretty(&registry)?);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&registry_path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    std::io::Write::write_all(&mut file, body.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// `config init`: write the commented template to ~/.stado/config.json.
fn init() -> Result<(), CmdError> {
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let path = std::path::Path::new(&home)
        .join(".stado")
        .join("config.json");
    if path.exists() {
        return Err(CmdError::click(format!(
            "config file already exists: {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&config_file::template())?;
    initialize_local_registry(std::path::Path::new(&home))?;
    std::fs::write(&path, format!("{body}\n"))?;
    println!("{}", path.display());
    Ok(())
}

/// Add the current root schema to a legacy config while preserving the exact
/// prior document beside it. Future schemas are never rewritten or downgraded.
fn migrate() -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists to migrate"))?;
    let raw = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&raw)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("config file must contain a JSON object"))?;
    match root.get("schema_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(config_file::SCHEMA_VERSION) => {
            println!(
                "config already uses schema_version {} ({})",
                config_file::SCHEMA_VERSION,
                path.display()
            );
            return Ok(());
        }
        Some(version) => {
            return Err(CmdError::click(format!(
                "cannot migrate config schema_version {version}; this binary supports {}",
                config_file::SCHEMA_VERSION
            )));
        }
        None => {}
    }

    root.insert(
        "schema_version".to_string(),
        Value::from(config_file::SCHEMA_VERSION),
    );
    let migrated = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let backup = std::path::PathBuf::from(format!("{}.before-schema-migration", path.display()));
    let mut backup_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot preserve config at {}: {error}",
                backup.display()
            ))
        })?;
    std::io::Write::write_all(&mut backup_file, raw.as_bytes())?;

    let temporary = std::path::PathBuf::from(format!("{}.migrating", path.display()));
    std::fs::write(&temporary, migrated)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!(
        "migrated config to schema_version {}; previous file: {}",
        config_file::SCHEMA_VERSION,
        backup.display()
    );
    Ok(())
}

/// `config validate`: structural check; problems print as `ERROR ...`
/// lines and exit 1 (Python `raise SystemExit(1)`).
fn validate() -> Result<(), CmdError> {
    let data = config_file::load_config_file().map_err(|exc| CmdError::click(exc.to_string()))?;
    let problems = config_file::validate(&Value::Object(data.clone()));
    if !problems.is_empty() {
        for problem in problems {
            println!("ERROR {problem}");
        }
        return Err(CmdError::silent(1));
    }
    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    match where_ {
        Some(path) => println!("config ok ({})", path.display()),
        None => println!("config ok (defaults; no config file)"),
    }
    Ok(())
}

/// `config show`: the resolved values for the operator-facing keys.
fn show() -> Result<(), CmdError> {
    // Keys mirror cli.py exactly (lowercased constant names).
    let mut resolved = Map::new();
    resolved.insert("project".into(), Value::from(config::project()));
    resolved.insert("bucket".into(), Value::from(config::bucket()));
    resolved.insert("region".into(), Value::from(config::region()));
    resolved.insert(
        "regions".into(),
        Value::Array(
            config::regions()
                .iter()
                .map(|r| Value::from(r.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "wc_providers".into(),
        Value::Array(
            config::wc_providers()
                .iter()
                .map(|p| Value::from(p.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "wc_disabled_providers".into(),
        Value::Array(
            config::wc_disabled_providers()
                .iter()
                .map(|p| Value::from(p.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "wc_storage_backend".into(),
        Value::from(config::wc_storage_backend()),
    );
    resolved.insert(
        "wc_local_storage_path".into(),
        Value::from(config::wc_local_storage_path()),
    );
    resolved.insert(
        "wc_backup_storage_backend".into(),
        Value::from(config::wc_backup_storage_backend()),
    );
    resolved.insert(
        "wc_backup_bucket".into(),
        Value::from(config::wc_backup_bucket()),
    );
    resolved.insert(
        "wc_backup_azure_storage_account".into(),
        Value::from(config::wc_backup_azure_storage_account()),
    );
    resolved.insert(
        "wc_backup_azure_container".into(),
        Value::from(config::wc_backup_azure_container()),
    );
    resolved.insert(
        "wc_backup_s3_region".into(),
        Value::from(config::wc_backup_s3_region()),
    );
    resolved.insert(
        "wc_backup_local_storage_path".into(),
        Value::from(config::wc_backup_local_storage_path()),
    );
    resolved.insert(
        "azure_subscription_id".into(),
        Value::from(config::azure_subscription_id()),
    );
    resolved.insert(
        "azure_vm_identity_id".into(),
        Value::from(config::azure_vm_identity_id()),
    );
    resolved.insert(
        "stado_api_url".into(),
        Value::from(config::stado_api_url()),
    );
    resolved.insert(
        "stado_release_version".into(),
        Value::from(config::stado_release_version()),
    );
    resolved.insert(
        "stado_release_platform".into(),
        Value::from(config::stado_release_platform()),
    );
    resolved.insert(
        "stado_deployment_id".into(),
        Value::from(config::stado_deployment_id()),
    );
    resolved.insert(
        "object_skarbiec_url".into(),
        Value::from(config::object_skarbiec_url()),
    );
    resolved.insert(
        "object_skarbiec_consumer".into(),
        Value::from(config::object_skarbiec_consumer()),
    );
    resolved.insert(
        "object_skarbiec_token_file".into(),
        Value::from(config::object_skarbiec_token_file()),
    );
    let object_namespaces = match config::object_api_namespaces() {
        Ok(namespaces) => Value::Object(
            namespaces
                .iter()
                .map(|(namespace, policy)| {
                    (
                        namespace.clone(),
                        Value::Object(Map::from_iter([
                            ("item".into(), Value::from(policy.item())),
                            (
                                "prefix_policies".into(),
                                Value::Array(
                                    policy
                                        .prefix_policies()
                                        .iter()
                                        .map(|prefix_policy| {
                                            Value::Object(Map::from_iter([
                                                (
                                                    "prefix".into(),
                                                    Value::from(prefix_policy.prefix()),
                                                ),
                                                (
                                                    "actions".into(),
                                                    Value::Array(
                                                        prefix_policy
                                                            .actions()
                                                            .iter()
                                                            .map(|action| {
                                                                Value::from(action.as_str())
                                                            })
                                                            .collect(),
                                                    ),
                                                ),
                                            ]))
                                        })
                                        .collect(),
                                ),
                            ),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("object_api_namespaces".into(), object_namespaces);
    resolved.insert(
        "release_skarbiec_url".into(),
        Value::from(config::release_skarbiec_url()),
    );
    resolved.insert(
        "release_skarbiec_consumer".into(),
        Value::from(config::release_skarbiec_consumer()),
    );
    resolved.insert(
        "release_skarbiec_token_file".into(),
        Value::from(config::release_skarbiec_token_file()),
    );
    let release_publishers = match config::release_api_publishers() {
        Ok(publishers) => Value::Object(
            publishers
                .iter()
                .map(|(product, policy)| {
                    (
                        product.clone(),
                        Value::Object(Map::from_iter([
                            ("item".into(), Value::from(policy.item())),
                            ("prefix".into(), Value::from(policy.prefix())),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("release_api_publishers".into(), release_publishers);
    resolved.insert(
        "service_skarbiec_url".into(),
        Value::from(config::service_skarbiec_url()),
    );
    resolved.insert(
        "service_skarbiec_consumer".into(),
        Value::from(config::service_skarbiec_consumer()),
    );
    resolved.insert(
        "service_skarbiec_token_file".into(),
        Value::from(config::service_skarbiec_token_file()),
    );
    let service_deployers = match config::service_api_deployers() {
        Ok(deployers) => Value::Object(
            deployers
                .iter()
                .map(|(product, policy)| {
                    (
                        product.clone(),
                        Value::Object(Map::from_iter([
                            ("consumer".into(), Value::from(policy.consumer())),
                            ("item".into(), Value::from(policy.item())),
                            (
                                "services".into(),
                                Value::Array(
                                    policy
                                        .services()
                                        .iter()
                                        .map(|service| Value::from(service.as_str()))
                                        .collect(),
                                ),
                            ),
                            (
                                "actions".into(),
                                Value::Array(
                                    policy
                                        .actions()
                                        .iter()
                                        .map(|action| Value::from(action.as_str()))
                                        .collect(),
                                ),
                            ),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("service_api_deployers".into(), service_deployers);
    resolved.insert(
        "agent_skarbiec_url".into(),
        Value::from(config::agent_skarbiec_url()),
    );
    resolved.insert(
        "agent_skarbiec_consumer".into(),
        Value::from(config::agent_skarbiec_consumer()),
    );
    resolved.insert(
        "agent_skarbiec_token_file".into(),
        Value::from(config::agent_skarbiec_token_file()),
    );
    resolved.insert(
        "agent_skarbiec_items".into(),
        Value::Array(
            config::agent_skarbiec_items()
                .iter()
                .map(|item| Value::from(item.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "agent_skarbiec_secret_fields".into(),
        Value::Array(
            config::agent_skarbiec_secret_fields()
                .iter()
                .map(|reference| Value::from(reference.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "azure_resource_group".into(),
        Value::from(config::azure_resource_group()),
    );
    resolved.insert(
        "azure_locations".into(),
        Value::Array(
            config::azure_locations()
                .iter()
                .map(|l| Value::from(l.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "dashboard_bind".into(),
        Value::from(config::dashboard_bind()),
    );
    resolved.insert(
        "dashboard_port".into(),
        Value::from(config::dashboard_port()),
    );

    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    let mut out = Map::new();
    out.insert(
        "file".into(),
        where_
            .map(|p| Value::from(p.display().to_string()))
            .unwrap_or(Value::Null),
    );
    out.insert("resolved".into(), Value::Object(resolved));
    println!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
    Ok(())
}
