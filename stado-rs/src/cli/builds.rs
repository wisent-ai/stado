//! `stado builds` — native build recipes in the canonical registry.
//!
//! A recipe names a repository, a branch, one POSIX sh build command, the
//! artifact paths the checkout leaves behind and the release platforms it is
//! built for. The control-plane poller (`scheduler::builds`) enqueues one
//! build job PER PLATFORM whenever the branch head moves, and records the
//! outcome per platform under the recipe's `runs` map; this command family is
//! the operator surface for the recipes themselves: list, add, edit, remove,
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
    fleet_namespace_mismatch, platform_job_os_arch, read_build_recipes, BuildRecipe, BuildRun,
    Registry, RegistryFetchError, BUILDS_KEY,
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
        /// Emit the created recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Change a recipe's source or build definition in place. Every flag is
    /// optional and a flag not given leaves its field alone; `--artifact` and
    /// `--platform`, when given at all, REPLACE the recorded list. `enabled`
    /// is not editable here: `enable` and `disable` own it.
    Edit {
        name: String,
        /// HTTPS clone URL of the repository to build. Changing it clears the
        /// last seen ref and the recorded runs.
        #[arg(long)]
        repo: Option<String>,
        /// Branch the poller watches. Changing it clears the last seen ref
        /// and the recorded runs.
        #[arg(long)]
        branch: Option<String>,
        /// Single POSIX sh build command run in the checkout.
        #[arg(long)]
        command: Option<String>,
        /// Path in the checkout to upload as a build artifact (repeatable);
        /// the paths given replace the recorded ones.
        #[arg(long = "artifact")]
        artifacts: Vec<String>,
        /// Release platform to build for (repeatable); the platforms given
        /// replace the recorded ones. A newly named platform simply has no
        /// run yet.
        #[arg(long = "platform")]
        platforms: Vec<String>,
        /// Declare a successful run's tag version on every registry host of
        /// that platform. Never promotes a signed release.
        #[arg(long = "auto-declare", overrides_with = "no_auto_declare")]
        auto_declare: bool,
        /// Stop declaring versions from successful runs.
        #[arg(long = "no-auto-declare", overrides_with = "auto_declare")]
        no_auto_declare: bool,
        /// Poll cadence in seconds.
        #[arg(long)]
        interval_seconds: Option<u64>,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove a build recipe.
    Remove {
        name: String,
        /// Emit `{"name": ..., "removed": true}` as JSON.
        #[arg(long)]
        json: bool,
    },
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
            json,
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
                json,
            )
            .await
        }
        BuildsCommands::Edit {
            name,
            repo,
            branch,
            command,
            artifacts,
            platforms,
            auto_declare,
            no_auto_declare,
            interval_seconds,
            json,
        } => {
            edit(
                &name,
                RecipeEdit {
                    repo,
                    branch,
                    command,
                    // An empty repeatable is the flag never given; the
                    // operator cannot ask for an empty list, only for a
                    // different one.
                    artifacts: (!artifacts.is_empty()).then_some(artifacts),
                    platforms: (!platforms.is_empty()).then_some(platforms),
                    // `overrides_with` in both directions leaves at most one
                    // of the pair set, so neither set is "leave it alone".
                    auto_declare: match (auto_declare, no_auto_declare) {
                        (true, false) => Some(true),
                        (false, true) => Some(false),
                        _ => None,
                    },
                    interval_seconds,
                },
                json,
            )
            .await
        }
        BuildsCommands::Remove { name, json } => remove(&name, json).await,
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

/// One check per recipe field, so `add` — which sets every field — and
/// `edit` — which replaces the fields it was given — accept and refuse
/// exactly the same words. A checker never rewrites what it accepts: a
/// recipe stores the repo, branch, command and paths the operator typed.
fn check_repo(repo: &str) -> Result<(), CmdError> {
    if repo.starts_with("https://") {
        return Ok(());
    }
    Err(CmdError::usage("--repo must be an https:// clone URL"))
}

fn check_branch(branch: &str) -> Result<(), CmdError> {
    if branch.trim().is_empty() {
        return Err(CmdError::usage("--branch must name a branch"));
    }
    Ok(())
}

fn check_command(command: &str) -> Result<(), CmdError> {
    if command.trim().is_empty() {
        return Err(CmdError::usage("--command must be a build command"));
    }
    Ok(())
}

