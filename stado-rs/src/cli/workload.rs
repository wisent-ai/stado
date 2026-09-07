//! Declaration-driven placement for interactive and receipt-producing host work.
//!
//! Workload names, ownership, plan schemas, registry gates and report contracts
//! live in `stado-rs/data/workloads.json`. This module is the only runtime
//! reader. Adding a product workload is a declaration change, not another CLI
//! verb.

use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::LazyLock;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::CmdError;
use crate::deploy::{host_channel, ssh_key};
use crate::targets::ComputeTarget;

pub const DECLARATION_PATH: &str = "stado-rs/data/workloads.json";
const DECLARATION: &str = include_str!("../../data/workloads.json");
const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct WorkloadCatalog {
    schema_version: u64,
    workloads: Vec<WorkloadKind>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkloadKind {
    pub kind: String,
    pub product: String,
    pub interactive: bool,
    pub registry_allowance: Option<String>,
    pub plan_schema: Option<String>,
    pub report: Vec<String>,
}

static CATALOG: LazyLock<Result<WorkloadCatalog, String>> = LazyLock::new(|| {
    let parsed: WorkloadCatalog = serde_json::from_str(DECLARATION)
        .map_err(|error| format!("{DECLARATION_PATH} is not readable JSON: {error}"))?;
    if parsed.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}; this build reads {SCHEMA_VERSION}",
            parsed.schema_version
        ));
    }
    if parsed.workloads.is_empty() {
        return Err(format!("{DECLARATION_PATH} declares no workloads"));
    }
    let mut names = BTreeSet::new();
    for workload in &parsed.workloads {
        if workload.kind.trim().is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} carries a workload with no kind"
            ));
        }
        if !names.insert(workload.kind.as_str()) {
            return Err(format!(
                "{DECLARATION_PATH} declares workload '{}' more than once",
                workload.kind
            ));
        }
        if workload.product.trim().is_empty() {
            return Err(format!(
                "{} declares no product; add it to {DECLARATION_PATH}",
                workload.kind
            ));
        }
        if workload.report.is_empty() {
            return Err(format!(
                "{} declares no report fields; add them to {DECLARATION_PATH}",
                workload.kind
            ));
        }
    }
    Ok(parsed)
});

fn catalog() -> Result<&'static WorkloadCatalog, CmdError> {
    CATALOG
        .as_ref()
        .map_err(|message| CmdError::click(message.clone()))
}

fn workload(kind: &str) -> Result<&'static WorkloadKind, CmdError> {
    catalog()?
        .workloads
        .iter()
        .find(|candidate| candidate.kind == kind)
        .ok_or_else(|| {
            CmdError::click(format!(
                "workload kind '{kind}' is not declared; add it to {DECLARATION_PATH}"
            ))
        })
}

#[derive(Subcommand)]
pub enum WorkloadCommands {
    /// List every workload kind compiled into this Stado build.
    List {
        /// Emit the declaration as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Place and run one declared non-interactive workload.
    Run {
        kind: String,
        /// Pin placement to one registry target.
        #[arg(long)]
        target: Option<String>,
        /// JSON plan whose schema is declared by the workload kind.
        #[arg(long)]
        plan: Option<String>,
        /// Emit the workload's report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read the latest report for a workload kind or receipt identifier.
    Status {
        kind_or_id: String,
        /// Read the report from one registry target.
        #[arg(long)]
        target: Option<String>,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Attach this process's stdin and stdout to an interactive workload.
    Attach {
        kind: String,
        /// Reattach on one registry target rather than placing afresh.
        #[arg(long)]
        target: Option<String>,
    },
}

pub async fn dispatch(command: WorkloadCommands) -> Result<(), CmdError> {
    match command {
        WorkloadCommands::List { json } => list(json),
        WorkloadCommands::Run {
            kind,
            target,
            plan,
            json,
        } => run(&kind, target.as_deref(), plan.as_deref(), json).await,
        WorkloadCommands::Status {
            kind_or_id,
            target,
            json,
        } => status(&kind_or_id, target.as_deref(), json).await,
        WorkloadCommands::Attach { kind, target } => attach(&kind, target.as_deref()).await,
    }
}

fn list(json_output: bool) -> Result<(), CmdError> {
    let catalog = catalog()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(catalog)?);
    } else {
        super::table::print(
            &[
                "KIND",
                "PRODUCT",
                "MODE",
                "ALLOWANCE",
                "PLAN SCHEMA",
                "REPORT",
            ],
            &catalog
                .workloads
                .iter()
                .map(|workload| {
                    vec![
                        workload.kind.clone(),
                        workload.product.clone(),
                        if workload.interactive {
                            "interactive"
                        } else {
                            "batch"
                        }
                        .to_string(),
                        workload
                            .registry_allowance
                            .as_deref()
                            .unwrap_or("none")
                            .to_string(),
                        workload
                            .plan_schema
                            .as_deref()
                            .unwrap_or("none")
                            .to_string(),
                        workload.report.join(", "),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    Ok(())
}

fn read_plan<'a>(
    declaration: &WorkloadKind,
    path: Option<&'a str>,
) -> Result<Option<(Value, &'a str)>, CmdError> {
    let Some(schema) = declaration.plan_schema.as_deref() else {
        if path.is_some() {
            return Err(CmdError::usage(format!(
                "{} accepts no plan; remove --plan because {DECLARATION_PATH} declares none",
                declaration.kind
            )));
        }
        return Ok(None);
    };
    let path = path.ok_or_else(|| {
        CmdError::usage(format!(
            "{} requires --plan FILE with schema {schema}; add the plan declared by {DECLARATION_PATH}",
            declaration.kind
        ))
    })?;
    let bytes = std::fs::read(path).map_err(|error| {
        CmdError::usage(format!("workload plan {path} cannot be read: {error}"))
    })?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::usage(format!(
            "workload plan {path} is not readable JSON: {error}"
        ))
    })?;
    if !document.is_object() {
        return Err(CmdError::usage(format!(
            "workload plan {path} must be a JSON object"
        )));
    }
    let observed = document
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    if observed != schema {
        return Err(CmdError::usage(format!(
            "{} plan declares schema {observed}, not {schema}; fix the whole plan before any work is enqueued",
            declaration.kind
        )));
    }
    Ok(Some((document, path)))
}

fn dynamic_allowance<'a>(
    declaration: &'a WorkloadKind,
    plan: Option<&'a Value>,
) -> Option<&'a str> {
    match declaration.registry_allowance.as_deref() {
        Some("$plan.action") => plan
            .and_then(|document| document.get("action"))
            .and_then(Value::as_str)
            .or(Some(crate::deploy::weles_browser_task::DEFAULT_ACTION)),
        allowance => allowance,
    }
}

fn target_declares(
    declaration: &WorkloadKind,
    target: &ComputeTarget,
    allowance: Option<&str>,
) -> bool {
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return false;
    }
    match declaration.kind.as_str() {
        "jeden-session" => true,
        "gui-automation" => target.release_platform == "darwin-arm64",
        "mobile-runtime" => target.mobile_runtime.is_some(),
        _ if declaration.product == "weles-worker" => {
            let Some(weles) = target.weles.as_ref() else {
                return false;
            };
            allowance.is_none_or(|required| {
                weles.enabled && weles.actions.iter().any(|action| action == required)
            })
        }
        _ => allowance.is_none(),
    }
}

async fn place(
    declaration: &WorkloadKind,
    requested: Option<&str>,
    allowance: Option<&str>,
) -> Result<ComputeTarget, CmdError> {
    let registry = super::registry::read_registry().await?;
    if let Some(name) = requested {
        let target = registry
            .targets
            .iter()
            .find(|candidate| candidate.name == name)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "target '{name}' is not declared; add it to the canonical registry"
                ))
            })?;
        if !target_declares(declaration, target, allowance) {
            return Err(CmdError::click(format!(
                "{} declares no {}; add it to {DECLARATION_PATH}",
                target.name, declaration.kind
            )));
        }
        return Ok(target.clone());
    }

    let mut candidates = registry
        .targets
        .iter()
        .filter(|target| target_declares(declaration, target, allowance))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.into_iter().next().ok_or_else(|| {
        CmdError::click(format!(
            "the fleet declares no {}; add it to {DECLARATION_PATH}",
            declaration.kind
        ))
    })
}

