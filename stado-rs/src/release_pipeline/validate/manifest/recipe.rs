//! One platform's recipe held to the schema's rules.

use std::collections::BTreeSet;

use crate::release_pipeline::contract::recipe::PlatformRecipe;
use crate::release_pipeline::RUNNER_PLATFORMS;

use super::super::predicates::{argv, env_name, identifier, platform_identifier, safe_relative};

/// Hold one platform's recipe to the schema's rules and return its staged
/// destinations. Shared by the full manifest validation and the worker's
/// view of the one platform it builds.
pub(super) fn validate_recipe<'a>(
    platform: &str,
    recipe: &'a PlatformRecipe,
) -> Result<BTreeSet<&'a str>, String> {
    if !platform_identifier(platform)
        || !RUNNER_PLATFORMS.contains(&recipe.runner_platform.as_str())
    {
        return Err(format!(
            "{platform:?}: invalid output platform or runner_platform"
        ));
    }
    // Forward compatibility does not mean silence: the contract keeps a
    // key it does not know so an older worker can still build, and the
    // binary the operator submits with names it here. A typo is refused
    // before a job is queued; a field from a newer Stado is refused with
    // the same sentence, which is the true answer for this binary.
    //
    // The sentence now also says what the refusal costs and what ends it.
    // A key entering this repository's own recipe hours after the newest
    // published release is cut has every builder in the fleet refuse it,
    // most of the day's build budget spent discovering that, and no
    // stado release buildable at all — including the one carrying the
    // reader for the key.
    if !recipe.extra.is_empty() {
        let mut unknown: Vec<&str> = recipe.extra.keys().map(String::as_str).collect();
        unknown.sort_unstable();
        return Err(format!(
            "{platform}: unknown recipe keys for this Stado: {}. This binary is the one that \
             builds: a key a release adds cannot gate the release that adds it. Express the \
             gate with keys every builder already reads, or deliver a Stado that reads this \
             one to the builders first (`stado release version converge --host <builder>`).",
            unknown.join(", ")
        ));
    }
    let mut gates = BTreeSet::new();
    for gate in recipe.quality.iter().chain(recipe.tests.iter()) {
        if !gates.insert(gate.name.as_str()) {
            return Err(format!("{platform}: duplicate step name {:?}", gate.name));
        }
        if !identifier(&gate.name) || !argv(&gate.argv) {
            return Err(format!(
                "{platform}: quality and test steps require valid names and non-empty argv"
            ));
        }
    }
    for (name, reference) in &recipe.secret_env {
        let Some((role, field)) = reference.split_once('#') else {
            return Err(format!(
                "{platform}: secret_env must use role#field references"
            ));
        };
        if !env_name(name) || !identifier(role) || !identifier(field) {
            return Err(format!("{platform}: secret_env is invalid"));
        }
    }
    for (name, value) in &recipe.env {
        // A value is a literal the build reads, so the only thing that
        // cannot be one is a control character: it would arrive in the
        // child's environment as something no reader of this file typed.
        if !env_name(name) || value.is_empty() || value.chars().any(char::is_control) {
            return Err(format!("{platform}: env is invalid"));
        }
        // One variable declared in both places has two answers and the
        // build would take whichever the exporter applied last. A public
        // constant and a credential are also different review paths, so
        // the collision is a mistake about which one this value is.
        if recipe.secret_env.contains_key(name) {
            return Err(format!(
                "{platform}: {name} is declared in both env and secret_env"
            ));
        }
    }
    if !argv(&recipe.build.argv) || recipe.stage.is_empty() {
        return Err(format!(
            "{platform}: build argv and stage mapping must not be empty"
        ));
    }
    let mut destinations = BTreeSet::new();
    for (source, destination) in &recipe.stage {
        if !safe_relative(source)
            || !safe_relative(destination)
            || !destinations.insert(destination.as_str())
        {
            return Err(format!("{platform}: stage paths are unsafe or duplicate"));
        }
    }
    Ok(destinations)
}
