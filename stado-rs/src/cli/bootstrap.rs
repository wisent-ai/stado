//! `stado bootstrap` — provision wisent-compute services persistently
//! across reboots. Thin CLI shell over [`crate::deploy::bootstrap`]
//! (Python `bootstrap` command in `stado/cli.py`).

use super::CmdError;

/// `bootstrap [--target NAME] [--dry-run] [--local]` command body.
pub async fn run(target: Option<String>, dry_run: bool, local: bool) -> Result<(), CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let runner = crate::deploy::production_runner();
    let hf_fetch = crate::deploy::local_install::production_hf_fetcher();
    let mut echo = |line: &str| println!("{line}");
    crate::deploy::bootstrap::run_bootstrap(
        &registry,
        target.as_deref(),
        dry_run,
        local,
        &runner,
        &hf_fetch,
        &mut echo,
    )
    .await
    .map_err(|exc| CmdError::click(exc.to_string()))
}
