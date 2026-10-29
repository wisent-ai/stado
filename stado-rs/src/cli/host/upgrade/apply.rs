//! The steps of an upgrade that change something: the build on the builder,
//! the signature and archive made on this machine, the delivery into a run
//! of its own on the host, the incoming program's judgement of the host's
//! configuration, the install, the rollback to the kept program, and the
//! removal of every run and export the upgrade made.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;
use crate::deploy::host_run::{AttachedOutcome, CargoOperation, HostCargoRequest, RUN_AREA};
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

use super::installed::INSTALLED;
use super::source::{Source, MANIFEST};

/// The name of the archive the host installs from, beside the program.
const ARCHIVE: &str = "stado.tar.gz";
/// The archive member `release local install` installs by default.
const MEMBER: &str = "bin/stado";

/// The binary the builder brought back, and its digest.
pub(super) struct Built {
    pub path: PathBuf,
    pub sha256: String,
}

/// Whether the first bytes of `path` are an executable of the host's kind:
/// Mach-O (thin 64-bit or universal) for a Mac, ELF otherwise.
fn platform_matches(path: &Path, darwin: bool) -> Result<bool, CmdError> {
    use std::io::Read as _;
    let mut magic = [0_u8; 4];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| {
            CmdError::click(format!("cannot read the built {}: {error}", path.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    Ok(if darwin {
        magic == [0xcf, 0xfa, 0xed, 0xfe] || magic == [0xca, 0xfe, 0xba, 0xbe]
    } else {
        magic == [0x7f, b'E', b'L', b'F']
    })
}

/// Build `source` on `builder` and bring the binary back here.
pub(super) async fn build(
    builder: &ComputeTarget,
    source: &Source,
    darwin: bool,
    runner: &Runner,
) -> Result<Built, CmdError> {
    let request = HostCargoRequest {
        manifest_path: source.checkout.join(MANIFEST),
        revision: source.commit.clone(),
        operation: CargoOperation::Build,
        binaries: vec![INSTALLED.to_string()],
        arguments: Vec::new(),
    };
    crate::deploy::host_run::validate_cargo(&request).map_err(CmdError::from)?;
    let receipt = crate::deploy::host_run::cargo(builder, &request, runner)
        .await
        .map_err(CmdError::from)?;
    if receipt.exit_code != nix::libc::EXIT_SUCCESS {
        let tail: Vec<&str> = receipt.stderr.lines().rev().take(20).collect();
        return Err(CmdError::click(format!(
            "cargo build of {} on {} exited {}; nothing was installed. Its last lines:\n{}",
            source.commit,
            builder.name,
            receipt.exit_code,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )));
    }
    let artifact = receipt
        .artifacts
        .into_iter()
        .find(|artifact| artifact.binary == INSTALLED)
        .ok_or_else(|| {
            CmdError::unreachable(format!(
                "the build of {} on {} brought no {INSTALLED} back",
                source.commit, builder.name
            ))
        })?;
    if !platform_matches(&artifact.path, darwin)? {
        return Err(CmdError::refused(format!(
            "{} built on {} is not an executable for the host's platform; name a --builder of \
             the host's release platform",
            artifact.path.display(),
            builder.name
        )));
    }
    Ok(Built {
        path: artifact.path,
        sha256: artifact.sha256,
    })
}

/// This upgrade's own export area under Stado's build area for the
/// checkout, removed when the upgrade ends.
pub(super) fn work_area(source: &Source) -> Result<PathBuf, CmdError> {
    let home = crate::config_file::expand_tilde("~");
    let area = stado_product::common::runs::checkout_area(&home, &source.checkout)
        .map_err(|error| CmdError::declaration(format!("{error:#}")))?
        .join("host-upgrade")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(area.join("bin")).map_err(|error| {
        CmdError::click(format!("cannot prepare {}: {error}", area.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Ok(area)
}

/// The program placed in the work area, signed for a Mac with `choice` and
/// `identifier`; its signature report, `None` off macOS.
pub(super) fn place(
    built: &Built,
    area: &Path,
    sign: Option<(&stado_product::SigningChoice, &str)>,
) -> Result<(PathBuf, Option<serde_json::Value>), CmdError> {
    use std::os::unix::fs::PermissionsExt as _;
    let program = area.join(MEMBER);
    std::fs::copy(&built.path, &program)
        .and_then(|_| {
            std::fs::set_permissions(
                &program,
                std::fs::Permissions::from_mode(crate::primitives::file_mode::program()),
            )
        })
        .map_err(|error| {
            CmdError::click(format!(
                "cannot place {} at {}: {error}",
                built.path.display(),
                program.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    let Some((choice, identifier)) = sign else {
        return Ok((program, None));
    };
    let signed = stado_product::sign_program(choice, &program, identifier, None)
        .map_err(|error| CmdError::click(format!("signing {}: {error:#}", program.display())))?;
    if !stado_product::signature_acceptable(&signed) {
        return Err(CmdError::refused(format!(
            "{} was signed, but its signature is not one stable Apple identity: {signed}; \
             nothing was delivered",
            program.display()
        )));
    }
    Ok((program, Some(signed)))
}

/// Archive the placed program as `bin/stado`, and answer the archive's
/// SHA-256.
pub(super) fn package(area: &Path, program: &Path) -> Result<String, CmdError> {
    let archive = area.join(ARCHIVE);
    let written = (|| -> std::io::Result<()> {
        let file = std::fs::File::create(&archive)?;
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder.append_path_with_name(program, MEMBER)?;
        builder.into_inner()?.finish()?.sync_all()
    })();
    written.map_err(|error| {
        CmdError::click(format!("cannot write {}: {error}", archive.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    crate::primitives::digest::sha256_file(&archive)
        .map(|digest| digest.sha256)
        .map_err(|error| {
            CmdError::click(format!("cannot read {}: {error}", archive.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })
}

/// Where the delivered upgrade lives on the host.
pub(super) struct Delivered {
    pub run_directory: String,
    pub program: String,
    pub archive: String,
}

/// Deliver the work area into a run of its own on the host.
pub(super) async fn deliver(
    target: &ComputeTarget,
    area: &Path,
    home: &str,
    runner: &Runner,
) -> Result<Delivered, CmdError> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let destination = format!("{RUN_AREA}/{run_id}/upgrade");
    crate::deploy::host_delivery::deliver_host(
        &target.name,
        &area.to_string_lossy(),
        &destination,
        None,
        runner,
    )
    .await
    .map_err(CmdError::from)?;
    Ok(Delivered {
        run_directory: format!("{home}/{RUN_AREA}/{run_id}"),
        program: format!("{home}/{destination}/{MEMBER}"),
        archive: format!("{home}/{destination}/{ARCHIVE}"),
    })
}

/// Run the delivered program on the host with `arguments`; its streams are
/// captured when `capture`, else they pass through to this terminal.
pub(super) async fn attach(
    target: &ComputeTarget,
    delivered: &Delivered,
    arguments: &[String],
    capture: bool,
) -> Result<AttachedOutcome, CmdError> {
    crate::deploy::host_run::run_attached(target, &delivered.program, arguments, capture)
        .await
        .map_err(CmdError::from)
}

/// The outcome's exit status, or how it ended without one.
pub(super) fn ended(outcome: &AttachedOutcome) -> Result<(), String> {
    match outcome.exit_code {
        Some(code) if code == nix::libc::EXIT_SUCCESS => Ok(()),
        Some(code) => {
            let said: Vec<&str> = [outcome.stdout.as_deref(), outcome.stderr.as_deref()]
                .into_iter()
                .flatten()
                .flat_map(str::lines)
                .filter(|line| !line.trim().is_empty())
                .collect();
            Err(if said.is_empty() {
                format!("exited {code}")
            } else {
                format!("exited {code}: {}", said.join(" | "))
            })
        }
        None => Err(format!(
            "ended with {} and returned no exit code",
            outcome.ended
        )),
    }
}

/// Remove the run the upgrade delivered; what stays is said.
pub(super) async fn remove_run(
    target: &ComputeTarget,
    delivered: &Delivered,
    runner: &Runner,
) -> Result<String, String> {
    match crate::deploy::host_run::remove_run_directory(target, &delivered.run_directory, runner)
        .await
    {
        Ok(outcome) => Ok(format!("removed {} ({:?})", outcome.path, outcome.status)),
        Err(error) => Err(format!(
            "{} stays on {}: {error}; `stado host run remove {} {}` removes it",
            delivered.run_directory, target.name, target.name, delivered.run_directory
        )),
    }
}

/// Remove the local work area; what stays is said.
pub(super) fn remove_area(area: &Path) -> Result<String, String> {
    match std::fs::remove_dir_all(area) {
        Ok(()) => Ok(format!("removed {}", area.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(format!("{} was already gone", area.display()))
        }
        Err(error) => Err(format!("{} stays: {error}", area.display())),
    }
}
