use std::path::Path;

use crate::cli::CmdError;

use crate::cli::host::files::retire::launchd::retire_system_launchd_file;
use crate::cli::host::files::retire::local::retire_file_local_document;
use crate::cli::host::files::retire::{
    retire_file_binding, RetireFileBinding, RetireFileOutcome, RetireFileRequest,
};

/// Resolve TARGET and perform one checked archive locally or over its declared
/// host channel. User binaries invoke the same installed Rust primitive on a
/// remote target; system launchd declarations use this build's fixed
/// digest-bound privileged primitive with the target's approved sudo grant.
async fn retire_file_document(
    target: &str,
    request: &RetireFileRequest<'_>,
    binding: Option<&RetireFileBinding>,
) -> Result<RetireFileOutcome, CmdError> {
    let RetireFileRequest {
        path,
        product,
        dry_run,
        ..
    } = *request;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut outcome = if Path::new(path).parent() == Some(Path::new("/Library/LaunchDaemons")) {
        retire_system_launchd_file(&resolved, request, binding).await?
    } else if crate::deploy::host_channel::target_is_this_host(&resolved) {
        retire_file_local_document(request, binding)?
    } else {
        let runner = crate::deploy::production_runner();
        let home = crate::deploy::host_channel::remote_home(&resolved, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let binary = format!("{home}/.stado/bin/stado");
        let expected_size = binding.map(|binding| binding.expected_size.to_string());
        let mut words = vec![
            binary.as_str(),
            "space",
            "file",
            "retire-local",
            path,
            "--product",
            product,
            "--json",
        ];
        if dry_run {
            words.push("--dry-run");
        }
        if let Some(binding) = binding {
            words.extend([
                "--transaction",
                binding.transaction.as_str(),
                "--expected-sha256",
                binding.expected_sha256.as_str(),
                "--expected-size",
                expected_size
                    .as_deref()
                    .expect("binding supplies expected size"),
                "--expected-mode",
                binding.expected_mode.as_str(),
            ]);
        }
        let output = crate::deploy::host_channel::run_program(&resolved, &words, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{}: installed Stado space file retire primitive failed: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(
                    &output,
                    "remote command returned no detail"
                )
            )));
        }
        serde_json::from_str::<RetireFileOutcome>(output.stdout.trim()).map_err(|error| {
            CmdError::click(format!(
                "{}: installed Stado returned an invalid retirement report: {error}",
                resolved.name
            ))
        })?
    };
    outcome.target = resolved.name.clone();
    if outcome.succeeded() {
        Ok(outcome)
    } else {
        Err(CmdError::click(outcome.failure_sentence()))
    }
}

/// Resolve and perform one declaration-bound retirement for the space capability.
pub async fn retire_file_outcome(
    target: &str,
    request: &RetireFileRequest<'_>,
) -> Result<RetireFileOutcome, CmdError> {
    let binding = retire_file_binding(request)?;
    retire_file_document(target, request, binding.as_ref()).await
}
