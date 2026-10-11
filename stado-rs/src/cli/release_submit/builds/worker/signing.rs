//! The macOS code-signing step a darwin build runs after it compiled.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::CmdError;
use crate::release_pipeline::{StepReceipt, StepStatus, WorkerRequest};

use super::steps::execute;

/// Sign the staged `output` of `request`'s darwin build from `source`, and
/// return the step's receipt with, when the step could not start, its cause.
///
/// The signer is this worker's own Stado — never whatever signing program the
/// builder's PATH happens to carry, which can be nothing, ending a build
/// before its first signature with "cannot run wisent-products".
///
/// The identity is the fleet's too: the Apple certificate and key Skarbiec
/// holds, handed to the signer's own temporary keychain through its
/// environment, the way runner reconciliation already hands them over.
/// Signing with whatever identity the builder's login keychain holds would
/// let a build placed on the operator's laptop sign while the same build
/// placed on another host dies at `no Apple signing identity is available`,
/// spending a release coordinate on a placement decision.
pub(super) async fn macos_signing(
    request: &WorkerRequest,
    source: &Path,
    output: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<(StepReceipt, Option<String>), CmdError> {
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
    match crate::deploy::native_signing::signing_environment().await {
        Ok(identity) => {
            let mut signing_environment = environment.clone();
            signing_environment.extend(identity);
            let step = execute("macos-code-signing", &argv, source, &signing_environment)?;
            Ok((step, None))
        }
        Err(error) => {
            println!("[release-worker] step macos-code-signing: {error}");
            Ok((
                StepReceipt {
                    name: "macos-code-signing".into(),
                    argv,
                    status: StepStatus::Failed,
                    exit_code: None,
                },
                Some(error.to_string()),
            ))
        }
    }
}