/// Artifact paths name something the checkout left behind: relative, inside
/// it, and not empty. An absolute path or a `..` hop would upload a file the
/// build did not produce.
fn check_artifacts(artifacts: &[String]) -> Result<(), CmdError> {
    if artifacts.iter().any(|path| {
        let path = path.trim();
        path.is_empty() || path.starts_with('/') || path.split('/').any(|part| part == "..")
    }) {
        return Err(CmdError::usage(
            "--artifact paths must be relative to the checkout, without '..'",
        ));
    }
    Ok(())
}

fn check_interval_seconds(interval_seconds: u64) -> Result<(), CmdError> {
    if interval_seconds == 0 {
        return Err(CmdError::usage("--interval-seconds must be positive"));
    }
    Ok(())
}

/// The recipe fields `stado builds edit` may replace. `None` is "the
/// operator did not name this flag, leave the field alone", which is why
/// every field is optional even though a stored recipe carries all of them.
///
/// `enabled` is absent deliberately: `enable` and `disable` own it. Whether
/// a recipe builds is a decision an operator takes on purpose, never a side
/// effect of correcting a build command.
struct RecipeEdit {
    repo: Option<String>,
    branch: Option<String>,
    command: Option<String>,
    /// Given at all, the paths REPLACE the recorded list.
    artifacts: Option<Vec<String>>,
    /// Given at all, the platforms REPLACE the recorded list.
    platforms: Option<Vec<String>>,
    auto_declare: Option<bool>,
    interval_seconds: Option<u64>,
}

impl RecipeEdit {
    /// Whether the operator named no field at all. An `edit` that names none
    /// is a mistake, not a request to rewrite an entry with what it already
    /// says.
    fn names_nothing(&self) -> bool {
        self.repo.is_none()
            && self.branch.is_none()
            && self.command.is_none()
            && self.artifacts.is_none()
            && self.platforms.is_none()
            && self.auto_declare.is_none()
            && self.interval_seconds.is_none()
    }

    /// Every named field validated as `add` validates it, with `--platform`
    /// words canonicalized. Validation runs before the registry is read, so
    /// a rejected flag never opens a fenced write.
    fn checked(self) -> Result<Self, CmdError> {
        if let Some(repo) = self.repo.as_deref() {
            check_repo(repo)?;
        }
        if let Some(branch) = self.branch.as_deref() {
            check_branch(branch)?;
        }
        if let Some(command) = self.command.as_deref() {
            check_command(command)?;
        }
        if let Some(artifacts) = self.artifacts.as_deref() {
            check_artifacts(artifacts)?;
        }
        if let Some(interval_seconds) = self.interval_seconds {
            check_interval_seconds(interval_seconds)?;
        }
        let platforms = match self.platforms.as_deref() {
            Some(platforms) => Some(canonical_platforms(platforms)?),
            None => None,
        };
        Ok(Self { platforms, ..self })
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

/// The canonical registry for read-only commands, through the same
/// last-known-good fallback every other CLI read uses
/// ([`crate::targets::fetch_registry_or_last_good_detail`]): when the fleet
/// object API refuses or is unreachable, the answer comes from the cached
/// copy with one stderr line saying so, and the exit code stays 0. An
/// absent document still reads as an empty registry rather than an error,
/// so `builds list` answers on a fresh deployment.
async fn read_registry() -> Result<Registry, CmdError> {
    match crate::targets::fetch_registry_or_last_good_detail().await {
        Ok((registry, copy)) => {
            if let Some(copy) = copy {
                crate::targets::report_registry_notice(&format!(
                    "fleet store unreachable: {}; showing the registry as of {}",
                    copy.cause, copy.read_at
                ));
            }
            Ok(registry)
        }
        Err(RegistryFetchError::Absent { .. }) => Ok(Registry::default()),
        Err(error) => Err(CmdError::click(error.to_string())),
    }
}

/// The fenced registry document for a builds MUTATION, refused when this
/// machine's ambient queue namespace is not the fleet's recorded one
/// ([`fleet_namespace_mismatch`]): a recipe written into another
/// namespace's registry is a recipe the fleet never polls, and a job
/// submitted from it is a job no fleet worker claims. Reads are exempt —
/// they degrade to the last-known-good copy instead ([`read_registry`]).
async fn fetch_mutation_document() -> Result<(Value, String), CmdError> {
    let (document, generation) = super::registry::fetch_versioned_document().await?;
    if let Some(mismatch) = fleet_namespace_mismatch(&document) {
        return Err(CmdError::click(mismatch));
    }
    Ok((document, generation))
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
    json: bool,
) -> Result<(), CmdError> {
    if !is_recipe_name(name) {
        return Err(CmdError::usage(
            "--name must be kebab-case: lowercase letters, digits and '-'",
        ));
    }
    check_repo(repo)?;
    check_branch(branch)?;
    check_command(command)?;
    check_artifacts(&artifacts)?;
    let platforms = canonical_platforms(&platforms)?;
    check_interval_seconds(interval_seconds)?;
    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = builds_array(&mut document)?;
    if entries.iter().any(|entry| entry_name(entry) == Some(name)) {
        return Err(CmdError::click(format!(
            "build recipe {name:?} already exists"
        )));
    }
    let created = recipe_json(&BuildRecipe {
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
    })?;
    entries.push(created.clone());
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&created);
    }
    println!(
        "{name}: added for {} (disabled; enable with `stado builds enable {name}`)",
        platforms.join(", ")
    );
    if auto_declare {
        println!("{name}: {AUTO_DECLARE_ON}");
    }
    Ok(())
}

