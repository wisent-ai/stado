//! Parsing `.wisent-release.json` and holding it to the schema's rules.

use std::collections::BTreeSet;

use regex::Regex;
use serde_json::Value;

use crate::release_pipeline::contract::manifest::{
    ProductManifest, ReleasePipelineManifest, VersionSource,
};
use crate::release_pipeline::{PRODUCT_MANIFEST, RUNNER_PLATFORMS, SCHEMA_VERSION};

use super::predicates::{argv, env_name, identifier, platform_identifier, safe_relative, sha256};
use super::roles::{runtime_role, RuntimeRole};

pub fn parse_product_manifest(bytes: &[u8]) -> Result<ProductManifest, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{PRODUCT_MANIFEST}: invalid JSON: {error}"))?;
    let releases = value
        .get("releases")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{PRODUCT_MANIFEST}: releases must be true or false"))?;
    let manifest = if releases {
        ProductManifest::Release(
            serde_json::from_value(value)
                .map_err(|error| format!("{PRODUCT_MANIFEST}: {error}"))?,
        )
    } else {
        ProductManifest::NonRelease(
            serde_json::from_value(value)
                .map_err(|error| format!("{PRODUCT_MANIFEST}: {error}"))?,
        )
    };
    validate_product_manifest(&manifest)?;
    Ok(manifest)
}

pub fn validate_product_manifest(manifest: &ProductManifest) -> Result<(), String> {
    match manifest {
        ProductManifest::NonRelease(value) => {
            if value.schema_version != SCHEMA_VERSION || value.releases {
                return Err(
                    "non-release manifest must declare schema_version 1 and releases false".into(),
                );
            }
            if !identifier(&value.product)
                || value.reason.trim().is_empty()
                || value.reason.chars().any(char::is_control)
            {
                return Err("non-release manifest product or reason is invalid".into());
            }
            Ok(())
        }
        ProductManifest::Release(value) => validate_release_manifest(value),
    }
}

