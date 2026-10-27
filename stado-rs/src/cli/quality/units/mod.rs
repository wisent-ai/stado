//! Check the host's native, installed unit argv with the binary a release
//! would put at that unit's executable path. Registry declarations select the
//! units; the unit files themselves are the evidence of what init will run.
mod qualification;

pub(crate) use qualification::check as qualify;

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;
use crate::deploy::{production_runner, service};

pub(crate) async fn check(
    candidate: &Path,
    program: &str,
    host: Option<&str>,
    platform: Option<&str>,
) -> Result<(), CmdError> {
    let registry = if host.is_some() {
        let (registry, notice) = crate::targets::registry_without_authority("unit argv admission")
            .await
            .map_err(|cause| {
                CmdError::unreachable(format!(
                    "cannot read local registry for unit argv admission: {cause}"
                ))
            })?;
        if let Some(notice) = notice {
            eprintln!("{notice}");
        }
        registry
    } else {
        crate::deploy::host_channel::canonical_registry()
            .await
            .map_err(CmdError::from)?
    };
    if let Some(name) = host {
        if !registry.targets.iter().any(|target| target.name == name) {
            return Err(CmdError::refused(format!(
                "registry declares no host {name}"
            )));
        }
    }
    let runner = production_runner();
    let mut checked = Vec::new();
    for target in registry.targets.iter().filter(|target| {
        host.is_none_or(|name| target.name == name)
            && platform.is_none_or(|name| target.release_platform == name)
    }) {
        for declared in service::declared_services(target) {
            if !declared.program.is_empty()
                && crate::deploy::service_catalog::executable_name(&declared.program)
                    != Some(program)
            {
                continue;
            }
            let file = service::fetch_unit_file(target, &declared, &runner)
                .await
                .map_err(|cause| {
                    CmdError::unreachable(format!(
                        "{} {}: cannot read installed unit argv: {cause}",
                        target.name,
                        declared.unit_id()
                    ))
                })?;
            let unit =
                service::parse_local_unit_file(&file.content, file.kind).map_err(|cause| {
                    CmdError::refused(format!(
                        "{} {} at {}: cannot parse installed unit: {cause}",
                        target.name, file.unit, file.path
                    ))
                })?;
            if crate::deploy::service_catalog::executable_name(&unit.program) != Some(program) {
                continue;
            }
            let (_, args) = unit.arguments.split_first().ok_or_else(|| {
                CmdError::refused(format!(
                    "{} {} at {} runs {program} without an argument vector",
                    target.name, file.unit, file.path
                ))
            })?;
            let output = crate::wait::output(
                Command::new(candidate)
                    .args(["quality", "parse", "--"])
                    .args(args),
            )
            .map_err(|cause| {
                CmdError::unreachable(format!(
                    "{} {}: cannot run candidate parser {}: {cause}",
                    target.name,
                    file.unit,
                    candidate.display()
                ))
            })?;
            if !output.status.success() {
                return Err(CmdError::refused(format!(
                    "{} {} at {} invokes {program} {:?}; candidate {} rejected that argv with {}: stdout: {}; stderr: {}",
                    target.name, file.unit, file.path, args, candidate.display(), output.status,
                    String::from_utf8_lossy(&output.stdout).trim(),
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            checked.push(format!("{} {}", target.name, file.unit));
        }
    }
    if let Some(host) = host.filter(|_host| checked.is_empty()) {
        return Err(CmdError::refused(format!(
            "{host}: no installed registry-managed unit invoking {program} was read; unit argv compatibility was not established"
        )));
    }
    println!(
        "candidate {} accepts installed unit argv: {}",
        candidate.display(),
        checked.join(", ")
    );
    Ok(())
}
