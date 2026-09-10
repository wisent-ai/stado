//! `stado release worker` — the builder-side half of one platform build job.

mod environment;
mod package;
pub(in crate::cli::release_submit) mod steps;

use environment::build_environment;

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::release_submit::builds::worker::package::{
    disk_sentence, measure_scratch, package, receipt, write_receipt, write_scratch,
};
use crate::cli::release_submit::builds::worker::steps::{
    ensure_rust_components, execute, require_free_space,
};
use crate::cli::release_submit::ReleaseWorkerArgs;
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{
    self, ArtifactReceipt, ProductManifest, ReceiptInput, StepReceipt, StepStatus, WorkerRequest,
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
        })
    };
    let request_bytes = read_named(&args.request, "the worker request")?;
    let request: WorkerRequest = serde_json::from_slice(&request_bytes)?;
    let manifest_bytes = read_named(&request.manifest_path, "the release manifest")?;
    if request.schema_version != 1
        || release_control::sha256_bytes(&manifest_bytes) != request.manifest_sha256
    {
        return Err(CmdError::click("worker manifest identity mismatch"));
    }
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&manifest_bytes).map_err(CmdError::click)?
    else {
        return Err(CmdError::click("worker manifest declares releases:false"));
    };
    if manifest.product != request.product || !manifest.platforms.contains_key(&request.platform) {
        return Err(CmdError::click("worker request disagrees with manifest"));
    }
    let source_bytes = read_named(&request.source_archive, "the source archive")?;
    if release_control::sha256_bytes(&source_bytes) != request.source_sha256 {
        return Err(CmdError::click("worker source digest mismatch"));
    }
    let queue_work_dir = std::env::current_dir()?;
    let temp = tempfile::Builder::new()
        .prefix(".stado-release-worker-")
        .tempdir_in(&queue_work_dir)?;
    let source = temp.path().join("source");
    release_control::safe_extract_archive(&source_bytes, &source).map_err(CmdError::click)?;
    let inputs_root = temp.path().join("inputs");
    std::fs::create_dir_all(&inputs_root)?;
    let mut receipt_inputs = BTreeMap::new();
    for (name, input) in &request.inputs {
        let bytes = read_named(&input.archive_path, &format!("input {name}"))?;
        if release_control::sha256_bytes(&bytes) != input.sha256 {
            return Err(CmdError::click(format!("input {name} digest mismatch")));
        }
        if input.extract {
            release_control::safe_extract_archive(&bytes, &inputs_root.join(&input.mount))
                .map_err(CmdError::click)?;
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
    let environment = build_environment(&request, &source, &output, &inputs_root);

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
            return Err(CmdError::click("release quality gate failed"));
        }
    }
    let mut build = execute("build", &recipe.build.argv, &source, &environment)?;
    if build.status == StepStatus::Passed && request.platform.starts_with("darwin-") {
        // The signer is the fleet's pinned revision, installed into this
        // builder's Stado-owned cache when absent - never whatever
        // `wisent-products` the builder's PATH happens to carry, which on
        // 2026-09-10 was nothing, and which ended weles-worker 0.6.6 before
        // its first signature with "cannot run wisent-products".
        let signing = match crate::deploy::native_signing::bootstrap_local_signer(
            &crate::deploy::production_runner(),
        )
        .await
        {
            Ok(signer) => execute(
                "macos-code-signing",
                &[
                    signer,
                    "signing".into(),
                    "stage".into(),
                    "--manifest".into(),
                    source.join(".wisent-release.json").display().to_string(),
                    "--output".into(),
                    output.display().to_string(),
                    "--platform".into(),
                    request.platform.clone(),
                    "--json".into(),
                ],
                &source,
                &environment,
            )?,
            Err(error) => {
                println!("[release-worker] step macos-code-signing: {error}");
                StepReceipt {
                    name: "macos-code-signing".into(),
                    argv: vec![crate::deploy::native_signing::SIGNER_SOURCE_SHA256.into()],
                    status: StepStatus::Failed,
                    exit_code: None,
                }
            }
        };
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
        println!("[release-worker] build: {disk}");
        // Give the record room: a build that filled the volume has left none
        // for the account of its own failure until its tree is gone.
        temp.close()
            .map_err(|error| CmdError::click(format!("cannot remove the build tree: {error}")))?;
        write_scratch(&scratch)?;
        let receipt = receipt(
            &request,
            &job_id,
            receipt_inputs,
            quality,
            build,
            StepStatus::Failed,
            None,
            Some(format!("build command failed; {disk}")),
        );
        write_receipt(&receipt)?;
        return Err(CmdError::click(format!(
            "release build command failed; {disk}"
        )));
    }
    write_scratch(&scratch)?;
    // The stage map is relative to `WISENT_OUTPUT_DIR`, which is what every
    // recipe's build script writes into -- brama and skarbiec both install to
    // `$WISENT_OUTPUT_DIR/stage/...`. Packaging resolved it against the source
    // tree instead, so the first entry always reported
    // `staged path .../source/stage/LICENSE ... is not there` and no release
    // carrying a stage mapping could ever be packaged through this path.
    let bytes = package(&output, &recipe.stage)?;
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
