//! `stado release worker` — the builder-side half of one platform build job.

pub(in crate::cli::release_submit) mod environment;
mod package;
pub(in crate::cli::release_submit) mod steps;

use environment::build_environment;

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::release_submit::builds::worker::package::{
    disk_sentence, measure_scratch, package, receipt, write_receipt, write_scratch,
};
use crate::cli::release_submit::builds::worker::steps::{
    cargo_source, ensure_rust_components, execute, require_free_space,
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
    // A Cargo source compiles through the declared compiler cache in every
    // step — the gates, the build and any script they call — so the crates a
    // builder compiled for an earlier release, of this product or another,
    // are restored instead of compiled again. A builder that lacks the
    // declared Kache installs it here, before the first step spends time.
    if cargo_source(&source) {
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
    }

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
            return Err(CmdError::refused("release quality gate failed"));
        }
    }
    let mut build = execute("build", &recipe.build.argv, &source, &environment)?;
    // The cause a step reported before it could run, kept for the refusal:
    // a step that never started has no exit status to name.
    let mut unstarted: Option<String> = None;
    if build.status == StepStatus::Passed && request.platform.starts_with("darwin-") {
        // The signer is this worker's own Stado — never whatever signing
        // program the builder's PATH happens to carry, which can be nothing,
        // ending a build before its first signature with "cannot run
        // wisent-products".
        //
        // The identity is the fleet's too: the Apple certificate and key
        // Skarbiec holds, handed to the signer's own temporary keychain
        // through its environment, the way runner reconciliation already
        // hands them over. Signing with whatever identity the builder's login
        // keychain holds would let a build placed on the operator's laptop
        // sign while the same build placed on another host dies at `no Apple
        // signing identity is available`, spending a release coordinate on a
        // placement decision.
        let mut argv = crate::deploy::native_signing::local_signer()?;
        argv.extend([
            "signing".into(),
            "stage".into(),
            "--manifest".into(),
            source.join(".wisent-release.json").display().to_string(),
            "--output".into(),
            output.display().to_string(),
            "--platform".into(),
            request.platform.clone(),
            "--json".into(),
        ]);
        let signing = match crate::deploy::native_signing::signing_environment().await {
            Ok(identity) => {
                let mut signing_environment = environment.clone();
                signing_environment.extend(identity);
                execute("macos-code-signing", &argv, &source, &signing_environment)?
            }
            Err(error) => {
                println!("[release-worker] step macos-code-signing: {error}");
                unstarted = Some(error.to_string());
                StepReceipt {
                    name: "macos-code-signing".into(),
                    argv,
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
    let mut test_environment = environment.clone();
    if !recipe.test_products.is_empty() {
        let home = std::env::var("HOME").map_err(|error| {
            CmdError::click(format!("HOME is not set: {error}"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        let inherited = std::env::var("PATH").map_err(|error| {
            CmdError::click(format!(
                "PATH is not set, so the post-build tests would find no program but the installed products: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        test_environment.insert("PATH".into(), format!("{home}/.stado/bin:{inherited}"));
    }
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
