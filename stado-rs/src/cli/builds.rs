//! `stado builds` — native build recipes in the canonical registry.
//!
//! A recipe names a repository, a branch, one POSIX sh build command, the
//! artifact paths the checkout leaves behind and the release platforms it is
//! built for. The control-plane poller (`scheduler::builds`) enqueues one
//! build job PER PLATFORM whenever the branch head moves, and records the
//! outcome per platform under the recipe's `runs` map; this command family is
//! the operator surface for the recipes themselves: list, add, remove,
//! enable, disable, run-now and status.
//!
//! A run's `version` is the exact git tag at the built commit (leading `v`
//! stripped) when that tag is an exact semantic version, and nothing
//! otherwise — a build of an untagged commit produces artifacts with no
//! version to declare. With `--auto-declare`, a successful run that has a
//! version writes `managed_versions` for every registry host on that
//! platform, through the same code path `stado host declare-version` uses.
//!
//! Boundary: builds publish artifacts and record versions. They never write
//! `release_control.products[...]` desired state — promoting a *signed*
//! release verifies manifests and signatures and stays the deliberate,
//! separate `stado release promote` step.
//!
//! Every mutation is a fenced read-modify-write of the canonical registry
//! document — the same raw-document read-versioned + compare-and-swap path
//! `stado host declare-version` uses — so two concurrent writers cannot
//! silently drop each other's edit. Mutations edit the raw JSON document,
//! never a re-serialized [`crate::targets::Registry`]: re-serializing the
//! typed model is a surgical key change plus a rewrite of every part it
//! never touches, and `Registry::to_document` itself warns it drops what
//! the loader drops.

use std::collections::BTreeMap;

use clap::Subcommand;
use serde_json::{json, Map, Value};

use crate::deploy::products;
use crate::models::isoformat_utc;
use crate::queue::runs::ALL_PREFIXES;
use crate::queue::submit::{default_store, submit_job, SubmitOptions};
use crate::targets::{
    load_registry_from_str, platform_job_os_arch, read_build_recipes, BuildRecipe, BuildRun,
    Registry, RegistryStore, BUILDS_KEY,
};

