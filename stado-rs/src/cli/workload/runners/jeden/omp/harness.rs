//! `stado workload harness install|status KIND --target HOST`: the harness a
//! session kind declares, carried from this machine to a host's managed path.
//!
//! The copy the operator runs here is the one a host receives, so a session
//! there behaves as the ones here do. It travels through a managed run
//! directory, is checked there by running it (`--version` must answer what
//! this machine's copy answers), and only then replaces the managed program
//! in one rename on the host's own volume.

use serde_json::json;

use crate::cli::workload::catalog::{WorkloadHarness, WorkloadKind};
use crate::cli::CmdError;
use crate::deploy::{host_channel, host_delivery, shlex_quote};
use crate::primitives::failure::FailureCode;

/// The managed run area deliveries are made into.
const RUN_AREA: &str = ".stado/work/runs";

/// The harness program on this machine's `PATH`, and what it says its
/// version is.
async fn local_harness(
    harness: &WorkloadHarness,
) -> Result<(std::path::PathBuf, String), CmdError> {
    let path = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|directory| directory.join(&harness.program))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} is not on this machine's PATH, so there is no copy to carry to a host",
                harness.program
            ))
            .stating(FailureCode::NotFound)
        })?;
    let output =
        crate::wait::output_async(tokio::process::Command::new(&path).arg("--version")).await?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{} --version failed here ({}): {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok((
        path,
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
}

/// What the managed harness on `target` answers to `--version`, or nothing
/// when it is not installed there.
async fn host_version(
    target: &crate::targets::ComputeTarget,
    managed: &str,
) -> Result<Option<String>, CmdError> {
    let script = format!(
        "program=\"$HOME\"/{}\n[ -x \"$program\" ] || exit 0\n\"$program\" --version\n",
        shlex_quote(managed)
    );
    let output = host_channel::run_script(target, &script, &crate::deploy::production_runner())
        .await
        .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "the managed harness on {} did not answer --version (exit {}): {}",
            target.name,
            output.code,
            output.detail().trim()
        )));
    }
    let version = output.stdout.trim().to_string();
    Ok(if version.is_empty() {
        None
    } else {
        Some(version)
    })
}

fn declared(kind: &WorkloadKind) -> Result<&WorkloadHarness, CmdError> {
    kind.harness.as_ref().ok_or_else(|| {
        CmdError::usage(format!(
            "workload kind '{}' runs no harness the fleet carries; its runtime is installed as \
             a product",
            kind.kind
        ))
    })
}

pub(crate) async fn status(
    kind: &WorkloadKind,
    target: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let harness = declared(kind)?;
    let (path, here) = local_harness(harness).await?;
    let target = crate::cli::canonical_host(target).await?;
    let there = host_version(&target, &harness.managed).await?;
    let current = there.as_deref() == Some(here.as_str());
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "kind": kind.kind,
            "target": target.name,
            "source": path,
            "version_here": here,
            "version_there": there,
            "managed": format!("~/{}", harness.managed),
            "current": current,
        }));
    } else {
        match there {
            Some(version) => println!(
                "{} on {}: {version} at ~/{} (this machine runs {here}{})",
                harness.program,
                target.name,
                harness.managed,
                if current {
                    ""
                } else {
                    "; install it with `stado workload harness install`"
                }
            ),
            None => println!(
                "{} is not installed on {}; install it with `stado workload harness install {} \
                 --target {}`",
                harness.program, target.name, kind.kind, target.name
            ),
        }
    }
    Ok(())
}

pub(crate) async fn install(
    kind: &WorkloadKind,
    target: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let harness = declared(kind)?;
    let (path, here) = local_harness(harness).await?;
    let target = crate::cli::canonical_host(target).await?;
    // The copy carried is this machine's program, built for this machine's
    // platform; a host on another platform could not run it.
    let platform =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .map_err(CmdError::refused)?;
    if target.release_platform != platform {
        return Err(CmdError::refused(format!(
            "{} declares release platform {:?} and this machine's {} is built for {platform}; \
             install the harness from a {} machine of the fleet",
            target.name, target.release_platform, harness.program, target.release_platform
        )));
    }
    let runner = crate::deploy::production_runner();
    let previous = host_version(&target, &harness.managed).await?;
    if previous.as_deref() != Some(here.as_str()) {
        let run_id = uuid::Uuid::new_v4().to_string();
        let delivered = format!("{RUN_AREA}/{run_id}/{}", harness.program);
        host_delivery::deliver_host(
            &target.name,
            &path.to_string_lossy(),
            &delivered,
            None,
            &runner,
        )
        .await
        .map_err(CmdError::from)?;
        let script = format!(
            "set -u\nhome=${{HOME%/}}\nstaged=\"$home\"/{staged}\nmanaged=\"$home\"/{managed}\n\
             answer=$(\"$staged\" --version) || exit\n\
             if [ \"$answer\" != {expected} ]; then printf 'the delivered harness answers %s, not %s\\n' \"$answer\" {expected} >&2; exit 1; fi\n\
             mkdir -p \"${{managed%/*}}\" || exit\n\
             mv -f \"$staged\" \"$managed\" || exit\n\
             rm -rf \"$home\"/{run}\n",
            staged = shlex_quote(&delivered),
            managed = shlex_quote(&harness.managed),
            expected = shlex_quote(&here),
            run = shlex_quote(&format!("{RUN_AREA}/{run_id}")),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(CmdError::from)?;
        if !output.succeeded() {
            return Err(CmdError::click(format!(
                "{} was delivered to {} but not installed at ~/{} (exit {}): {}",
                harness.program,
                target.name,
                harness.managed,
                output.code,
                output.detail().trim()
            )));
        }
    }
    let installed = host_version(&target, &harness.managed).await?;
    if installed.as_deref() != Some(here.as_str()) {
        return Err(CmdError::click(format!(
            "~/{} on {} answers {installed:?} after the install, not {here}",
            harness.managed, target.name
        )));
    }
    let changed = previous.as_deref() != Some(here.as_str());
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "kind": kind.kind,
            "target": target.name,
            "version": here,
            "previous": previous,
            "managed": format!("~/{}", harness.managed),
            "changed": changed,
        }));
    } else if changed {
        println!(
            "{} {here} installed at ~/{} on {}",
            harness.program, harness.managed, target.name
        );
    } else {
        println!("{} on {} already runs {here}", harness.program, target.name);
    }
    Ok(())
}