/// What turning `auto_declare` on means, said the same way by `add` and
/// `edit` so the operator reads one sentence about one behaviour.
const AUTO_DECLARE_ON: &str = "auto-declare on — a successful tagged build declares that version \
                               on every matching host (signed promotion stays `stado release \
                               promote`)";

/// Change a stored recipe's source or build definition in place, one fenced
/// read-modify-write like every other mutation here.
///
/// The state semantics are the substance of this command, because they decide
/// whether the recipe re-fires:
///
/// * a changed `repo` or `branch` is a DIFFERENT source, so `last_seen_ref`
///   and every recorded run are cleared. The runs describe commits of the old
///   source, and a retained head would leave the current head of the new one
///   unbuilt until it happened to move.
/// * a changed command, artifact set, platform set, interval or auto-declare
///   flag says how the SAME source is built, so `last_seen_ref` and the runs
///   are kept: this head has already been built, and the poller only fires
///   when it moves. A newly named platform simply has no run yet; build it
///   now with `stado builds run`.
///
/// A value re-typed as it already stands is not a change: it neither prints
/// as one nor clears anything, and an `edit` in which nothing at all changed
/// does not spend a registry write.
async fn edit(name: &str, fields: RecipeEdit, json: bool) -> Result<(), CmdError> {
    if fields.names_nothing() {
        return Err(CmdError::usage(format!(
            "{name}: nothing to edit — name at least one of --repo, --branch, --command, \
             --artifact, --platform, --interval-seconds, --auto-declare, --no-auto-declare \
             (whether a recipe builds is `stado builds enable`/`disable`)"
        )));
    }
    let fields = fields.checked()?;
    let (mut document, generation) = fetch_mutation_document().await?;
    let entry = find_entry(builds_array(&mut document)?, name)?;
    let object = entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("build recipe {name:?} must be an object")))?;
    let mut changes: Vec<String> = Vec::new();
    // Which halves of the source moved, for one sentence naming both when
    // both did.
    let mut source: Vec<&str> = Vec::new();
    if let Some(repo) = fields.repo {
        if let Some(change) = replace_field(object, "repo", "repo", Value::String(repo)) {
            source.push("repo");
            changes.push(change);
        }
    }
    // `branch` is the `ref` key on the wire; see `BuildRecipe::branch`.
    if let Some(branch) = fields.branch {
        if let Some(change) = replace_field(object, "ref", "branch", Value::String(branch)) {
            source.push("branch");
            changes.push(change);
        }
    }
    if let Some(command) = fields.command {
        changes.extend(replace_field(
            object,
            "command",
            "command",
            Value::String(command),
        ));
    }
    if let Some(artifacts) = fields.artifacts {
        changes.extend(replace_field(
            object,
            "artifacts",
            "artifacts",
            json!(artifacts),
        ));
    }
    // A platform the recipe did not declare before has nothing recorded for
    // it, which is worth saying: the operator asked for a build there.
    let mut gained: Vec<String> = Vec::new();
    if let Some(platforms) = fields.platforms {
        let previous = string_list(object.get("platforms"));
        gained = platforms
            .iter()
            .filter(|platform| !previous.contains(*platform))
            .cloned()
            .collect();
        changes.extend(replace_field(
            object,
            "platforms",
            "platforms",
            json!(platforms),
        ));
    }
    if let Some(auto_declare) = fields.auto_declare {
        changes.extend(replace_field(
            object,
            "auto_declare",
            "auto-declare",
            Value::Bool(auto_declare),
        ));
    }
    if let Some(interval_seconds) = fields.interval_seconds {
        changes.extend(replace_field(
            object,
            "interval_seconds",
            "interval-seconds",
            json!(interval_seconds),
        ));
    }
    // The recorded state, counted before a source change spends it.
    let recorded_ref = object
        .get("last_seen_ref")
        .and_then(Value::as_str)
        .map(short_ref);
    let recorded_runs = object
        .get("runs")
        .and_then(Value::as_object)
        .map_or(0, Map::len);
    if !source.is_empty() {
        // Shaped exactly like a freshly added recipe: the new source has been
        // seen at no head and built by no run.
        object.insert("last_seen_ref".to_string(), Value::Null);
        object.insert("runs".to_string(), Value::Object(Map::new()));
    }
    let updated = normalized_recipe_json(entry);
    if changes.is_empty() {
        // Every value given is already what the entry says. Saying so beats
        // spending a compare-and-swap on a document that would not differ.
        if json {
            return print_json(&updated);
        }
        println!("{name}: unchanged — every value given is already recorded");
        return Ok(());
    }
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&updated);
    }
    for change in &changes {
        println!("{name}: {change}");
    }
    let state = match (&recorded_ref, recorded_runs) {
        (None, 0) => None,
        (sha, runs) => Some(format!(
            "{} and {}",
            sha.as_deref().map_or_else(
                || "no last_seen_ref".to_string(),
                |sha| format!("last_seen_ref {sha}")
            ),
            runs_phrase(runs)
        )),
    };
    match (source.is_empty(), state) {
        (true, None) => println!(
            "{name}: same source, nothing built yet — the next poll builds the current head"
        ),
        (true, Some(state)) => println!(
            "{name}: same source — kept {state}; the next poll builds only when the head moves, \
             so build it now with `stado builds run {name}`"
        ),
        (false, None) => println!(
            "{name}: {} changed — there was no last_seen_ref and no recorded run to clear; the \
             next poll builds the current head",
            source.join(" and ")
        ),
        (false, Some(state)) => println!(
            "{name}: {} changed — cleared {state}; the next poll builds the current head of the \
             new source",
            source.join(" and ")
        ),
    }
    for platform in &gained {
        println!("{name}: {platform} is new and has no run yet");
    }
    if fields.auto_declare == Some(true) {
        println!("{name}: {AUTO_DECLARE_ON}");
    }
    Ok(())
}