use super::CmdError;

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
        /// Release platform to build for, e.g. `darwin-arm64` (repeatable,
        /// at least one). Each platform gets its own build job, claimed only
        /// by a worker that is actually that platform.
        #[arg(long = "platform", required = true)]
        platforms: Vec<String>,
        /// Declare a successful run's tag version on every registry host of
        /// that platform. Never promotes a signed release.
        #[arg(long)]
        auto_declare: bool,
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
    /// Enqueue one build job per platform for a recipe now, ignoring the
    /// poll cadence.
    Run {
        name: String,
        /// Emit the submitted job ids and updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one recipe and the state of its per-platform build jobs.
    Status {
        name: String,
        /// Emit the recipe and job states as JSON.
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
            platforms,
            auto_declare,
            interval_seconds,
        } => {
            add(
                &name,
                &repo,
                &branch,
                &command,
                artifacts,
                platforms,
                auto_declare,
                interval_seconds,
            )
            .await
        }
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

/// Every `--platform` word resolved against the published platform table,
/// in the order given and without repeats: an unknown word is a usage error
/// naming the accepted ones, and asking twice for the same platform builds
/// it once.
fn canonical_platforms(platforms: &[String]) -> Result<Vec<String>, CmdError> {
    let mut canonical: Vec<String> = Vec::with_capacity(platforms.len());
    for platform in platforms {
        let word = products::managed_platform(platform.trim())
            .map_err(|error| CmdError::usage(error.to_string()))?;
        if !canonical.iter().any(|seen| seen == word) {
            canonical.push(word.to_string());
        }
    }
    if canonical.is_empty() {
        return Err(CmdError::usage(
            "--platform must name at least one platform",
        ));
    }
    Ok(canonical)
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

/// The platforms a recipe has something to say about: the ones it declares,
/// then any platform that still carries a recorded run after being dropped
/// from the declaration — the run happened, and hiding it would hide it for
/// good.
fn reported_platforms(recipe: &BuildRecipe) -> Vec<String> {
    let mut platforms = recipe.platforms.clone();
    for platform in recipe.runs.keys() {
        if !platforms.iter().any(|declared| declared == platform) {
            platforms.push(platform.clone());
        }
    }
    platforms
}

/// The run columns `list` and `status` share: platform, status, version,
/// declared, when. A platform with no recorded run reads `never`, not blank.
///
/// `when` is padded to the widest timestamp
/// [`crate::models::isoformat_utc`] emits, so `status` can append its own
/// column after it; a caller for whom `when` IS the last column trims the
/// row.
fn run_row(platform: &str, run: Option<&BuildRun>) -> String {
    match run {
        Some(run) => format!(
            "{platform:<14} {:<10} {:<14} {:<9} {:<32}",
            run.status,
            run.version.as_deref().unwrap_or("-"),
            run.declared,
            run.at
        ),
        None => format!(
            "{platform:<14} {:<10} {:<14} {:<9} {:<32}",
            "never", "-", "-", "-"
        ),
    }
}

/// The header for [`run_row`]'s columns, padded by the same widths so the two
/// cannot drift apart.
fn run_header() -> String {
    format!(
        "  {:<14} {:<10} {:<14} {:<9} {:<32}",
        "PLATFORM", "STATUS", "VERSION", "DECLARED", "WHEN"
    )
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
        "{:<24} {:<44} {:<8} {:<13} LAST SEEN",
        "NAME", "REPO@REF", "ENABLED", "AUTO-DECLARE"
    );
    println!("{}", "-".repeat(110));
    for recipe in &recipes {
        let source = format!("{}@{}", recipe.repo, recipe.branch);
        let seen = recipe
            .last_seen_ref
            .as_deref()
            .map(|sha| sha.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<24} {:<44} {:<8} {:<13} {seen}",
            recipe.name, source, recipe.enabled, recipe.auto_declare
        );
        let platforms = reported_platforms(recipe);
        if platforms.is_empty() {
            println!("  (no platforms declared; re-add the recipe with --platform)");
            continue;
        }
        println!("{}", run_header().trim_end());
        for platform in &platforms {
            println!(
                "  {}",
                run_row(platform, recipe.runs.get(platform)).trim_end()
            );
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn add(
    name: &str,
    repo: &str,
    branch: &str,
    command: &str,
    artifacts: Vec<String>,
    platforms: Vec<String>,
    auto_declare: bool,
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
    let platforms = canonical_platforms(&platforms)?;
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
        platforms: platforms.clone(),
        auto_declare,
        enabled: false,
        interval_seconds,
        last_seen_ref: None,
        runs: BTreeMap::new(),
    })?);
    super::registry::push_document_if(&document, &generation).await?;
    println!(
        "{name}: added for {} (disabled; enable with `stado builds enable {name}`)",
        platforms.join(", ")
    );
    if auto_declare {
        println!(
            "{name}: auto-declare on — a successful tagged build declares that version on every \
             matching host (signed promotion stays `stado release promote`)"
        );
    }
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
    println!("{name}: {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

/// Enqueue one build job per declared platform now, poll cadence and enable
/// flag notwithstanding — `run` is the operator saying "build it", and saying
/// it about a disabled recipe is how a recipe is vetted before it is enabled.
///
/// The jobs are submitted before the registry write, and each carries the
/// platform as `platform_os`/`architecture` so only a worker of that
/// platform can claim it. A submit that fails leaves the recipe untouched.
async fn run_now(name: &str, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = super::registry::fetch_versioned_document().await?;
    let recipe: BuildRecipe = {
        let entry = find_entry(builds_array(&mut document)?, name)?;
        serde_json::from_value(entry.clone()).map_err(|error| {
            CmdError::click(format!("build recipe {name:?} does not parse: {error}"))
        })?
    };
    let platforms = canonical_platforms(&recipe.platforms).map_err(|error| {
        CmdError::click(format!(
            "build recipe {name:?} declares no usable platform ({error}); re-add it with --platform"
        ))
    })?;
    let command = crate::scheduler::builds::build_job_command(&recipe);
    let at = isoformat_utc(chrono::Utc::now());
    let mut jobs = Map::new();
    let mut submitted: Vec<(String, BuildRun)> = Vec::with_capacity(platforms.len());
    for platform in &platforms {
        let (platform_os, architecture) = platform_job_os_arch(platform).ok_or_else(|| {
            CmdError::click(format!("{platform:?} names no job platform/architecture"))
        })?;
        let job = submit_job(
            &command,
            &SubmitOptions {
                platform_os: platform_os.to_string(),
                architecture: architecture.to_string(),
                ..SubmitOptions::default()
            },
        )
        .await?;
        jobs.insert(platform.clone(), Value::String(job.job_id.clone()));
        submitted.push((
            platform.clone(),
            BuildRun {
                status: "running".to_string(),
                at: at.clone(),
                job_id: job.job_id,
                artifact_uris: Vec::new(),
                version: None,
                declared: false,
            },
        ));
    }
    let entry = find_entry(builds_array(&mut document)?, name)?;
    let object = entry
        .as_object_mut()
        .expect("a named recipe entry is an object");
    let runs = object
        .entry("runs".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("build recipe {name:?}: runs is not an object")))?;
    for (platform, run) in &submitted {
        runs.insert(platform.clone(), serde_json::to_value(run)?);
    }
    let updated = normalized_recipe_json(entry);
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&json!({
            "name": name,
            "platforms": platforms,
            "jobs": jobs,
            "recipe": updated,
        }));
    }
    for (platform, run) in &submitted {
        println!("{name}: submitted build job {} for {platform}", run.job_id);
    }
    Ok(())
}

