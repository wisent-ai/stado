//! `stado release worker` — the builder-side half of one platform build job.

pub(in crate::cli::release_submit) mod environment;
mod package;
mod signing;
pub(in crate::cli::release_submit) mod steps;

use environment::build_environment;

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::release_submit::builds::worker::package::{
    disk_sentence, measure_scratch, package, receipt, write_receipt, write_scratch,
};
use crate::cli::release_submit::builds::worker::steps::{
    ensure_rust_components, execute, require_free_space, require_node_toolchain,
};
use crate::cli::release_submit::ReleaseWorkerArgs;
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{
    self, ArtifactReceipt, ReceiptInput, StepReceipt, StepStatus, WorkerRequest,
};

pub async fn worker(args: &ReleaseWorkerArgs) -> Result<(), CmdError> {
    // Name the file. Each of these was a bare `?`, so a missing one surfaced as
    // `Error: No such file or directory (os error 2)` with no path at all, on a
    // builder whose log ended with a successful compile -- and the operator's
    // only recourse was to guess which of four paths it meant.
    let read_named = |path: &dyn AsRef<Path>, what: &str| -> Result<Vec<u8>, CmdError> {
        let path = path.as_ref();
        std::fs::read(path).map_err(|error| {
            CmdError::click(format!("cannot read {what} {}: {error}", path.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })
    };
    let request_bytes = read_named(&args.request, "the worker request")?;
    let request: WorkerRequest = serde_json::from_slice(&request_bytes)?;
    let manifest_bytes = read_named(&request.manifest_path, "the release manifest")?;
    if request.schema_version != 1
        || release_control::sha256_bytes(&manifest_bytes) != request.manifest_sha256
    {
        return Err(CmdError::click("worker manifest identity mismatch")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // The worker reads the product and its platform's recipe, nothing
    // else: it runs the Stado its host already has, and a delivery or
    // promotion section that changed shape in this commit is the control
    // host's to read.
    let manifest = release_pipeline::parse_worker_manifest(&manifest_bytes, &request.platform)
        .map_err(CmdError::declaration)?;
    if manifest.product != request.product {
        return Err(CmdError::click("worker request disagrees with manifest")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let source_bytes = read_named(&request.source_archive, "the source archive")?;
    if release_control::sha256_bytes(&source_bytes) != request.source_sha256 {
        return Err(CmdError::click("worker source digest mismatch")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let queue_work_dir = std::env::current_dir()?;
    let temp = tempfile::Builder::new()
        .prefix(".stado-release-worker-")
        .tempdir_in(&queue_work_dir)?;
    let source = temp.path().join("source");
    release_control::safe_extract_source_archive(&source_bytes, &source)
        .map_err(CmdError::refused)?;
    let inputs_root = temp.path().join("inputs");
    std::fs::create_dir_all(&inputs_root)?;
    let mut receipt_inputs = BTreeMap::new();
    for (name, input) in &request.inputs {
        let bytes = read_named(&input.archive_path, &format!("input {name}"))?;
        if release_control::sha256_bytes(&bytes) != input.sha256 {
            return Err(CmdError::click(format!("input {name} digest mismatch"))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        if input.extract {
            release_control::safe_extract_archive(&bytes, &inputs_root.join(&input.mount))
                .map_err(CmdError::refused)?;
        } else {
            let destination = inputs_root.join(&input.mount);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(destination, &bytes)?;
        }
        receipt_inputs.insert(
            name.clone(),
            ReceiptInput {
                uri: input.uri.clone(),
                sha256: input.sha256.clone(),
                mount: input.mount.clone(),
                extract: input.extract,
            },
        );
    }
    let output = source.join(".wisent-output");
    std::fs::create_dir_all(&output)?;
    let evidence_root = queue_work_dir.join("output/qualification");
    std::fs::create_dir_all(&evidence_root)?;
    let mut environment =
        build_environment(&request, &source, &output, &inputs_root, &evidence_root);
    environment::add_compiler_cache(&source, &mut environment)?;
    environment::ensure_node(&source)?;

    // Give the pinned toolchain the components its own gates are about to
    // demand. rustup installs a pinned toolchain on first use WITHOUT optional
    // components, so the first release job on a fresh agent died with
    // "'cargo-fmt' is not installed for the toolchain" — a fact about host
    // provisioning that no release should trip over and no operator should
    // fix by hand, host by host. Adding a component is idempotent and rustup
    // resolves the pin from the working directory, so a provisioned host pays
    // a no-op and a fresh one provisions itself, exactly the way the
    // toolchain itself already arrives.
    ensure_rust_components(&manifest.platforms[&request.platform], &source)?;
    let recipe = &manifest.platforms[&request.platform];
    // Before the first crate, not after the last one: a build with no room
    // fails having spent every minute it was going to spend.
    require_free_space(recipe, &source)?;
    // A Node source on a builder with no Node would run its gates and stop
    // at the first `npx`; refuse it here, naming the missing program.
    require_node_toolchain(&source, &environment, &request.platform)?;
    let job_id = std::env::var("WC_JOB_ID").unwrap_or_default();
    let mut quality = Vec::new();
    for gate in &recipe.quality {
        let step = execute(&gate.name, &gate.argv, &source, &environment)?;
        let passed = step.status == StepStatus::Passed;
        quality.push(step);
        if !passed {
            let receipt = receipt(
                &request,
                &job_id,
                receipt_inputs,
                quality,
                StepReceipt {
                    name: "build".into(),
                    argv: recipe.build.argv.clone(),
                    status: StepStatus::Failed,
                    exit_code: None,
                },
                StepStatus::Failed,
                None,
                Some(format!("quality gate {} failed", gate.name)),
            );
            write_receipt(&receipt)?;
            return Err(CmdError::refused("release quality gate failed"));
        }
    }
    let mut build = execute("build", &recipe.build.argv, &source, &environment)?;
    // The cause a step reported before it could run, kept for the refusal:
    // a step that never started has no exit status to name.
    let mut unstarted: Option<String> = None;
    if build.status == StepStatus::Passed && request.platform.starts_with("darwin-") {
        let (signing, cause) =
            signing::macos_signing(&request, &source, &output, &environment).await?;
        unstarted = cause;
        if signing.status != StepStatus::Passed {
            build = signing;
        } else {
            quality.push(signing);
        }
    }
    // Measured before anything is removed and before anything more is
    // written: what this build put on disk and what the volume had left. The
    // record goes beside the receipt, and the next placement of this product
    // and platform reads it.
    let scratch = measure_scratch(temp.path(), &request, &job_id, build.status.clone())?;
    if build.status != StepStatus::Passed {
        let disk = disk_sentence(&scratch);
        let cause = match (&unstarted, build.exit_code) {
            (Some(cause), _) => format!("step {} could not run: {cause}", build.name),
            (None, Some(code)) => format!("step {} exited {code}", build.name),
            (None, None) => format!("step {} ended without an exit status", build.name),
        };
        println!("[release-worker] build: {cause}; {disk}");
        // Give the record room: a build that filled the volume has left none
        // for the account of its own failure until its tree is gone.
        temp.close().map_err(|error| {
            CmdError::click(format!("cannot remove the build tree: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        write_scratch(&scratch)?;
        let receipt = receipt(
            &request,
            &job_id,
            receipt_inputs,
            quality,
            build,
            StepStatus::Failed,
            None,
            Some(format!("{cause}; {disk}")),
        );
        write_receipt(&receipt)?;
        return Err(CmdError::refused(format!(
            "release build failed: {cause}; {disk}"
        )));
    }
    write_scratch(&scratch)?;
    // The product CLIs the tests drive, installed through Stado on this
    // builder first, and found by the tests on PATH where Stado installs them.
    let test_environment = environment::test_environment(&environment, recipe)?;
    for needed in &recipe.test_products {
        let argv = vec![
            "stado".to_string(),
            "product".into(),
            "install".into(),
            needed.product.clone(),
            "--surface".into(),
            needed.surface.clone(),
        ];
        let name = format!("test-product:{}:{}", needed.product, needed.surface);
        let step = execute(&name, &argv, &source, &environment)?;
        let passed = step.status == StepStatus::Passed;
        quality.push(step);
        if !passed {
            let cause = format!(
                "{} {} could not be installed for the post-build tests",
                needed.product, needed.surface
            );
            let receipt = receipt(
                &request,
                &job_id,
                receipt_inputs,
                quality,
                build,
                StepStatus::Failed,
                None,
                Some(cause.clone()),
            );
            write_receipt(&receipt)?;
            return Err(CmdError::refused(cause));
        }
    }
    for test in &recipe.tests {
        let step = execute(
            &format!("test:{}", test.name),
            &test.argv,
            &source,
            &test_environment,
        )?;
        let passed = step.status == StepStatus::Passed;
        quality.push(step);
        if !passed {
            let receipt = receipt(
                &request,
                &job_id,
                receipt_inputs,
                quality,
                build,
                StepStatus::Failed,
                None,
                Some(format!("post-build test {} failed", test.name)),
            );
            write_receipt(&receipt)?;
            return Err(CmdError::refused(format!(
                "post-build test {} failed",
                test.name
            )));
        }
    }
    // The stage map is relative to `WISENT_OUTPUT_DIR`, which is what every
    // recipe's build script writes into -- brama and skarbiec both install to
    // `$WISENT_OUTPUT_DIR/stage/...`. Packaging resolved it against the source
    // tree instead, so the first entry always reported
    // `staged path .../source/stage/LICENSE ... is not there` and no release
    // carrying a stage mapping could ever be packaged through this path.
    let bytes = package(&output, &source, &recipe.stage)?;
    std::fs::create_dir_all("output")?;
    std::fs::write("output/release.tar.gz", &bytes)?;
    let receipt = receipt(
        &request,
        &job_id,
        receipt_inputs,
        quality,
        build,
        StepStatus::Passed,
        Some(ArtifactReceipt {
            sha256: release_control::sha256_bytes(&bytes),
            bytes: bytes.len() as u64,
            path: "release.tar.gz".into(),
        }),
        None,
    );
    write_receipt(&receipt)
}