async fn run(
    kind: &str,
    requested_target: Option<&str>,
    plan_path: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let declaration = workload(kind)?;
    if declaration.interactive {
        if json_output {
            return Err(CmdError::usage(format!(
                "{kind} is interactive and cannot produce JSON; use `stado workload attach {kind}`"
            )));
        }
        return Err(CmdError::usage(format!(
            "{kind} is interactive; use `stado workload attach {kind}`"
        )));
    }
    let plan = read_plan(declaration, plan_path)?;
    let document = plan.as_ref().map(|(document, _)| document);
    let allowance = dynamic_allowance(declaration, document);
    let plan_target = if kind == "weles-capture" {
        document
            .and_then(|value| value.get("target"))
            .and_then(Value::as_str)
    } else {
        None
    };
    let resolved = place(declaration, requested_target.or(plan_target), allowance).await?;
    let target = resolved.name.as_str();

    match kind {
        "weles-capture" => {
            run_weles_capture(
                target,
                required_plan_path(plan.as_ref(), kind)?,
                json_output,
            )
            .await
        }
        "weles-browser-task" => {
            run_weles_browser_task(target, required_plan(document, kind)?, json_output).await
        }
        "weles-diagnostics" => {
            run_weles_diagnostics(target, required_plan(document, kind)?, json_output).await
        }
        "weles-image-inspect" => {
            run_weles_image_inspect(target, required_text(document, "url")?, json_output).await
        }
        "weles-activity" => weles_activity(target, json_output).await,
        "weles-recordings" => {
            set_weles_recordings_dir(target, required_text(document, "path")?, json_output).await
        }
        "weles-api-runtime" => {
            refresh_weles_api_runtime(target, required_text(document, "revision")?, json_output)
                .await
        }
        "weles-browser-runtime" => {
            let document = required_plan(document, kind)?;
            let components = string_array(document, "components")?;
            weles_browser_runtime(
                target,
                &components,
                boolean(document, "repair", false),
                json_output,
            )
            .await
        }
        "gui-automation" => {
            run_gui_automation(target, required_plan(document, kind)?, json_output).await
        }
        "mobile-runtime" => {
            let document = required_plan(document, kind)?;
            mobile_runtime(target, boolean(document, "repair", false), json_output).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} has no runner; add it to {DECLARATION_PATH}"
        ))),
    }
}

async fn status(
    selector: &str,
    requested_target: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let (kind, receipt) = if let Some((kind, id)) = selector.split_once(':') {
        (kind, Some(id))
    } else if catalog()?
        .workloads
        .iter()
        .any(|entry| entry.kind == selector)
    {
        (selector, None)
    } else {
        ("weles-capture", Some(selector))
    };
    let declaration = workload(kind)?;
    if declaration.interactive {
        return Err(CmdError::usage(format!(
            "{kind} is interactive and has no receipt report; use `stado workload attach {kind}`"
        )));
    }
    let resolved = place(
        declaration,
        requested_target,
        dynamic_allowance(declaration, None),
    )
    .await?;
    let target = resolved.name.as_str();
    match (kind, receipt) {
        ("weles-capture", Some(batch)) => weles_capture_status(target, batch, json_output).await,
        ("weles-capture", None) => Err(CmdError::usage(
            "weles-capture status needs its receipt id: `stado workload status weles-capture:<batch>`",
        )),
        ("weles-diagnostics", Some(run_id)) => weles_run_diagnostics(target, run_id, None, json_output).await,
        ("weles-diagnostics", None) => Err(CmdError::usage(
            "weles-diagnostics status needs its receipt id: `stado workload status weles-diagnostics:<run-id>`",
        )),
        ("weles-browser-runtime", _) => weles_browser_runtime(target, &[], false, json_output).await,
        ("mobile-runtime", _) => mobile_runtime(target, false, json_output).await,
        ("gui-automation", _) => gui_automation_status(target, json_output).await,
        ("weles-recordings", _) => recordings_status(&resolved, json_output),
        ("weles-activity" | "weles-browser-task" | "weles-image-inspect" | "weles-api-runtime", _) => {
            weles_activity(target, json_output).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no status report; add it to {DECLARATION_PATH}"
        ))),
    }
}

async fn attach(kind: &str, requested_target: Option<&str>) -> Result<(), CmdError> {
    let declaration = workload(kind)?;
    if !declaration.interactive {
        return Err(CmdError::usage(format!(
            "{kind} is not interactive; use `stado workload run {kind}`"
        )));
    }
    match kind {
        "jeden-session" => {
            let target = match requested_target {
                Some(name) => Some(place(declaration, Some(name), None).await?.name),
                None => None,
            };
            let workspace = current_workspace();
            connect_jeden(&workspace, target.as_deref(), None).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no stream attachment; add it to {DECLARATION_PATH}"
        ))),
    }
}

fn required_text<'a>(document: Option<&'a Value>, field: &str) -> Result<&'a str, CmdError> {
    document
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::usage(format!(
                "workload plan declares no {field}; add it to the plan"
            ))
        })
}

fn boolean(document: &Value, field: &str, default: bool) -> bool {
    document
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn string_array(document: &Value, field: &str) -> Result<Vec<String>, CmdError> {
    let Some(value) = document.get(field) else {
        return Ok(Vec::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        CmdError::usage(format!("workload plan {field} must be an array of strings"))
    })?;
    entries
        .iter()
        .map(|entry| {
            entry.as_str().map(ToString::to_string).ok_or_else(|| {
                CmdError::usage(format!("workload plan {field} must contain only strings"))
            })
        })
        .collect()
}

fn required_plan<'a>(document: Option<&'a Value>, kind: &str) -> Result<&'a Value, CmdError> {
    document.ok_or_else(|| {
        CmdError::usage(format!(
            "{kind} declares no plan document; add the plan required by {DECLARATION_PATH}"
        ))
    })
}

fn required_plan_path<'a>(
    plan: Option<&(Value, &'a str)>,
    kind: &str,
) -> Result<&'a str, CmdError> {
    plan.map(|(_, path)| *path).ok_or_else(|| {
        CmdError::usage(format!(
            "{kind} declares no plan file; add the plan required by {DECLARATION_PATH}"
        ))
    })
}

fn print_json(report: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(report));
}

fn recordings_status(target: &ComputeTarget, json_output: bool) -> Result<(), CmdError> {
    let path = target
        .weles
        .as_ref()
        .and_then(|weles| weles.recordings_dir.as_deref())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no recordings directory; add it to the canonical registry",
                target.name
            ))
        })?;
    if json_output {
        print_json(&json!({
            "kind": "weles-recordings",
            "target": target.name,
            "recordings_directory": path,
        }));
    } else {
        println!("{}: Weles recordings are written to {path}", target.name);
    }
    Ok(())
}

fn print_gui_report(
    report: &crate::deploy::host_gui_automation::GuiAutomationReport,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        for (item, state) in &report.items {
            println!("{}\t{item}\t{state}", report.target);
        }
    }
    match &report.error {
        Some(detail) if !detail.is_empty() => Err(CmdError::click(detail.clone())),
        Some(_) => Err(CmdError::click("remote command failed")),
        None => Ok(()),
    }
}

async fn gui_automation_status(target: &str, json_output: bool) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let password = super::service::host_sudo_password(&resolved).await?;
    let runner = crate::deploy::production_runner();
    let report =
        crate::deploy::host_gui_automation::status(&resolved, password.as_deref(), &runner).await;
    print_gui_report(&report, json_output)
}

async fn run_gui_automation(target: &str, plan: &Value, json_output: bool) -> Result<(), CmdError> {
    let operation = required_text(Some(plan), "operation")?;
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = match operation {
        "enable" => {
            let password = super::service::host_sudo_password(&resolved)
                .await?
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "{} declares no readable host-account password; add it to the host account credential",
                        resolved.name
                    ))
                })?;
            crate::deploy::host_gui_automation::enable(&resolved, &password, &runner).await
        }
        "grant-accessibility" => {
            let password = super::service::host_sudo_password(&resolved).await?;
            crate::deploy::host_gui_automation::grant_accessibility(
                &resolved,
                boolean(plan, "apple_only", false),
                password.as_deref(),
                &runner,
            )
            .await
        }
        "disable" => {
            let bundle = plan.get("bundle").and_then(Value::as_str).unwrap_or("");
            crate::deploy::host_gui_automation::disable(&resolved, bundle, &runner).await
        }
        other => {
            return Err(CmdError::usage(format!(
                "gui-automation plan operation '{other}' is not enable, disable, or grant-accessibility; fix the plan"
            )))
        }
    };
    print_gui_report(&report, json_output)
}