async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let registry = read_registry().await?;
    let recipes = read_build_recipes(&registry);
    let index = recipe_index(&recipes, name)?;
    let recipe = &recipes[index];
    let mut job_states: BTreeMap<String, Option<&'static str>> = BTreeMap::new();
    if !recipe.runs.is_empty() {
        let store = default_store(crate::config::bucket()).await?;
        for (platform, run) in &recipe.runs {
            let mut found = None;
            for state in ALL_PREFIXES {
                if store.read_job(state, &run.job_id).await?.is_some() {
                    found = Some(state);
                    break;
                }
            }
            job_states.insert(platform.clone(), found);
        }
    }
    if json {
        return print_json(&json!({
            "recipe": recipe_json(recipe)?,
            "job_states": job_states,
        }));
    }
    println!("name:         {}", recipe.name);
    println!("source:       {}@{}", recipe.repo, recipe.branch);
    println!("command:      {}", recipe.command);
    println!("artifacts:    {}", recipe.artifacts.join(", "));
    println!("platforms:    {}", recipe.platforms.join(", "));
    println!("enabled:      {}", recipe.enabled);
    println!("auto-declare: {}", recipe.auto_declare);
    println!("interval:     {}s", recipe.interval_seconds);
    println!(
        "last seen:    {}",
        recipe.last_seen_ref.as_deref().unwrap_or("-")
    );
    let platforms = reported_platforms(recipe);
    if platforms.is_empty() {
        println!("runs:         none (no platforms declared)");
        return Ok(());
    }
    println!("runs:");
    println!("{}  JOB STATE", run_header());
    for platform in &platforms {
        let run = recipe.runs.get(platform);
        let state = match job_states.get(platform) {
            Some(Some(state)) => state,
            Some(None) => "(not in queue)",
            None => "-",
        };
        println!("  {}  {state}", run_row(platform, run));
    }
    for (platform, run) in &recipe.runs {
        for uri in &run.artifact_uris {
            println!("artifact:     {platform} {uri}");
        }
    }
    Ok(())
}