/// Replace `key` with `value` unless the entry already says exactly that,
/// returning the sentence naming the change. `None` is "nothing moved": an
/// operator who re-types the current value has changed nothing, and nothing
/// is what gets reported — and, for the source, what gets cleared.
fn replace_field(
    object: &mut Map<String, Value>,
    key: &str,
    label: &str,
    value: Value,
) -> Option<String> {
    let current = object.get(key);
    if current == Some(&value) {
        return None;
    }
    let before = current.map_or_else(|| "-".to_string(), display_field);
    let after = display_field(&value);
    object.insert(key.to_string(), value);
    Some(format!("{label} {before} → {after}"))
}

/// A recipe field as one human phrase: a string bare, a list joined, anything
/// else as the JSON it is.
fn display_field(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(display_field)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// The strings of a raw JSON array field, ignoring entries that are not
/// strings — a hand-written recipe is not trusted to be well typed.
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A recorded-run count as words, so a sentence about one run does not read
/// "1 recorded runs".
fn runs_phrase(count: usize) -> String {
    match count {
        0 => "no recorded run".to_string(),
        1 => "1 recorded run".to_string(),
        many => format!("{many} recorded runs"),
    }
}

/// A commit sha at the length `builds list` prints it.
fn short_ref(sha: &str) -> String {
    sha.chars().take(8).collect()
}

async fn remove(name: &str, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = fetch_mutation_document().await?;
    let entries = builds_array(&mut document)?;
    let before = entries.len();
    entries.retain(|entry| entry_name(entry) != Some(name));
    if entries.len() == before {
        return Err(CmdError::click(format!(
            "registry declares no build recipe {name:?}"
        )));
    }
    super::registry::push_document_if(&document, &generation).await?;
    if json {
        return print_json(&json!({ "name": name, "removed": true }));
    }
    println!("{name}: removed");
    Ok(())
}

async fn set_enabled(name: &str, enabled: bool, json: bool) -> Result<(), CmdError> {
    let (mut document, generation) = fetch_mutation_document().await?;
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
    let (mut document, generation) = fetch_mutation_document().await?;
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
                reason: None,
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