async fn registry_target(target: &str) -> Result<ComputeTarget, CmdError> {
    let registry = super::registry::read_registry().await?;
    registry
        .targets
        .iter()
        .find(|candidate| candidate.name == target)
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "target '{target}' is not declared; add it to the canonical registry"
            ))
        })
}

async fn set_weles_recordings_dir(
    target: &str,
    path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    use serde_json::Map;

    if !std::path::Path::new(path).is_absolute() {
        return Err(CmdError::usage(
            "weles-recordings plan path must be absolute; fix the plan",
        ));
    }

    let store = crate::targets::RegistryStore::open().await?;
    let current = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click("canonical registry declares no generation; publish it first")
    })?;
    let mut document: Value = serde_json::from_str(&current.content)?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            CmdError::click("canonical registry declares no targets array; repair it")
        })?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
        .ok_or_else(|| {
            CmdError::click(format!(
                "target '{target}' is not declared; add it to the canonical registry"
            ))
        })?
        .as_object_mut()
        .ok_or_else(|| CmdError::click("canonical registry target is not an object; repair it"))?;

    let weles = entry
        .entry("weles")
        .or_insert_with(|| json!({"enabled": false, "actions": []}))
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} declares no Weles object; add it to the canonical registry"
            ))
        })?;
    weles.insert(
        "recordings_dir".to_string(),
        Value::String(path.to_string()),
    );

    if let Some(cleanup) = entry.get_mut("disk_cleanup").and_then(Value::as_object_mut) {
        let cleaners = cleanup
            .entry("cleaners")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(
                    "disk_cleanup declares no cleaners object; repair the canonical registry",
                )
            })?;
        let cleaner = cleaners
            .entry("weles_recordings")
            .or_insert_with(|| json!({"min_age_seconds": 604800}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(
                    "weles_recordings declares no cleaner object; repair the canonical registry",
                )
            })?;
        cleaner.insert("root".to_string(), Value::String(path.to_string()));
    }

    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let payload = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let generation = store.compare_and_swap(&current.version, &payload).await?;
    if !json_output {
        println!("registry: {target} weles.recordings_dir={path} (generation {generation})");
    }

    let hostname = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let registry = crate::targets::load_registry_from_str(&payload)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let is_self = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .is_some_and(|entry| entry.name == target);
    if !is_self {
        if json_output {
            print_json(&json!({
                "kind": "weles-recordings",
                "target": target,
                "recordings_directory": path,
                "registry_generation": generation,
                "updated_launch_agents": 0,
                "launch_agents_local": false,
            }));
        } else {
            println!(
                "{target}: registry updated; run this workload on that host to update its LaunchAgents"
            );
        }
        return Ok(());
    }

    std::fs::create_dir_all(path)?;
    let agents_dir = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| CmdError::click("HOME is not set; set it before updating LaunchAgents"))?
        .join("Library/LaunchAgents");
    let mut touched = 0usize;
    for item in std::fs::read_dir(&agents_dir)? {
        let plist = item?.path();
        let Some(name) = plist.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("com.wisent.weles-") || !name.ends_with(".plist") {
            continue;
        }
        set_plist_recordings_root(&plist, path)?;
        touched += 1;
        if !json_output {
            println!("  {name}: WELES_RECORDINGS_ROOT={path}");
        }
    }
    if json_output {
        print_json(&json!({
            "kind": "weles-recordings",
            "target": target,
            "recordings_directory": path,
            "registry_generation": generation,
            "updated_launch_agents": touched,
            "launch_agents_local": true,
        }));
    } else {
        println!("updated {touched} LaunchAgent plist(s); reload Weles agents to apply");
    }
    Ok(())
}

fn set_plist_recordings_root(plist: &std::path::Path, path: &str) -> Result<(), CmdError> {
    fn plutil(plist: &std::path::Path, args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("/usr/bin/plutil")
            .args(args)
            .arg(plist)
            .output()
    }

    let key = "EnvironmentVariables.WELES_RECORDINGS_ROOT";
    if plutil(plist, &["-replace", key, "-string", path])?
        .status
        .success()
    {
        return Ok(());
    }
    if plutil(plist, &["-insert", key, "-string", path])?
        .status
        .success()
    {
        return Ok(());
    }
    let _ = plutil(
        plist,
        &["-insert", "EnvironmentVariables", "-xml", "<dict/>"],
    )?;
    let retry = plutil(plist, &["-insert", key, "-string", path])?;
    if retry.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&retry.stderr).trim().to_string();
    Err(CmdError::click(format!(
        "{}: failed to update WELES_RECORDINGS_ROOT: {message}",
        plist.display()
    )))
}

async fn run_weles_capture(
    target: &str,
    plan_path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    // Parsing validates the entire plan before the admission endpoint is
    // resolved or contacted. Enqueue is therefore all-or-nothing.
    let plan = crate::deploy::weles_capture::parse_plan(plan_path, target, None)
        .map_err(|error| CmdError::usage(error.to_string()))?;
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let accepted = crate::deploy::weles_capture::enqueue(&channel, &plan)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    if json_output {
        print_json(&json!({
            "kind": "weles-capture",
            "target": target,
            "batch": plan.batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "admission_token": channel.token_state(),
            "enqueued": accepted.len(),
            "actions": accepted.iter().map(|action| json!({
                "action_id": action.action_id,
                "site_slug": action.site_slug,
                "axis": action.axis,
                "artifact_prefix": action.artifact_prefix,
            })).collect::<Vec<_>>(),
            "status": "enqueued",
        }));
    } else {
        println!(
            "{target}: enqueued {} {} action(s) for batch {} on {}",
            accepted.len(),
            crate::deploy::weles_capture::CAPTURE_ACTION,
            plan.batch,
            admission.declared_url,
        );
        for action in &accepted {
            println!(
                "  {:<38} {:<24} {:<13} {}",
                action.action_id, action.site_slug, action.axis, action.artifact_prefix,
            );
        }
    }
    Ok(())
}

async fn run_weles_image_inspect(
    target: &str,
    source_url: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let parsed = url::Url::parse(source_url)
        .map_err(|error| CmdError::usage(format!("workload plan url is not a URL: {error}")))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CmdError::usage(
            "weles-image-inspect plan url must be an HTTPS URL without embedded credentials",
        ));
    }
    let source_url = parsed.to_string();
    let host = parsed.host_str().ok_or_else(|| {
        CmdError::usage("weles-image-inspect plan url declares no host; add it to the URL")
    })?;
    let objective = "Inspect this public page without signing in or changing any user or application state. Scroll through the whole page to trigger lazy-loaded media. Inspect every rendered img element and report its currentSrc URL host and pathname, complete flag, naturalWidth and naturalHeight. Inspect PerformanceResourceTiming entries for image resources and /api/stado/object requests, including responseStatus where Chromium exposes it. Count loaded and failed images, count /api/stado/object image URLs, list every failed URL or HTTP status, and list any visible image-error placeholder text and the affected card or room name. Return one concise JSON object containing final_url, rendered_images, loaded_images, failed_images, stado_object_images, failed_resources, and visible_placeholders.";
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let result = crate::deploy::weles_capture::observe_action_payload(
        &channel,
        "generic_browser_task",
        json!({
            "url": source_url.as_str(),
            "objective": objective,
            "flow_name": format!("stado-image-inspection:{host}"),
            "session_label": format!("stado-image-inspection-{host}"),
            "proxy": "none",
            "headless": true,
            "constraints": {
                "read_only": true,
                "no_login": true,
                "no_mutation": true,
            },
        }),
        None,
        false,
    )
    .await
    .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let run_id = result
        .get("run_id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target} returned no Weles diagnostic run id; inspect the Weles admission report"
            ))
        })?;
    let diagnostics = crate::deploy::weles_capture::image_diagnostics(&channel, run_id)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let task_result = result.get("result").cloned().unwrap_or(Value::Null);
    let report = json!({
        "kind": "weles-image-inspect",
        "target": target,
        "source_url": source_url.as_str(),
        "action": "generic_browser_task",
        "endpoint": admission.declared_url,
        "transport": channel.transport(),
        "admission_token": channel.token_state(),
        "browser_run": {
            "run_id": run_id,
            "trajectory_ok": result.get("ok").and_then(Value::as_bool).unwrap_or(false),
            "exit_code": result.get("exitCode"),
            "final_url": task_result.get("final_url"),
            "trajectory_error": task_result.get("error"),
        },
        "images": diagnostics,
    });
    if json_output {
        print_json(&report);
    } else {
        println!(
            "{target}: inspected {} through {}",
            report["source_url"].as_str().unwrap_or(source_url.as_str()),
            admission.declared_url,
        );
        print_json(&diagnostics);
    }
    Ok(())
}

