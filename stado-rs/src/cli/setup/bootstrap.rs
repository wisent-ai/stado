//! `stado bootstrap` — provision wisent-compute services persistently
//! across reboots. Thin CLI shell over [`crate::deploy::bootstrap`]
//! (Python `bootstrap` command in `stado/cli.py`).

use crate::cli::CmdError;

/// `bootstrap [--target NAME] [--dry-run] [--local] [--print-install-script]`
/// command body.
pub async fn run(
    target: Option<String>,
    dry_run: bool,
    local: bool,
    print_install_script: bool,
) -> Result<(), CmdError> {
    if print_install_script {
        // The one verified installer of a Stado release, for a machine that
        // has no Stado yet: a container image build, or a host being
        // provisioned by hand.
        print!(
            "{}",
            crate::deploy::bootstrap::remote_install_script(
                &crate::config::stado_api_url(),
                &crate::config::stado_release_version(),
            )
        );
        return Ok(());
    }
    run_reporting(target, dry_run, local, &mut |line: &str| println!("{line}")).await
}

/// The bootstrap itself, with its progress lines handed to `echo`: standard
/// output for `stado bootstrap`, standard error for a caller whose standard
/// output carries one JSON answer (`stado fleet enroll --json`).
pub async fn run_reporting(
    target: Option<String>,
    dry_run: bool,
    local: bool,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(CmdError::from)?;
    let runner = crate::deploy::production_runner();
    let hf_fetch = crate::deploy::local_install::production_hf_fetcher();
    crate::deploy::bootstrap::run_bootstrap(
        &registry,
        target.as_deref(),
        dry_run,
        local,
        &runner,
        &hf_fetch,
        echo,
    )
    .await
    .map_err(|exc| CmdError::click(exc.to_string()))
}
