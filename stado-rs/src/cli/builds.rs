//! `stado builds` — native build recipes in the canonical registry.
//!
//! A recipe names a repository, a branch, one POSIX sh build command and the
//! artifact paths the checkout leaves behind. The control-plane poller
//! (`scheduler::builds`) enqueues one build job whenever the branch head
//! moves; this command family is the operator surface for the recipes
//! themselves: list, add, remove, enable, disable, run-now and status.
//!
//! v1 boundary: builds produce artifacts and job results only. Version
//! declaration and fleet delivery remain manual (`stado host
//! declare-version`, `converge --apply`).
//!
//! Every mutation is a fenced read-modify-write of the canonical registry
//! document — the same raw-document read-versioned + compare-and-swap path
//! `stado host declare-version` uses — so two concurrent writers cannot
//! silently drop each other's edit. Mutations edit the raw JSON document,
//! never a re-serialized [`crate::targets::Registry`]: re-serializing the
//! typed model is a surgical key change plus a rewrite of every part it
//! never touched, and `Registry::to_document` itself warns it drops what
//! the loader drops.

use chrono::Utc;
use clap::Subcommand;
use serde_json::{json, Value};

use crate::queue::submit::{default_store, submit_job, SubmitOptions};
use crate::targets::{
    load_registry_from_str, read_build_recipes, BuildRecipe, BuildRun, Registry, RegistryStore,
    BUILDS_KEY,
};

use super::CmdError;

/// Canonical lifecycle states, in the order `stado builds status` probes
/// them for the recipe's last job (mirrors `cli::status`).
const JOB_STATES: &[&str] = &[
    "running",
    "queue",
    "completed",
    "uploaded",
    "failed",
    "cancelled",
];

#[derive(Subcommand)]
pub enum BuildsCommands {
    /// List every build recipe in the registry.
    List {
        /// Emit the machine-readable recipe array.
        #[arg(long)]
        json: bool,
    },
    /// Add a build recipe. Recipes start disabled; enable one explicitly.
    Add {
        /// Unique kebab-case recipe name.
        #[arg(long)]
        name: String,
        /// HTTPS clone URL of the repository to build.
        #[arg(long)]
        repo: String,
        /// Branch the poller watches.
        #[arg(long)]
        branch: String,
        /// Single POSIX sh build command run in the checkout.
        #[arg(long)]
        command: String,
        /// Path in the checkout to upload as a build artifact (repeatable).
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
        /// Poll cadence in seconds (default 300).
        #[arg(long, default_value_t = 300)]
        interval_seconds: u64,
    },
    /// Remove a build recipe.
    Remove { name: String },
    /// Enable a recipe: the control-plane poller starts building it.
    Enable {
        name: String,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Disable a recipe without deleting it.
    Disable {
        name: String,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enqueue one build job for a recipe now, ignoring the poll cadence.
    Run {
        name: String,
        /// Emit the submitted job id and updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one recipe and the state of its last build job.
    Status {
        name: String,
        /// Emit the recipe and job state as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: BuildsCommands) -> Result<(), CmdError> {
    match command {
        BuildsCommands::List { json } => list(json).await,
        BuildsCommands::Add {
            name,
            repo,
            branch,
            command,
            artifacts,
            interval_seconds,
        } => add(&name, &repo, &branch, &command, artifacts, interval_seconds).await,
        BuildsCommands::Remove { name } => remove(&name).await,
        BuildsCommands::Enable { name, json } => set_enabled(&name, true, json).await,
        BuildsCommands::Disable { name, json } => set_enabled(&name, false, json).await,
        BuildsCommands::Run { name, json } => run_now(&name, json).await,
        BuildsCommands::Status { name, json } => status(&name, json).await,
    }
}

/// `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$` — kebab-case recipe names, so a name
/// is safe verbatim in a shell word, a JSON key and a table column.
fn is_recipe_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    let inner_ok = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
    let edge_ok = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    match (bytes.first(), bytes.last()) {
        (Some(&first), Some(&last)) if edge_ok(first) && edge_ok(last) => {
            bytes.iter().all(|&byte| inner_ok(byte))
        }
        _ => false,
    }
}

/// The mutable `builds` array of the raw registry document, created empty
/// when the document does not carry one yet.
fn builds_array(document: &mut Value) -> Result<&mut Vec<Value>, CmdError> {
    document
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry: must be an object"))?
        .entry(BUILDS_KEY.to_string())
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CmdError::click("registry.builds: must be an array"))
}

fn entry_name(entry: &Value) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

fn find_entry<'a>(entries: &'a mut [Value], name: &str) -> Result<&'a mut Value, CmdError> {
    entries
        .iter_mut()
        .find(|entry| entry_name(entry) == Some(name))
        .ok_or_else(|| CmdError::click(format!("registry declares no build recipe {name:?}")))
}