async fn weles_browser_runtime(
    target: &str,
    components: &[String],
    repair: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let declared = crate::deploy::weles_browser_runtime::requirements(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let required = if components.is_empty() {
        vec![crate::deploy::weles_browser_runtime::DEFAULT_COMPONENT.to_string()]
    } else {
        components.to_vec()
    };
    let mut report =
        crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    let mut installed = Vec::new();
    if repair {
        installed = crate::deploy::weles_browser_runtime::repair(&resolved, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        report =
            crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
    }
    if json_output {
        let mut object = report.to_report(&resolved.name);
        object.insert("kind".to_string(), json!("weles-browser-runtime"));
        object.insert("repaired".to_string(), json!(installed));
        print_json(&Value::Object(object));
    } else {
        println!("host:           {}", resolved.name);
        println!("runtime:        {}", report.verdict());
        println!("required state: {}", report.required_state());
        println!("browser engine: {}", report.browser_engine_state());
        for line in &installed {
            println!("repair:   {line}");
        }
        super::table::print(
            &["COMPONENT", "REVISION", "DEFAULT", "STATE", "EXPECTED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.revision.clone(),
                        component.install_by_default.to_string(),
                        component.state.clone(),
                        component.expected_path.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}

async fn mobile_runtime(target: &str, repair: bool, json_output: bool) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let Some(declared) = crate::deploy::mobile_runtime::requirement(&resolved).cloned() else {
        return Err(CmdError::click(format!(
            "{} declares no mobile-runtime; add it to the canonical registry",
            resolved.name
        )));
    };
    let mut report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut installed = Vec::new();
    if repair {
        installed = crate::deploy::mobile_runtime::repair(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    }
    if json_output {
        let mut object = report.to_report(&resolved.name);
        object.insert("kind".to_string(), json!("mobile-runtime"));
        object.insert("repaired".to_string(), json!(installed));
        print_json(&Value::Object(object));
    } else {
        println!("host:     {}", resolved.name);
        println!("runtime:  {}", report.verdict());
        for line in &installed {
            println!("repair:   {line}");
        }
        super::table::print(
            &["COMPONENT", "DECLARED", "OBSERVED", "STATE", "RESOLVED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.declared.clone(),
                        if component.observed.is_empty() {
                            "-".to_string()
                        } else {
                            component.observed.clone()
                        },
                        component.state.clone(),
                        component.path.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}

async fn run_weles_diagnostics(
    target: &str,
    plan: &Value,
    json_output: bool,
) -> Result<(), CmdError> {
    let run_id = required_text(Some(plan), "run_id")?;
    let file = plan.get("file").and_then(Value::as_str);
    weles_run_diagnostics(target, run_id, file, json_output).await
}

async fn weles_run_diagnostics(
    target: &str,
    run_id: &str,
    file: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let Some(path) = file else {
        let manifest = crate::deploy::weles_capture::run_diagnostics(&channel, run_id)
            .await
            .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
        print_json(&manifest);
        return Ok(());
    };
    let bytes = crate::deploy::weles_capture::run_diagnostic_file(&channel, run_id, path)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let byte_count = bytes.len();
    let (encoding, content) = match String::from_utf8(bytes) {
        Ok(text) => ("utf8", text),
        Err(error) => ("base64", STANDARD.encode(error.into_bytes())),
    };
    if json_output {
        print_json(&json!({
            "kind": "weles-diagnostics",
            "target": target,
            "run_id": run_id,
            "path": path,
            "bytes": byte_count,
            "encoding": encoding,
            "content": content,
        }));
    } else if encoding == "utf8" {
        print!("{content}");
    } else {
        println!("base64:{content}");
    }
    Ok(())
}

async fn weles_capture_status(
    target: &str,
    batch: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let batch_status = crate::deploy::weles_capture::status(&channel, batch)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let states = batch_status.captures;
    let totals = crate::deploy::weles_capture::totals(&states);
    let stored: usize = states.iter().map(|state| state.artifacts.len()).sum();
    if json_output {
        print_json(&json!({
            "kind": "weles-capture",
            "target": target,
            "batch": batch,
            "action": crate::deploy::weles_capture::CAPTURE_ACTION,
            "endpoint": admission.declared_url,
            "transport": channel.transport(),
            "artifacts_unreachable": batch_status.artifacts_unreachable,
            "actions": states.iter().map(|state| json!({
                "action_id": state.action_id,
                "site_slug": state.site_slug,
                "axis": state.axis,
                "state": state.state,
                "error": state.error,
                "artifact_prefix": state.artifact_prefix,
                "artifacts": state.artifacts,
            })).collect::<Vec<_>>(),
            "totals": totals.iter().map(|(state, count)| {
                (state.clone(), Value::from(*count))
            }).collect::<serde_json::Map<String, Value>>(),
            "artifacts_stored": stored,
        }));
    } else {
        println!(
            "{target}: batch {batch} carries {} {} action(s), {}",
            states.len(),
            crate::deploy::weles_capture::CAPTURE_ACTION,
            totals
                .iter()
                .map(|(state, count)| format!("{count} {state}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if let Some(unreachable) = &batch_status.artifacts_unreachable {
            println!("{target}: artifact listing unreadable: {unreachable}");
        }
        for state in &states {
            println!(
                "  {:<9} {:<24} {:<13} {:>3} artifact(s)  {}{}",
                state.state,
                state.site_slug,
                state.axis,
                state.artifacts.len(),
                state.action_id,
                state
                    .error
                    .as_deref()
                    .map_or_else(String::new, |error| format!("  {error}")),
            );
        }
        println!(
            "{target}: {stored} object(s) under stado://{}/{batch}/",
            crate::deploy::weles_capture::ARTIFACT_NAMESPACE
        );
    }
    if states.is_empty() {
        return Err(CmdError::click(format!(
            "{target}: no {} action carries batch {batch}; enqueue it with `stado workload run weles-capture`",
            crate::deploy::weles_capture::CAPTURE_ACTION
        )));
    }
    Ok(())
}

/// Fed to the host's own `node` over the channel's stdin, with the run limit
/// and API port as argv — the same two values the retired bash wrapper took
/// from the host's environment. There is nothing to install on the host and
/// nothing left behind after the read.
///
/// Recordings hold page DOM, console output, HAR bodies, personas and proxy
/// identities. None of that is emitted. What leaves the host is counts,
/// timestamps, run identifiers, artifact sizes, cost, and the pass/fail flag a
/// trajectory wrote about itself — the fields a remote operator view needs to
/// name a run and say how it ended.
const WELES_ACTIVITY_SOURCE: &str = r#"const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const runLimit = Math.max(1, Number.parseInt(process.argv.at(-2), 10) || 40);
const apiPort = Number.parseInt(process.argv.at(-1), 10) || 8788;
const home = os.homedir();
const legacyWorkerRoot = path.join(home, '.local/share/weles-worker');
const managedServiceRoot = path.join(home, '.stado/services/weles-admission');
const managedWorkerRoot = path.join(managedServiceRoot, 'current');

const hostname = String(os.hostname()).trim().toLowerCase().replace(/\.+$/, '');
const shortHostname = hostname.endsWith('.local') ? hostname.slice(0, -'.local'.length) : hostname;

const isoOrNull = (value) => {
  const time = Number(value);
  return Number.isFinite(time) && time > 0 ? new Date(time).toISOString() : null;
};

const readJson = (file) => {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
};

const compareVersions = (left, right) => {
  const parts = (value) => String(value).split('.').map((piece) => Number.parseInt(piece, 10) || 0);
  const [a, b] = [parts(left), parts(right)];
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) return difference;
  }
  return 0;
};

const releaseVersions = new Set();
const recordingSources = [];
const addRecordingSource = (release, platform, recordings, priority) => {
  if (typeof release !== 'string' || !release) return;
  try {
    if (!fs.statSync(recordings).isDirectory()) return;
  } catch {
    return;
  }
  releaseVersions.add(release);
  recordingSources.push({ release, platform, recordings, priority });
};
const addManagedRuntime = (runtime, platform, priority) => {
  const manifest = readJson(path.join(runtime, 'package.json'));
  const release = typeof manifest?.version === 'string' && manifest.version
    ? manifest.version
    : null;
  if (release) releaseVersions.add(release);
  addRecordingSource(release, platform, path.join(runtime, 'recordings'), priority);
};

// `current` is the active immutable coordinate. Count its release even before
// the first browser run creates a recordings directory.
addManagedRuntime(path.join(managedWorkerRoot, 'runtime'), 'managed', 2);

// Also report every immutable release Stado installed. The service store is
// digest-addressed (`sha256-*/<platform>/runtime`), not version-addressed, and
// tying release discovery to a recordings directory hid fresh installations
// until their first browser artifact existed.
try {
  for (const releaseEntry of fs.readdirSync(managedServiceRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory() || !releaseEntry.name.startsWith('sha256-')) continue;
    const releaseRoot = path.join(managedServiceRoot, releaseEntry.name);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addManagedRuntime(
        path.join(releaseRoot, platformEntry.name, 'runtime'),
        platformEntry.name,
        1,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

// Keep reporting recordings written by the retired per-version installer while
// hosts complete their cutover to the fleet-managed service.
try {
  for (const releaseEntry of fs.readdirSync(legacyWorkerRoot, { withFileTypes: true })) {
    if (!releaseEntry.isDirectory()) continue;
    const release = releaseEntry.name;
    const releaseRoot = path.join(legacyWorkerRoot, release);
    for (const platformEntry of fs.readdirSync(releaseRoot, { withFileTypes: true })) {
      if (!platformEntry.isDirectory()) continue;
      addRecordingSource(
        release,
        platformEntry.name,
        path.join(releaseRoot, platformEntry.name, 'recordings'),
        0,
      );
    }
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
const releases = [...releaseVersions].sort(compareVersions);

// The version marker names the release the retired activator staged. It can
// disagree with the active fleet-managed release and remains useful evidence
// that the old delivery path has not been removed from a host yet.
const releaseMarker = (() => {
  try {
    return fs.readFileSync(path.join(home, '.stado/files/weles-release-version'), 'utf8').trim() || null;
  } catch {
    return null;
  }
})();

const ARTIFACT_CLASSES = [
  ['screenshots', /\.png$/i],
  ['pages', /\.html$/i],
  ['videos', /\.webm$/i],
  ['logs', /\.(log|ndjson)$/i],
  ['records', /\.json$|\.jsonl$|\.har$/i],
];

const classify = (name) => {
  for (const [label, pattern] of ARTIFACT_CLASSES) {
    if (pattern.test(name)) return label;
  }
  return 'other';
};

const RUNNING_WINDOW_MS = 180_000;

const describeRun = (release, platform, runDirectory) => {
  const stat = fs.statSync(runDirectory);
  const counts = { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 };
  let bytes = 0;
  let action = null;
  let resultOk = null;
  let resultHealthy = null;
  let resultSignal = null;
  let resultAt = null;
  let uploadProof = null;
  let startedAt = null;
  let completedAt = null;

  const walk = (directory, depth) => {
    let entries = [];
    try {
      entries = fs.readdirSync(directory, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        // The one directory directly under a run is the action that produced it.
        if (depth === 0 && !action) action = entry.name;
        if (depth < 4) walk(full, depth + 1);
        continue;
      }
      if (!entry.isFile()) continue;
      counts[classify(entry.name)] += 1;
      try {
        bytes += fs.statSync(full).size;
      } catch {
        // A file rotated away mid-walk is not worth failing the report over.
      }
      if (/result\.json$/i.test(entry.name)) {
        const document = readJson(full);
        if (document && typeof document.ok === 'boolean') resultOk = document.ok;
        if (typeof document?.completed_at === 'string') completedAt = document.completed_at;
      } else if (entry.name === 'ban_signal.json') {
        const document = readJson(full);
        if (typeof document?.healthy === 'boolean') resultHealthy = document.healthy;
        if (typeof document?.signal === 'string' && document.signal) resultSignal = document.signal;
        if (typeof document?.ts === 'string') resultAt = document.ts;
      } else if (entry.name === '.uploaded.json') {
        const document = readJson(full);
        if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
          uploadProof = { sha256: document.sha256, destination: document.destination };
        }
      } else if (entry.name === 'session_meta.json') {
        const document = readJson(full);
        if (typeof document?.started_at === 'string') startedAt = document.started_at;
      }
    }
  };
  walk(runDirectory, 0);

  if (!uploadProof) {
    const document = readJson(path.join(runDirectory, '.uploaded.json'));
    if (typeof document?.sha256 === 'string' && typeof document?.destination === 'string') {
      uploadProof = { sha256: document.sha256, destination: document.destination };
    }
  }
  const costs = readJson(path.join(path.dirname(runDirectory), '_costs', `${path.basename(runDirectory)}.json`));
  const isFresh = Date.now() - stat.mtimeMs < RUNNING_WINDOW_MS;

  let status = 'recorded';
  if (resultHealthy === true || resultOk === true) status = 'succeeded';
  else if (resultHealthy === false || resultOk === false || resultSignal) status = 'failed';
  else if (isFresh) status = 'running';

  return {
    id: path.basename(runDirectory),
    release,
    platform,
    action,
    status,
    started_at: startedAt ?? isoOrNull(stat.birthtimeMs),
    completed_at: completedAt,
    updated_at: isoOrNull(stat.mtimeMs),
    artifact_counts: counts,
    artifact_bytes: bytes,
    cost_usd: typeof costs?.cost_usd === 'number' ? costs.cost_usd : null,
    result: resultHealthy !== null || resultSignal
      ? { healthy: resultHealthy, signal: resultSignal, recorded_at: resultAt }
      : null,
    uploaded: uploadProof !== null,
    upload_proof: uploadProof,
  };
};

const runsById = new Map();
for (const source of recordingSources) {
  let entries = [];
  try {
    entries = fs.readdirSync(source.recordings, { withFileTypes: true });
  } catch {
    continue;
  }
  for (const entry of entries) {
    // `_costs` is the sidecar ledger of the runs beside it, not a run.
    if (!entry.isDirectory() || entry.name === '_costs') continue;
    const candidate = {
      release: source.release,
      platform: source.platform,
      directory: path.join(source.recordings, entry.name),
      priority: source.priority,
    };
    const existing = runsById.get(entry.name);
    if (!existing || candidate.priority > existing.priority) runsById.set(entry.name, candidate);
  }
}
const runs = [...runsById.values()];
runs.sort((left, right) => {
  const time = (row) => {
    try {
      return fs.statSync(row.directory).mtimeMs;
    } catch {
      return 0;
    }
  };
  return time(right) - time(left);
});

const describedById = new Map();
for (const row of runs) {
  const summary = describeRun(row.release, row.platform, row.directory);
  describedById.set(summary.id, summary);
}

// Weles API requests keep their process result outside a release runtime so an
// update cannot erase it. Fold those durable records into the live recording
// inventory: a cleaned recording loses its artifact counts, not the fact that
// the run happened or how its process ended.
const detachedRoot = path.join(home, '.stado/weles-detached-runs');
try {
  for (const entry of fs.readdirSync(detachedRoot, { withFileTypes: true })) {
    if (!entry.isFile() || !entry.name.endsWith('.json')) continue;
    const file = path.join(detachedRoot, entry.name);
    const document = readJson(file);
    if (!document || typeof document !== 'object') continue;
    const stat = fs.statSync(file);
    const fallbackId = entry.name.slice(0, -'.json'.length);
    const id = typeof document.run_id === 'string' && document.run_id
      ? document.run_id
      : fallbackId;
    const action = typeof document.action === 'string' && document.action
      ? document.action
      : null;

    let status = 'recorded';
    if (document.status === 'running' || document.ok === null) status = 'running';
    else if (document.ok === true) status = 'succeeded';
    else if (document.ok === false || document.status === 'failed') status = 'failed';

    const resultCandidates = [
      document.result,
      document.result && typeof document.result === 'object' ? document.result.result : null,
    ];
    let result = null;
    for (const candidate of resultCandidates) {
      if (!candidate || typeof candidate !== 'object') continue;
      const healthy = typeof candidate.healthy === 'boolean' ? candidate.healthy : null;
      const signal = typeof candidate.signal === 'string' && candidate.signal ? candidate.signal : null;
      if (healthy !== null || signal) {
        result = {
          healthy,
          signal,
          recorded_at: typeof candidate.ts === 'string'
            ? candidate.ts
            : (typeof document.completed_at === 'string' ? document.completed_at : null),
        };
        break;
      }
    }

    const release = typeof document.release_version === 'string' && document.release_version
      ? document.release_version
      : null;
    const durable = {
      id,
      release,
      platform: process.platform,
      action,
      status,
      started_at: typeof document.started_at === 'string'
        ? document.started_at
        : isoOrNull(stat.birthtimeMs),
      completed_at: typeof document.completed_at === 'string' ? document.completed_at : null,
      updated_at: isoOrNull(stat.mtimeMs),
      artifact_counts: { screenshots: 0, pages: 0, videos: 0, logs: 0, records: 0, other: 0 },
      artifact_bytes: 0,
      cost_usd: null,
      result,
      uploaded: false,
      upload_proof: null,
    };
    const live = describedById.get(id);
    describedById.set(id, live
      ? {
          ...live,
          action: live.action ?? durable.action,
          status: durable.status === 'recorded' ? live.status : durable.status,
          started_at: live.started_at ?? durable.started_at,
          completed_at: durable.completed_at ?? live.completed_at,
          updated_at: durable.updated_at ?? live.updated_at,
          result: durable.result ?? live.result,
        }
      : durable);
  }
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}

const allDescribed = [...describedById.values()].sort(
  (left, right) => (Date.parse(right.updated_at ?? '') || 0) - (Date.parse(left.updated_at ?? '') || 0),
);
const runTotal = allDescribed.length;
const described = allDescribed.slice(0, runLimit);

const probePort = (port) =>
  new Promise((resolve) => {
    const socket = net.createConnection({ host: '127.0.0.1', port });
    const finish = (listening) => {
      socket.destroy();
      resolve(listening);
    };
    socket.setTimeout(1500);
    socket.once('connect', () => finish(true));
    socket.once('timeout', () => finish(false));
    socket.once('error', () => finish(false));
  });

probePort(apiPort).then((listening) => {
  const document = {
    schema_version: 1,
    host: shortHostname || hostname,
    hostname,
    generated_at: new Date().toISOString(),
    worker: {
      staged_release: releaseMarker,
      installed_releases: releases,
      newest_release: releases.at(-1) ?? null,
    },
    api: {
      endpoint: `http://127.0.0.1:${apiPort}`,
      listening,
    },
    run_total: runTotal,
    runs: described,
  };
  process.stdout.write(`STADO-WELES-ACTIVITY ${JSON.stringify(document)}\n`);
});
"#;

/// The marker [`WELES_ACTIVITY_SOURCE`] prefixes to its one JSON line, so a
/// login shell's own greeting cannot be mistaken for the report.
const WELES_ACTIVITY_MARKER: &str = "STADO-WELES-ACTIVITY ";

/// Run [`WELES_ACTIVITY_SOURCE`] on one host with the host's own node, and
/// hand back what it printed.
///
/// The run limit and API port are the host's environment or the defaults the
/// retired wrapper carried, resolved on the host so an operator's local
/// environment cannot steer a remote read.
async fn read_weles_activity(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<String, crate::deploy::DeployError> {
    use crate::deploy::host_channel;
    let mut node = None;
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if host_channel::remote_test(resolved, &format!("-x {candidate}"), runner).await? {
            node = Some(candidate);
            break;
        }
    }
    let Some(node) = node else {
        return Err(crate::deploy::DeployError(
            "Node.js is unavailable on this host".to_string(),
        ));
    };
    let environment = host_channel::run_command(
        resolved,
        "printf '%s %s' \"${WELES_ACTIVITY_RUN_LIMIT:-40}\" \"${WELES_API_PORT:-8788}\"",
        runner,
    )
    .await?;
    if !environment.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &environment,
            "the host's Weles environment could not be read",
        )));
    }
    let mut values = environment.stdout.split_whitespace();
    let limit = values.next().unwrap_or("40");
    let port = values.next().unwrap_or("8788");
    let output = host_channel::run_program_with_stdin(
        resolved,
        &[node, "-", limit, port],
        WELES_ACTIVITY_SOURCE,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(crate::deploy::DeployError(host_channel::last_error_line(
            &output,
            "the Weles activity read did not complete",
        )));
    }
    Ok(output.stdout)
}
async fn weles_activity(target: &str, json_output: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;
    let output = read_weles_activity(&resolved, &runner)
        .await
        .map_err(|error| {
            CmdError::click(format!("{target}: cannot read Weles activity: {error}"))
        })?;
    let document = output
        .lines()
        .filter_map(|line| line.trim().strip_prefix(WELES_ACTIVITY_MARKER))
        .next_back()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{target}: the Weles activity read printed no report line; inspect the host runtime"
            ))
        })?;
    let mut report: Value = serde_json::from_str(document).map_err(|error| {
        CmdError::click(format!(
            "{target}: the Weles activity report is not readable JSON: {error}"
        ))
    })?;
    if let Some(object) = report.as_object_mut() {
        object.insert("kind".to_string(), json!("weles-activity"));
    }
    if json_output {
        print_json(&report);
        return Ok(());
    }
    let worker = &report["worker"];
    println!(
        "{target}: worker {} staged, {} newest installed, API {} on {}",
        worker["staged_release"].as_str().unwrap_or("unknown"),
        worker["newest_release"].as_str().unwrap_or("unknown"),
        if report["api"]["listening"].as_bool().unwrap_or_default() {
            "answering"
        } else {
            "silent"
        },
        report["api"]["endpoint"]
            .as_str()
            .unwrap_or("unknown endpoint"),
    );
    let runs = report["runs"].as_array().map_or(&[][..], Vec::as_slice);
    println!(
        "{target}: {} recorded run(s), {} newest below",
        report["run_total"].as_u64().unwrap_or_default(),
        runs.len()
    );
    for run in runs {
        println!(
            "  {:<10} {:<22} {:<38} {}",
            run["status"].as_str().unwrap_or("unknown"),
            run["action"].as_str().unwrap_or("unknown action"),
            run["id"].as_str().unwrap_or("-"),
            run["updated_at"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

async fn run_weles_browser_task(
    target: &str,
    plan: &Value,
    json_output: bool,
) -> Result<(), CmdError> {
    let url = required_text(Some(plan), "url")?;
    let mut objective = required_text(Some(plan), "objective")?.to_string();
    if let Some(path) = objective.strip_prefix('@') {
        objective = std::fs::read_to_string(path)
            .map_err(|error| {
                CmdError::usage(format!(
                    "workload objective file {path} cannot be read: {error}"
                ))
            })?
            .trim()
            .to_string();
    } else {
        objective = objective.trim().to_string();
    }
    if objective.is_empty() {
        return Err(CmdError::usage(
            "weles-browser-task plan declares an empty objective; add the task to the plan",
        ));
    }
    let session_label = required_text(Some(plan), "session_label")?;
    let action = plan
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or(crate::deploy::weles_browser_task::DEFAULT_ACTION);
    let allowlist_file = plan
        .get("allowlist_file")
        .and_then(Value::as_str)
        .unwrap_or(crate::deploy::weles_browser_task::DEFAULT_ALLOWLIST_FILE);
    let login_item = plan.get("login_item").and_then(Value::as_str);
    let account_id = plan.get("account_id").and_then(Value::as_str);
    let fresh_profile = boolean(plan, "fresh_profile", false);
    let allow_login = boolean(plan, "allow_login", false);
    let sign_in_origin = plan.get("sign_in_origin").and_then(Value::as_str);
    let sign_in_item = plan.get("sign_in_item").and_then(Value::as_str);
    let defer_fills = boolean(plan, "defer_fills", false);
    let prefill_all = boolean(plan, "prefill_all", false);
    let flow_name = plan.get("flow_name").and_then(Value::as_str);
    let windowed = boolean(plan, "windowed", false);

    if defer_fills && prefill_all {
        return Err(CmdError::usage(
            "weles-browser-task plan cannot enable both defer_fills and prefill_all; choose one",
        ));
    }
    let sign_in =
        match (sign_in_origin, sign_in_item) {
            (None, None) => None,
            (Some(_), None) => {
                return Err(CmdError::usage(
                    "weles-browser-task plan sign_in_origin needs sign_in_item; add the vault item",
                ))
            }
            (None, Some(_)) => return Err(CmdError::usage(
                "weles-browser-task plan sign_in_item needs sign_in_origin; add the page origin",
            )),
            (Some(origin), Some(item)) => {
                if !allow_login {
                    return Err(CmdError::usage(
                        "weles-browser-task plan sign_in_origin requires allow_login=true",
                    ));
                }
                let origin = crate::deploy::weles_browser_task::exact_origin(origin)
                    .map_err(|error| CmdError::usage(error.to_string()))?;
                Some((origin, item))
            }
        };
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::usage(format!("workload plan url is not a URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.username().is_empty() {
        return Err(CmdError::usage(
            "weles-browser-task plan url must be HTTP or HTTPS without embedded credentials",
        ));
    }
    if let Some(item) = login_item {
        let bytes = item.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 128
            || !bytes[0].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item is not a valid Weles item id",
            ));
        }
        if !action.ends_with("_login") {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item requires an action ending in _login",
            ));
        }
        if !allow_login {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item requires allow_login=true",
            ));
        }
    }
    let account_id = match account_id {
        Some(pinned) => Some(
            crate::deploy::weles_capture::checked_account_id(pinned)
                .map_err(|error| CmdError::click(error.to_string()))?
                .to_string(),
        ),
        None => fresh_profile.then(|| format!("stado-fresh-profile-{}", uuid::Uuid::new_v4())),
    };

    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let allowlist =
        crate::deploy::weles_browser_task::host_allowlist(&resolved, allowlist_file, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    crate::deploy::weles_browser_task::ensure_allowed(&resolved.name, action, &allowlist)
        .map_err(|error| CmdError::click(error.to_string()))?;

    let (credential_prefill, credential_deferred) = match &sign_in {
        None => (Vec::new(), Vec::new()),
        Some((origin, item)) => {
            let prefill = crate::deploy::weles_browser_task::issue_sign_in_prefill(
                &resolved,
                origin,
                item,
                crate::deploy::weles_browser_task::REGISTERED_SCOPES_FILE,
                &runner,
            )
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
            if !json_output {
                println!("sign-in:   {origin} as the account in {item}");
                println!(
                    "prefill:    {} field(s) to {}, issued on {}, single-use",
                    prefill.entries.len(),
                    prefill.agents.join(", "),
                    resolved.name
                );
                if !prefill.deferred.is_empty() {
                    println!(
                        "deferred:   {} field(s) handed over unspent, for the page that has them",
                        prefill.deferred.len()
                    );
                }
                if !prefill.unconfirmed.is_empty() {
                    println!(
                        "note:       this channel could not confirm {} in the vault; the worker broker reads it at fill time",
                        prefill.unconfirmed.join(", ")
                    );
                }
            }
            if defer_fills {
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (Vec::new(), all)
            } else if prefill_all {
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (all, Vec::new())
            } else {
                (prefill.entries, prefill.deferred)
            }
        }
    };

    let task = crate::deploy::weles_browser_task::BrowserTask {
        action,
        url: parsed.as_str(),
        objective: &objective,
        session_label,
        login_item,
        account_id: account_id.as_deref(),
        fresh_profile,
        allow_login,
        headless: !windowed,
        credential_prefill,
    };
    let outcome =
        crate::deploy::weles_browser_task::submit(target, &task, flow_name, &credential_deferred)
            .await
            .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    if json_output {
        let mut report = outcome.to_report(&resolved.name, action);
        report.insert("kind".to_string(), json!("weles-browser-task"));
        print_json(&Value::Object(report));
    } else {
        println!("host:      {}", resolved.name);
        println!("action:    {action}");
        println!("run:       {}", outcome.run_id);
        println!("outcome:   {}", if outcome.ok { "ok" } else { "failed" });
        if let Some(code) = outcome.exit_code {
            println!("exit:      {code}");
        }
        if let Some(profile) = &outcome.profile {
            println!(
                "profile:   {}",
                profile["directory"].as_str().unwrap_or("fresh")
            );
        }
        if !outcome.result.is_null() {
            println!("result:    {}", serde_json::to_string(&outcome.result)?);
        }
    }
    if outcome.ok {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{}: {action} run {} did not succeed; inspect its workload status",
            resolved.name, outcome.run_id
        )))
    }
}

