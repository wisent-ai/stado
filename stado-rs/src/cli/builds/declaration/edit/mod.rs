//! `stado builds edit`: replace the fields the operator named, and say what
//! that did to the recorded state.

mod fields;

pub(in crate::cli::builds) use fields::RecipeEdit;

use serde_json::{json, Map, Value};

use crate::cli::builds::declaration::edit::fields::{
    replace_field, runs_phrase, short_ref, string_list,
};
use crate::cli::builds::declaration::AUTO_DECLARE_ON;
use crate::cli::builds::{
    builds_array, fetch_mutation_document, find_entry, normalized_recipe_json, print_json,
};
use crate::cli::CmdError;

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
pub(in crate::cli::builds) async fn edit(
    name: &str,
    fields: RecipeEdit,
    json: bool,
) -> Result<(), CmdError> {
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
    crate::cli::registry::push_document_if(&document, &generation).await?;
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
        // The printed command has to be one that runs. `builds run` requires
        // --run-id, so naming the recipe alone produced a clap usage error for
        // anyone who copied this line; the token is the caller's to choose and
        // to keep, because reusing it is what recovers the same durable run
        // instead of starting a second one.
        (true, Some(state)) => println!(
            "{name}: same source — kept {state}; the next poll builds only when the head moves, \
             so build it now with `stado builds run {name} --run-id <token>`, where <token> is \
             yours to pick and to retain — reuse it to recover this run rather than start \
             another"
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
