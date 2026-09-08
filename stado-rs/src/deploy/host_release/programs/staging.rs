use serde::{Deserialize, Serialize};

use super::super::deliver::step_failure;
use super::super::{is_sha256, marker, markers, plan, resolve_release_request};
use super::super::{ReleaseRequest, STAGE_TIMEOUT};
use super::program::REMOTE_RETAIN_READER_ARCHIVE_BODY;
use super::{bindings, probe_script, stage_script, FETCH_PRELUDE, SANITIZE_PRELUDE};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Ensure the canonical Stado archive is retained for private reader
/// convergence without activating or restarting the already-attested root.
///
/// Ordinary root staging moves its already-downloaded archive into this
/// namespace. A resumed partial state first reuses that exact file when its
/// digest still matches; only an absent or corrupt retained file is fetched,
/// and that fallback neither extracts the root member nor touches a unit.
pub async fn ensure_stado_reader_archive(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    self_store: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    let plan = plan(target, request, self_store)?;
    if plan.product.name != "stado" || plan.product.install.is_tree() {
        return Err(DeployError(
            "reader archive convergence is defined only for the Stado program product".to_string(),
        ));
    }
    let cached_script = format!(
        "{}set -euo pipefail\n\
         reader_archive=\"$HOME/.stado/releases/$binary/$version/$platform/$reader_archive_name\"\n\
         [ -f \"$reader_archive\" ] && [ ! -L \"$reader_archive\" ]\n\
         digest_line=$(/usr/bin/openssl dgst -sha256 -r \"$reader_archive\")\n\
         actual_sha256=${{digest_line%% *}}\n\
         [ \"$actual_sha256\" = \"$expected_sha256\" ]\n",
        bindings(&plan),
    );
    if host_channel::run_script(target, &cached_script, runner)
        .await?
        .ok()
    {
        return Ok(());
    }

    let script = format!(
        "{}{SANITIZE_PRELUDE}{FETCH_PRELUDE}{REMOTE_RETAIN_READER_ARCHIVE_BODY}",
        bindings(&plan),
    );
    let output =
        host_channel::run_script_with_timeout(target, &script, STAGE_TIMEOUT, runner).await?;
    let output_markers = markers(&output.stdout);
    if !output.ok() || marker(&output_markers, "step") != "retain_reader_archive" {
        return Err(DeployError(format!(
            "cannot retain the verified Stado reader archive: {}",
            step_failure(&output_markers, &output)
        )));
    }
    if marker(&output_markers, "sha256") != plan.sha256 {
        return Err(DeployError(
            "retained Stado reader archive did not report the canonical digest".to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedRelease {
    pub request: ReleaseRequest,
    pub self_store: bool,
    pub staged_sha256: String,
}

/// Fetch, verify and stage the currently declared runtime without changing the
/// active path or restarting a unit.
pub async fn stage_declared_release(
    target_name: &str,
    binary: &str,
    version: &str,
    runner: &Runner,
) -> Result<StagedRelease, DeployError> {
    let (target, request, self_store) =
        resolve_release_request(target_name, binary, version, false, true, runner).await?;
    let plan = plan(&target, &request, self_store)?;
    let probe = host_channel::run_script(&target, &probe_script(&plan), runner).await?;
    let probe_markers = markers(&probe.stdout);
    if !probe.ok()
        || marker(&probe_markers, "step") != "probe"
        || marker(&probe_markers, "sanitizer") != "ok"
        || marker(&probe_markers, "platform") != plan.platform
    {
        return Err(DeployError(step_failure(&probe_markers, &probe)));
    }
    let stage =
        host_channel::run_script_with_timeout(&target, &stage_script(&plan), STAGE_TIMEOUT, runner)
            .await?;
    let stage_markers = markers(&stage.stdout);
    if !stage.ok() || marker(&stage_markers, "step") != "stage" {
        return Err(DeployError(step_failure(&stage_markers, &stage)));
    }
    let staged_sha256 = marker(&stage_markers, "staged_sha256").to_string();
    if !is_sha256(&staged_sha256) {
        return Err(DeployError(
            "staged declared runtime did not report its extracted program digest".to_string(),
        ));
    }
    Ok(StagedRelease {
        request,
        self_store,
        staged_sha256,
    })
}