/// A raw recipe entry as the contract shape: parsed and re-serialized so
/// serde defaults fill absent fields, verbatim when it does not parse.
fn normalized_recipe_json(entry: &Value) -> Value {
    serde_json::from_value::<BuildRecipe>(entry.clone())
        .ok()
        .and_then(|recipe| serde_json::to_value(recipe).ok())
        .unwrap_or_else(|| entry.clone())
}

/// The canonical registry for read-only commands: an absent document reads
/// as an empty registry rather than an error, so `builds list` answers on a
/// fresh deployment.
async fn read_registry() -> Result<Registry, CmdError> {
    let store = RegistryStore::open().await?;
    match store.read_versioned().await? {
        Some(blob) => load_registry_from_str(&blob.content)
            .map_err(|error| CmdError::click(format!("registry did not parse: {error}"))),
        None => Ok(Registry::default()),
    }
}

fn recipe_index(recipes: &[BuildRecipe], name: &str) -> Result<usize, CmdError> {
    recipes
        .iter()
        .position(|recipe| recipe.name == name)
        .ok_or_else(|| CmdError::click(format!("registry declares no build recipe {name:?}")))
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn recipe_json(recipe: &BuildRecipe) -> Result<Value, CmdError> {
    Ok(serde_json::to_value(recipe)?)
}

async fn list(json: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    if json {
        return print_json(&serde_json::to_value(&recipes)?);
    }
    if recipes.is_empty() {
        println!("(no build recipes; add one with `stado builds add`)");
        return Ok(());
    }
    println!(
        "{:<24} {:<44} {:<8} {:<10} LAST RUN",
        "NAME", "REPO@REF", "ENABLED", "LAST SEEN"
    );
    println!("{}", "-".repeat(110));
    for recipe in &recipes {
        let source = format!("{}@{}", recipe.repo, recipe.branch);
        let seen = recipe
            .last_seen_ref
            .as_deref()
            .map(|sha| sha.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        let last_run = recipe
            .last_run
            .as_ref()
            .map(|run| format!("{} {} ({})", run.status, run.job_id, run.at))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<24} {:<44} {:<8} {:<10} {last_run}",
            recipe.name, source, recipe.enabled, seen
        );
    }
    Ok(())
}

async fn add(
    name: &str,
    repo: &str,
    branch: &str,
    command: &str,
    artifacts: Vec<String>,
    interval_seconds: u64,
) -> Result<(), CmdError> {
    if !is_recipe_name(name) {
        return Err(CmdError::usage(
            "--name must be kebab-case: lowercase letters, digits and '-'",
        ));
    }
    if !repo.starts_with("https://") {
        return Err(CmdError::usage("--repo must be an https:// clone URL"));
    }
    if branch.trim().is_empty() {
        return Err(CmdError::usage("--branch must name a branch"));
    }
    if command.trim().is_empty() {
        return Err(CmdError::usage("--command must be a build command"));
    }
    if artifacts.iter().any(|path| {
        let path = path.trim();
        path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..")
    }) {
        return Err(CmdError::usage(
            "--artifact paths must be relative to the checkout, without '..'",
        ));
    }
    if interval_seconds == 0 {
        return Err(CmdError::usage("--interval-seconds must be positive"));
    }
    let (mut document, generation) = super::registry::fetch_versioned_document().await?;
    let entries = builds_array(&mut document)?;
    if entries.iter().any(|entry| entry_name(entry) == Some(name)) {
        return Err(CmdError::click(format!(
            "build recipe {name:?} already exists"
        )));
    }
    entries.push(serde_json::to_value(BuildRecipe {
        name: name.to_string(),
        repo: repo.to_string(),
        branch: branch.to_string(),
        command: command.to_string(),
        artifacts,
        enabled: false,
        interval_seconds,
        last_seen_ref: None,
        last_run: None,
    })?);
    super::registry::push_document_if(&document, &generation).await?;
    println!("{name}: added (disabled; enable with `stado builds enable {name}`)");
    Ok(())
}

