//! The environment one build job runs with, assembled from the immutable
//! request rather than inherited from whatever launched the worker.

use std::collections::BTreeMap;
use std::path::Path;

use crate::release_pipeline::WorkerRequest;

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