const WELES_API_SERVICE: &str = "weles-api";
const WELES_API_PORT: u16 = 8788;
const WELES_API_WORK_DIR: &str = ".stado/build-work/weles-api-managed";
const WELES_SOURCE_REPOSITORY: &str = "https://github.com/wisent-ai/weles.git";
const WELES_API_BUILD_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

async fn refresh_weles_api_runtime(
    target: &str,
    revision: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CmdError::usage(
            "weles-api-runtime plan revision must be one full 40-character git object name",
        ));
    }
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let refused = |step: &str, detail: String| {
        CmdError::click(format!(
            "{}: the runtime was not moved to {revision}; {step} refused: {detail}",
            resolved.name
        ))
    };
    let home = host_channel::remote_home(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let work = format!("{home}/{WELES_API_WORK_DIR}");
    let quoted_work = crate::deploy::shlex_quote(&work);
    let path = crate::deploy::shlex_quote(WELES_API_BUILD_PATH);
    let marker = format!("{work}/.weles-api-revision");
    let cloned = host_channel::remote_test(&resolved, &format!("-d {quoted_work}/.git"), &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let mut steps: Vec<(&str, String)> = Vec::new();
    if !cloned {
        steps.push((
            "clone",
            format!(
                "PATH={path} git clone --filter=blob:none --no-checkout {} {quoted_work}",
                crate::deploy::shlex_quote(WELES_SOURCE_REPOSITORY)
            ),
        ));
    }
    steps.extend([
        (
            "fetch",
            format!("PATH={path} git -C {quoted_work} fetch origin {revision}"),
        ),
        (
            "checkout",
            format!("PATH={path} git -C {quoted_work} checkout --detach --force {revision}"),
        ),
        (
            "install",
            format!("cd {quoted_work} && PATH={path} npm ci --ignore-scripts"),
        ),
        (
            "node-pty helper",
            format!(
                "PATH={path} chmod u=rwx,go=rx {quoted_work}/node_modules/node-pty/prebuilds/*/spawn-helper \
                 {quoted_work}/node_modules/node-pty/build/Release/spawn-helper 2>/dev/null || true"
            ),
        ),
        (
            "recording dependency",
            format!("cd {quoted_work} && PATH={path} npx --no-install playwright install ffmpeg"),
        ),
        (
            "build",
            format!("cd {quoted_work} && PATH={path} npm run build"),
        ),
        (
            "record",
            format!(
                "PATH={path} printf '%s\\n' {} > {}",
                crate::deploy::shlex_quote(revision),
                crate::deploy::shlex_quote(&marker)
            ),
        ),
    ]);
    for (step, command) in steps {
        let output = host_channel::run_command(&resolved, &command, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !output.ok() {
            return Err(refused(
                step,
                host_channel::last_error_line(&output, "no output"),
            ));
        }
    }

    let recorded = host_channel::run_command(
        &resolved,
        &format!("cat {}", crate::deploy::shlex_quote(&marker)),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let observed = recorded.stdout.trim();
    if observed != revision {
        return Err(refused(
            "readback",
            format!(
                "the host recorded {} in {marker}",
                if observed.is_empty() {
                    "nothing"
                } else {
                    observed
                }
            ),
        ));
    }

    let listeners = host_channel::run_command(
        &resolved,
        &format!("PATH={path} lsof -tiTCP:{WELES_API_PORT} -sTCP:LISTEN 2>/dev/null || true"),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let mut ended = Vec::new();
    for pid in listeners
        .stdout
        .split_whitespace()
        .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
    {
        let described = host_channel::run_command(
            &resolved,
            &format!("PATH={path} ps -p {pid} -o command= 2>/dev/null || true"),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
        let command = described.stdout.trim();
        if command.is_empty() {
            continue;
        }
        if !command.contains("weles-api-server") && !command.contains("weles-api-launcher") {
            return Err(refused(
                "port takeover",
                format!(
                    "port {WELES_API_PORT} is held by pid {pid}, which is not a Weles API: {command}"
                ),
            ));
        }
        let stopped =
            host_channel::run_command(&resolved, &format!("PATH={path} kill -TERM {pid}"), &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        if !stopped.ok() {
            return Err(refused(
                "port takeover",
                format!(
                    "pid {pid} on port {WELES_API_PORT} refused SIGTERM: {}",
                    host_channel::last_error_line(&stopped, "no output")
                ),
            ));
        }
        ended.push(pid.to_string());
        if !json_output {
            println!(
                "{}: ended unowned Weles API pid {pid} on port {WELES_API_PORT}",
                resolved.name
            );
        }
    }
    if json_output && !ended.is_empty() {
        eprintln!(
            "{} ended unowned Weles API pid(s) {} before the managed restart",
            resolved.name,
            ended.join(", ")
        );
    }
    crate::cli::service::restart(
        WELES_API_SERVICE,
        Some(&resolved.name),
        None,
        None,
        json_output,
    )
    .await?;
    if !json_output {
        println!(
            "{}: {WELES_API_SERVICE} now serves {revision}",
            resolved.name
        );
    }
    Ok(())
}

fn current_workspace() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| validate_component("workspace", name).is_ok())
        .unwrap_or_else(|| "__home__".to_string())
}

const CHECKOUT_ROOT: &str = "Documents/CodingProjects/Wisent";
const HOME_WORKSPACE: &str = "__home__";
const MANAGED_JEDEN: &str = ".stado/bin/jeden";
const MANAGED_JEDEN_LAUNCHER: &str = ".stado/bin/jeden-run-with-stado";
const MANAGED_SANDBOX_HELPER: &str = ".stado/bin/jeden-sandbox-helper";
const MANAGED_STADO: &str = ".stado/bin/stado";
const PLACEMENT_PREFIX: &str = "STADO_JEDEN_PLACEMENT ";

async fn connect_jeden(
    workspace: &str,
    requested_target: Option<&str>,
    resume: Option<&str>,
) -> Result<(), CmdError> {
    validate_component("workspace", workspace)?;
    if let Some(session) = resume {
        validate_component("resume session", session)?;
    }
    let (registry, canonical) = match crate::targets::fetch_registry_or_last_good().await {
        Ok((registry, notice)) => {
            if let Some(notice) = notice {
                crate::targets::report_registry_notice(&notice);
            }
            let canonical = registry.staleness_seconds.is_none();
            (registry, canonical)
        }
        Err(_) => (
            crate::targets::load_bundled_registry()
                .map_err(|error| CmdError::click(error.to_string()))?,
            false,
        ),
    };
    let mut candidates = if let Some(name) = requested_target {
        let target = host_channel::resolve_target(&registry, name)
            .map_err(|error| CmdError::click(error.to_string()))?
            .clone();
        if !canonical && !host_channel::target_is_this_host(&target) {
            return Err(CmdError::click(
                "canonical registry is unavailable; refresh it before a remote Jeden reconnect",
            ));
        }
        vec![target]
    } else {
        let capacity = live_capacity().await;
        let mut targets = registry
            .targets
            .iter()
            .filter(|target| {
                target.is_provider(crate::capabilities::ProviderId::Local)
                    && (host_channel::target_is_this_host(target)
                        || (canonical && target.has_ssh_connection()))
            })
            .cloned()
            .collect::<Vec<_>>();
        targets.sort_by(|left, right| {
            target_score(right, &capacity)
                .cmp(&target_score(left, &capacity))
                .then_with(|| left.name.cmp(&right.name))
        });
        targets
    };
    if candidates.is_empty() {
        return Err(CmdError::click(
            "the registry declares no reachable local host for jeden-session; add it to the canonical registry",
        ));
    }
    let runner = crate::deploy::production_runner();
    let mut refusals = Vec::new();
    for target in candidates.drain(..) {
        let checkout = checkout_path(workspace);
        let resume_probe = resume
            .map(|session| format!("test -d \"$HOME\"/.jeden/sessions/{session}\\n"))
            .unwrap_or_default();
        let probe = format!(
            "set -e\\ntest -d \"$HOME\"/{checkout}\\n{resume_probe}test -x \"$HOME\"/{MANAGED_JEDEN}\\ntest -x \"$HOME\"/{MANAGED_JEDEN_LAUNCHER}\\ntest -x \"$HOME\"/{MANAGED_SANDBOX_HELPER}\\ntest -x \"$HOME\"/{MANAGED_STADO}\\nprintf ready\\n",
        );
        match host_channel::run_script(&target, &probe, &runner).await {
            Ok(output) if output.ok() && output.stdout.trim() == "ready" => {
                return attach_jeden(target, workspace, &checkout).await;
            }
            Ok(output) => refusals.push(format!(
                "{}: {}",
                target.name,
                host_channel::last_error_line(
                    &output,
                    "workspace, durable session ledger, or managed Jeden runtime is unavailable"
                )
            )),
            Err(error) => refusals.push(format!("{}: {error}", target.name)),
        }
    }
    Err(CmdError::click(format!(
        "no Stado host can run jeden-session in {workspace}; {}",
        refusals.join("; ")
    )))
}

fn validate_component(label: &str, value: &str) -> Result<(), CmdError> {
    let bytes = value.as_bytes();
    let safe = !bytes.is_empty()
        && value != "."
        && value != ".."
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if safe {
        Ok(())
    } else {
        Err(CmdError::usage(format!(
            "{label} must contain only letters, numbers, dot, dash, or underscore"
        )))
    }
}

fn checkout_path(workspace: &str) -> String {
    if workspace == HOME_WORKSPACE {
        String::new()
    } else {
        format!("{CHECKOUT_ROOT}/{workspace}")
    }
}

async fn live_capacity() -> Vec<Value> {
    let Ok(store) = crate::queue::submit::default_store("").await else {
        return Vec::new();
    };
    crate::queue::capacity::read_consumer_capacity(&store)
        .await
        .map(|entries| entries.into_values().collect())
        .unwrap_or_default()
}

fn target_score(target: &ComputeTarget, capacity: &[Value]) -> i64 {
    let hostnames = target
        .hostnames
        .iter()
        .map(|host| crate::targets::normalize_hostname(host))
        .collect::<Vec<_>>();
    let live = capacity
        .iter()
        .filter(|entry| entry.get("kind").and_then(Value::as_str) == Some("local"))
        .find(|entry| {
            let consumer = entry
                .get("consumer_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            hostnames.iter().any(|host| {
                consumer == format!("local-{host}")
                    || consumer
                        .strip_prefix("local-")
                        .is_some_and(|value| crate::targets::normalize_hostname(value) == *host)
            })
        });
    let accepting = live
        .and_then(|entry| entry.get("accepting_jobs"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let available_cpu_cores = live
        .and_then(|entry| entry.get("available_cpu_cores"))
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .max(0);
    let live_bonus = if accepting { 1_000_000 } else { 0 };
    let local_bonus = i64::from(host_channel::target_is_this_host(target));
    live_bonus + available_cpu_cores.saturating_mul(1_000) + local_bonus
}

async fn attach_jeden(
    target: ComputeTarget,
    workspace: &str,
    checkout: &str,
) -> Result<(), CmdError> {
    eprintln!(
        "{PLACEMENT_PREFIX}{}",
        serde_json::to_string(&json!({
            "kind": "jeden-session",
            "target": target.name,
            "workspace": workspace,
            "cwd": format!("~/{checkout}"),
            "ledger": "~/.jeden/sessions",
        }))?
    );
    let status = if host_channel::target_is_this_host(&target) {
        tokio::process::Command::new(expand_home(MANAGED_JEDEN_LAUNCHER)?)
            .arg("rpc")
            .env("JEDEN_STADO_BIN", expand_home(MANAGED_STADO)?)
            .env("JEDEN_BIN", expand_home(MANAGED_JEDEN)?)
            .current_dir(expand_home(checkout)?)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?
    } else {
        let connection =
            host_channel::select_ssh_connection(&target, &crate::deploy::production_runner())
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        let key = ssh_key::materialize(&target.name)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let mut argv = host_channel::ssh_options(connection.destination);
        argv.insert(1, "-T".to_string());
        argv.push(format!(
            "cd \"$HOME\"/{checkout}; export JEDEN_STADO_BIN=\"$HOME\"/{MANAGED_STADO} JEDEN_BIN=\"$HOME\"/{MANAGED_JEDEN}; exec \"$HOME\"/{MANAGED_JEDEN_LAUNCHER} rpc"
        ));
        let argv = ssh_key::add_identity(argv, &key)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| CmdError::click("registry SSH channel is empty; repair the target"))?;
        let result = tokio::process::Command::new(program)
            .args(arguments)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .status()
            .await?;
        drop(key);
        result
    };
    if status.success() {
        Ok(())
    } else {
        Err(CmdError::silent(status.code().unwrap_or(1)))
    }
}

fn expand_home(path: &str) -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| CmdError::click("HOME is not set; set it before attaching"))?;
    Ok(std::path::PathBuf::from(home).join(path))
}
