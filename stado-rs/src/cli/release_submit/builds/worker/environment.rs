//! The environment one build job runs with, assembled from the immutable
//! request rather than inherited from whatever launched the worker.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::CmdError;
use crate::release_pipeline::WorkerRequest;

use super::steps::cargo_source;

/// `WISENT_SOURCE_COMMIT` and `WISENT_SOURCE_SHA256` are the snapshot's own
/// identity, and a build that needs them has nowhere else to get them: the
/// worker unpacks a `git archive`, so there is no repository to ask. Both
/// names are set from the immutable request so a build script cannot inherit
/// an unrelated parent `STADO_SOURCE_REVISION`; Stado's build script requires
/// them to agree exactly. The source commit was already validated before the
/// archive existed.
pub(super) fn build_environment(
    request: &WorkerRequest,
    source: &Path,
    output: &Path,
    inputs_root: &Path,
    evidence_root: &Path,
) -> BTreeMap<String, String> {
    // `WISENT_SOURCE_COMMIT` and `WISENT_SOURCE_SHA256` are the snapshot's own
    // identity, and a build that needs them has nowhere else to get them: the
    // worker unpacks a `git archive`, so there is no repository to ask. Both
    // names are set from the immutable request so a build script cannot inherit
    // an unrelated parent `STADO_SOURCE_REVISION`; Stado's build script requires
    // them to agree exactly. The source commit was already validated before the
    // archive existed.
    let mut environment = BTreeMap::from([
        ("WISENT_SOURCE_DIR".into(), source.display().to_string()),
        ("WISENT_OUTPUT_DIR".into(), output.display().to_string()),
        (
            "WISENT_TEST_EVIDENCE_DIR".into(),
            evidence_root.display().to_string(),
        ),
        (
            "WISENT_INPUTS_DIR".into(),
            inputs_root.display().to_string(),
        ),
        ("WISENT_PRODUCT".into(), request.product.clone()),
        ("WISENT_VERSION".into(), request.version.clone()),
        ("WISENT_PLATFORM".into(), request.platform.clone()),
        ("WISENT_SOURCE_COMMIT".into(), request.source_commit.clone()),
        (
            "STADO_SOURCE_REVISION".into(),
            request.source_commit.clone(),
        ),
        ("WISENT_SOURCE_SHA256".into(), request.source_sha256.clone()),
    ]);
    // The scratch tree is thrown away with the job, and a Cargo product
    // compiled from scratch there three times per platform per release -
    // clippy, the documentation test, the release build - is what makes a
    // release take hours. The compiled dependencies live
    // on the builder instead, per product and platform, so the next release
    // of the same product recompiles only what its commit changed. Cargo
    // locks the directory itself, so two jobs of one product on one host
    // take turns rather than corrupt it. A recipe step that names its own
    // `--target-dir` keeps it; the stage map relies on that. The cache sits
    // under the host's declared work root when it has one — the agent hands
    // the declaration to every job — and under `~/.stado` otherwise.
    if let Some(home) = std::env::var_os("HOME") {
        environment.insert(
            "CARGO_TARGET_DIR".into(),
            crate::providers::local::work_base::build_cache_root(Path::new(&home))
                .join(&request.product)
                .join(&request.platform)
                .join("cargo-target")
                .display()
                .to_string(),
        );
    }
    // Build steps sign and build through `stado product`. The first `stado`
    // on their PATH is this worker's own executable, so a step can never
    // reach a different installed Stado than the one running the job. The
    // directories a bare step program is looked up in come next, so a step
    // that is a script finds the same `cargo` the worker would have run.
    let own = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    if let Some(path) = stado_product::common::step_search_path(own, &inherited) {
        environment.insert("PATH".into(), path.to_string_lossy().into_owned());
    }
    for (name, input) in &request.inputs {
        environment.insert(
            input_variable(name),
            inputs_root.join(&input.mount).display().to_string(),
        );
    }
    environment
}

/// The variable a release step reads one declared input from:
/// `WISENT_INPUT_<NAME>_DIR`, the name upper-cased with every other character
/// an underscore. One spelling for the worker that publishes it, the web
/// steps that read it and `stado quality check`, which stages inputs the same
/// way before it runs the gates.
pub(crate) fn input_variable(name: &str) -> String {
    let key = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() {
                b.to_ascii_uppercase() as char
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("WISENT_INPUT_{key}_DIR")
}

/// The Node.js a Node product's release steps run. A source whose root
/// carries `package.json` is built with `npm` and checked with `npx`, and a
/// Linux builder has no Node of its own: Weles' linux-amd64 release failed
/// its formatting gate with `npx: not found` before a line was built. The
/// worker installs the release Stado declares
/// ([`stado_product::node_runtime`]) before the first step, and the step
/// search path already puts `~/.local/bin`, where its programs are linked,
/// ahead of the inherited one. A builder that cannot install it refuses the
/// build naming the step that failed.
pub(super) fn ensure_node(source: &Path) -> Result<(), CmdError> {
    if !source.join("package.json").is_file() {
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| CmdError::refused("HOME is not set; Node.js is installed under it"))?;
    let report = stado_product::node_runtime::ensure(&home).map_err(|error| {
        CmdError::click(format!("{error:#}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    println!(
        "[release-worker] node runtime: {} {}",
        report["node"], report["version"]
    );
    Ok(())
}

/// A Cargo source compiles through the declared compiler cache in every
/// step — the gates, the build and any script they call — so the crates a
/// builder compiled for an earlier release, of this product or another, are
/// restored instead of compiled again. A builder that lacks the declared
/// Kache installs it here, before the first step spends time, and
/// `environment` names it as `RUSTC_WRAPPER`.
pub(super) fn add_compiler_cache(
    source: &Path,
    environment: &mut BTreeMap<String, String>,
) -> Result<(), CmdError> {
    if !cargo_source(source)? {
        return Ok(());
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            CmdError::refused("HOME is not set; the compiler cache is installed under it")
        })?;
    let wrapper = stado_product::compiler_cache::ensure(&home).map_err(|error| {
        CmdError::click(format!("{error:#}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    println!(
        "[release-worker] compiler cache: {} {}",
        wrapper.path.display(),
        wrapper.version
    );
    environment.insert("RUSTC_WRAPPER".into(), wrapper.path.display().to_string());
    Ok(())
}

/// The environment the post-build tests run with: the build's, with the
/// product CLIs `recipe` installs for the tests found on PATH where Stado
/// installs them, ahead of the inherited search path.
pub(super) fn test_environment(
    environment: &BTreeMap<String, String>,
    recipe: &crate::release_pipeline::PlatformRecipe,
) -> Result<BTreeMap<String, String>, CmdError> {
    let mut tests = environment.clone();
    if recipe.test_products.is_empty() {
        return Ok(tests);
    }
    let home = std::env::var("HOME").map_err(|error| {
        CmdError::click(format!("HOME is not set: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let inherited = std::env::var("PATH").map_err(|error| {
        CmdError::click(format!(
            "PATH is not set, so the post-build tests would find no program but the installed \
             products: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    tests.insert("PATH".into(), format!("{home}/.stado/bin:{inherited}"));
    Ok(tests)
}