async fn remove(name: &str) -> Result<(), CmdError> {
    let (mut document, generation) = super::registry::fetch_versioned_document().await?;
    let entries = builds_array(&mut document)?;
    let before = entries.len();
    entries.retain(|entry| entry_name(entry) != Some(name));
    if entries.len() == before {
        return Err(CmdError::click(format!(
            "registry declares no build recipe {name:?}"
        )));
    }
    super::registry::push_document_if(&document, &generation).await?;
    println!("{name}: removed");
    Ok(())
}

async fn set_enabled(name: &str, enabled: bool, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = super::registry::fetch_versioned_document().await?;
    let entry = find_entry(builds_array(&mut document)?, name)?;
    entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("build recipe {name:?} must be an object")))?
        .insert("enabled".to_string(), Value::Bool(enabled));
    let updated = normalized_recipe_json(entry);
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&updated);
    }
    println!(
        "{name}: {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Enqueue one build job now, poll cadence and enable flag notwithstanding —
/// `run` is the operator saying "build it", and saying it about a disabled
/// recipe is how a recipe is vetted before it is enabled.
async fn run_now(name: &str, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = super::registry::fetch_versioned_document().await?;
    let entry = find_entry(builds_array(&mut document)?, name)?;
    let recipe: BuildRecipe = serde_json::from_value(entry.clone())
        .map_err(|error| CmdError::click(format!("build recipe {name:?} does not parse: {error}")))?;
    let command = crate::scheduler::builds::build_job_command(&recipe);
    let job = submit_job(&command, &SubmitOptions::default()).await?;
    entry
        .as_object_mut()
        .expect("a named recipe entry is an object")
        .insert(
            "last_run".to_string(),
            serde_json::to_value(BuildRun {
                status: "running".to_string(),
                at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                job_id: job.job_id.clone(),
                artifact_uris: Vec::new(),
            })?,
        );
    let updated = normalized_recipe_json(entry);
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&json!({
            "name": name,
            "job_id": job.job_id,
            "recipe": updated,
        }));
    }
    println!("{name}: submitted build job {}", job.job_id);
    Ok(())
}

async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    let index = recipe_index(&recipes, name)?;
    let recipe = &recipes[index];
    let mut job_state: Option<&str> = None;
    if let Some(run) = &recipe.last_run {
        let store = default_store(crate::config::bucket()).await?;
        for state in JOB_STATES {
            if store.read_job(state, &run.job_id).await?.is_some() {
                job_state = Some(state);
                break;
            }
        }
    }
    if json {
        return print_json(&json!({
            "recipe": recipe_json(recipe)?,
            "job_state": job_state,
        }));
    }
    println!("name:      {}", recipe.name);
    println!("source:    {}@{}", recipe.repo, recipe.branch);
    println!("command:   {}", recipe.command);
    println!("artifacts: {}", recipe.artifacts.join(", "));
    println!("enabled:   {}", recipe.enabled);
    println!("interval:  {}s", recipe.interval_seconds);
    println!(
        "last seen: {}",
        recipe.last_seen_ref.as_deref().unwrap_or("-")
    );
    match &recipe.last_run {
        Some(run) => {
            println!("last run:  {} at {} (job {})", run.status, run.at, run.job_id);
            println!("job state: {}", job_state.unwrap_or("(job not found in queue)"));
            for uri in &run.artifact_uris {
                println!("artifact:  {uri}");
            }
        }
        None => println!("last run:  never"),
    }
    Ok(())
}
