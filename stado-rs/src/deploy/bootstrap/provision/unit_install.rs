//! Stage three helper: run one prepared unit-install command and turn a
//! non-zero remote exit into the provision's error.

use crate::deploy::{CommandSpec, DeployError, Runner};

pub(super) async fn run_unit_install(
    spec: &CommandSpec,
    runner: &Runner,
) -> Result<(), DeployError> {
    let output = runner(spec.clone()).await.map_err(DeployError)?;
    if !output.ok() {
        return Err(DeployError(format!(
            "unit install failed: {}",
            output.detail()
        )));
    }
    Ok(())
}