pub fn validate_release_manifest(manifest: &ReleasePipelineManifest) -> Result<(), String> {
    if manifest.schema_version != SCHEMA_VERSION || !manifest.releases {
        return Err("release manifest must declare schema_version 1 and releases true".into());
    }
    if !identifier(&manifest.product) {
        return Err("release manifest product is not a canonical identifier".into());
    }
    if !safe_relative(manifest.version_source.path()) {
        return Err("release manifest version_source path must be repository-relative".into());
    }
    match &manifest.version_source {
        VersionSource::Json { pointer, .. } if !pointer.starts_with('/') => {
            return Err("JSON version_source pointer must be an absolute JSON pointer".into())
        }
        VersionSource::Regex { pattern, .. } => {
            let expression = Regex::new(pattern)
                .map_err(|error| format!("version_source regex is invalid: {error}"))?;
            if expression
                .capture_names()
                .filter(|name| *name == Some("version"))
                .count()
                != 1
            {
                return Err(
                    "regex version_source must contain exactly one capture named version".into(),
                );
            }
        }
        _ => {}
    }
    if manifest.platforms.is_empty() {
        return Err("release manifest platforms must not be empty".into());
    }
    // How many platforms actually ship the declared runtime. A product may
    // now have platforms that do not — a web site beside a binary — and the
    // count is what keeps a runtime nothing stages from passing.
    let mut runtime_platforms = 0_usize;
    for (platform, recipe) in &manifest.platforms {
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
        if !recipe.extra.is_empty() {
            let mut unknown: Vec<&str> = recipe.extra.keys().map(String::as_str).collect();
            unknown.sort_unstable();
            return Err(format!(
                "{platform}: unknown recipe keys for this Stado: {}",
                unknown.join(", ")
            ));
        }
        let mut gates = BTreeSet::new();
        for gate in &recipe.quality {
            if !identifier(&gate.name) || !gates.insert(gate.name.as_str()) || !argv(&gate.argv) {
                return Err(format!(
                    "{platform}: quality gates require unique names and non-empty argv"
                ));
            }
        }
        for (name, reference) in &recipe.secret_env {
            let Some((item, field)) = reference.split_once('#') else {
                return Err(format!(
                    "{platform}: secret_env must use item#field references"
                ));
            };
            if !env_name(name) || !identifier(item) || !identifier(field) {
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
        match runtime_role(&destinations, manifest.runtime.as_ref()) {
            // Both destinations staged: the platform ships the runtime, and
            // is held to exactly what it was held to before.
            RuntimeRole::Runtime => runtime_platforms += 1,
            // Neither: the platform ships something else. `jeden`'s web site,
            // built by `stado web build`, stages a site tarball and no binary
            // at all, and holding it to a contract about a binary refused a
            // manifest that was correct.
            RuntimeRole::NotRuntime => {}
            // Exactly one of the two: the half-staged case this check exists
            // for. A platform that stages the binary and not its launcher
            // rolls out something the host cannot start.
            RuntimeRole::HalfStaged => {
                return Err(format!(
                    "{platform}: runtime binary and launcher must be staged destinations"
                ));
            }
        }
    }
    // A `runtime` no platform stages is a declaration nothing checks against
    // the world: the coordinate would publish with a binary path that exists
    // in no artifact, and the first rollout would find out. `jeden`'s
    // manifest is exactly that today — `runtime.binary` is `jeden` while both
    // platforms stage `bin/jeden` — and it must stay refused rather than
    // become valid because the per-platform rule stopped looking.
    if manifest.runtime.is_some() && runtime_platforms == 0 {
        return Err(
            "runtime is declared but no platform stages its binary and launcher: either a platform is missing them from its stage map, or the product declares no runtime".into(),
        );
    }
    if manifest.promotion.reconcile != manifest.runtime.is_some() {
        return Err("runtime must be declared exactly when promotion.reconcile is true".into());
    }
    if let Some(runtime) = &manifest.runtime {
        if !safe_relative(&runtime.binary)
            || !safe_relative(&runtime.launcher)
            || runtime.config_schema == 0
            || runtime.state_schema == 0
            || !identifier(&runtime.minimum_stado_version)
        {
            return Err("release runtime contract is invalid".into());
        }
        let mut rollback = BTreeSet::new();
        if runtime
            .rollback_compatible_with
            .iter()
            .any(|version| !identifier(version) || !rollback.insert(version))
        {
            return Err("rollback_compatible_with contains an invalid or duplicate version".into());
        }
    }
    let channels: BTreeSet<_> = manifest.promotion.channels.iter().copied().collect();
    let mut mounts: Vec<&str> = Vec::new();
    let mut environment_names = BTreeSet::new();
    for (name, input) in &manifest.inputs {
        let env_name = name
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    byte.to_ascii_uppercase() as char
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let object = crate::object_store::ObjectRef::parse(&input.uri)
            .map_err(|error| format!("release input {name:?} URI is invalid: {error}"))?;
        let immutable_source = object.namespace() == "sources"
            && object.key().split('/').any(|part| part == input.sha256);
        let immutable_release =
            object.namespace() == "releases" && object.key().split('/').count() >= 4;
        if !identifier(name)
            || !environment_names.insert(env_name)
            || !sha256(&input.sha256)
            || !safe_relative(&input.mount)
            || (!immutable_source && !immutable_release)
            || mounts.iter().any(|existing| {
                input.mount == *existing
                    || input.mount.starts_with(&format!("{existing}/"))
                    || existing.starts_with(&format!("{}/", input.mount))
            })
        {
            return Err(format!(
                "release input {name:?} must use an immutable Stado URI, exact digest, and unique non-overlapping mount"
            ));
        }
        mounts.push(&input.mount);
    }
    if channels.len() != manifest.promotion.channels.len() || channels.is_empty() {
        return Err("promotion must contain unique channels".into());
    }
    let mut deliveries = BTreeSet::new();
    for delivery in &manifest.deliveries {
        if !identifier(&delivery.name)
            || !deliveries.insert(delivery.name.as_str())
            || !manifest.platforms.contains_key(&delivery.platform)
            || !argv(&delivery.argv)
        {
            return Err(
                "deliveries require unique names, declared platforms, and non-empty argv".into(),
            );
        }
        let mut secret_names = BTreeSet::new();
        for (name, reference) in &delivery.secret_env {
            let Some((item, field)) = reference.split_once('#') else {
                return Err(format!(
                    "delivery {} secret_env must use item#field references",
                    delivery.name
                ));
            };
            if !env_name(name)
                || !secret_names.insert(name)
                || !identifier(item)
                || !identifier(field)
            {
                return Err(format!("delivery {} secret_env is invalid", delivery.name));
            }
        }
    }
    Ok(())
}
