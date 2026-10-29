//! `stado release local install` — verify the delivered archive, install one
//! member, and reconcile every reader of the name it replaced.

mod activation;
mod attestation;
mod unit_migration;

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

use super::retain::{
    declare_delivered_version, extract_member, retain_archive, retained_archive_path,
};
use super::{converge_service_local_stado_readers, regular_file_matches, ReleaseInstallLocalArgs};

/// The command every line this install prints is prefixed with.
const VERB: &str = "release local install";

/// Verify the delivered archive against the delivery contract's digest,
/// extract one member, and install it under `$HOME/.stado/bin` by rename —
/// Linux refuses to write into a running executable (ETXTBSY) but allows
/// replacing the name, and a dated backup is kept beside it.
pub(in crate::cli::release_cmd) async fn install_local(
    args: &ReleaseInstallLocalArgs,
) -> Result<(), CmdError> {
    let name = if args.name.is_empty() {
        match args.member.rsplit_once('/') {
            Some((_, last)) => last.to_string(),
            None => args.member.clone(),
        }
    } else {
        args.name.clone()
    };
    // A delivery contract variable set to text that is not Unicode is
    // refused by name, not read as unset.
    let declared = |variable: &str| {
        crate::primitives::environment::declared(variable).map_err(|cause| {
            CmdError::click(cause).stating(crate::primitives::failure::FailureCode::Config)
        })
    };
    // The version a delivery of this very product carries. Every product the
    // release agent delivers through this command is declared under the
    // host's `managed_versions` afterwards; before, only Stado was, so a
    // delivered Skarbiec left the host declaring the version it replaced and
    // `release version show` read the host as ahead of its declaration.
    let delivered_version = if declared("WISENT_PRODUCT")?.as_deref() == Some(name.as_str()) {
        let version = declared("WISENT_VERSION")?.ok_or_else(|| {
            CmdError::click(format!("WISENT_VERSION is not set for the {name} delivery"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        let version = version.trim();
        if !crate::deploy::host_release::is_exact_semver(version) {
            return Err(CmdError::click(format!(
                "WISENT_VERSION is not an exact semantic version for the {name} delivery"
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        Some(version.to_string())
    } else {
        None
    };
    let stado_version = delivered_version.clone().filter(|_| name == "stado");
    let archive = declared("WISENT_RELEASE_ARCHIVE")?
        .ok_or_else(|| CmdError::click("WISENT_RELEASE_ARCHIVE is not set; this command is the delivery contract's local endpoint").stating(crate::primitives::failure::FailureCode::Config))?;
    let expected = declared("WISENT_RELEASE_SHA256")?
        .ok_or_else(|| CmdError::click("WISENT_RELEASE_SHA256 is not set; this command is the delivery contract's local endpoint").stating(crate::primitives::failure::FailureCode::Config))?;
    install_archive(
        name.clone(),
        &args.member,
        &archive,
        &expected,
        stado_version,
        true,
    )
    .await?;
    // Stado's own declaration is written inside `install_archive`, after its
    // readers converged; every other product's is written here.
    match delivered_version {
        Some(version) if name != "stado" => declare_delivered_version(&name, &version).await,
        _ => Ok(()),
    }
}

/// Verify one release archive, install its member under `$HOME/.stado/bin`,
/// and reconcile every reader of the replaced name. `release local install`
/// feeds it the delivery contract's archive; `release local restore` feeds it an archive an
/// earlier delivery retained on this host, and does not declare the version
/// because the declaration lives in the registry it is restoring access to.
pub(in crate::cli::release_cmd) async fn install_archive(
    name: String,
    member: &str,
    archive: &str,
    expected: &str,
    stado_version: Option<String>,
    declare: bool,
) -> Result<(), CmdError> {
    use sha2::Digest as _;
    if let Some(version) = stado_version.as_deref() {
        super::failed::refuse_failed(version, declare).await?;
    }
    let home = crate::config_file::expand_tilde("~");
    if stado_version.is_some()
        && !std::fs::symlink_metadata(archive)
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot inspect delivered Stado archive {archive}: {error}"
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?
            .file_type()
            .is_file()
    {
        return Err(CmdError::refused(
            "delivered Stado archive must be a regular file, not a symlink",
        ));
    }
    let bytes = std::fs::read(archive).map_err(|error| {
        CmdError::click(format!("cannot read delivered archive {archive}: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(CmdError::click(format!(
            "delivered archive digest mismatch: expected {expected}, got {actual}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let reader_archive = match stado_version.as_deref() {
        Some(version) => retained_archive_path(&home, version)?,
        None => PathBuf::from(archive),
    };
    let content = extract_member(bytes, member)?;
    if stado_version.is_some() && Path::new(archive) != reader_archive {
        retain_archive(Path::new(archive), &reader_archive)?;
    }
    let directory = home.join(".stado").join("bin");
    std::fs::create_dir_all(&directory).map_err(|error| {
        CmdError::click(format!("cannot prepare {}: {error}", directory.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    // The installed coordinate is the cheap, persistent handshake between the
    // delivery child and already-running queue agents. Agents launched from
    // this managed path finish their current jobs, compare this file with their
    // compiled version, then let their declared supervisor recreate them.
    let release_version_stage = if let Some(version) = stado_version.as_deref() {
        let path = directory.join("stado.release-version.release-incoming");
        std::fs::write(&path, format!("{version}\n")).map_err(|error| {
            CmdError::click(format!(
                "cannot stage the installed Stado release coordinate: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        Some(path)
    } else {
        None
    };
    let destination = directory.join(&name);
    let root_already_current = regular_file_matches(&destination, &content)?;
    let staged = directory.join(format!("{name}.release-incoming"));
    if !root_already_current {
        // A binary that cannot be examined refuses: read as absent, it would
        // be replaced without the `.previous` a restore reinstalls.
        let replaced = crate::primitives::presence::present(&destination).map_err(|error| {
            CmdError::click(error.to_string())
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        if replaced {
            // The replaced binary becomes `<name>.previous`, the rollback
            // target `release local restore --previous` reinstalls and
            // reclaim keeps the retained release of; staged, then renamed.
            let held = directory.join(format!("{name}.previous.release-incoming"));
            std::fs::copy(&destination, &held)
                .and_then(|_| std::fs::rename(&held, directory.join(format!("{name}.previous"))))
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot keep the replaced {name} as {name}.previous: {error}"
                    ))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?;
        }
        std::fs::write(&staged, &content).map_err(|error| {
            CmdError::click(format!("cannot stage {name}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(
                &staged,
                std::fs::Permissions::from_mode(crate::primitives::file_mode::program()),
            )
            .map_err(|error| {
                CmdError::click(format!("cannot mark {name} executable: {error}"))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        }
    }
    // An incoming Stado reads this host's configuration with its own rules,
    // and a rule the configuration does not meet closes the served
    // boundaries only once that binary runs. So the incoming binary judges
    // this host first, after its own migrations, and one it refuses is not
    // installed — whether or not the delivery names a version
    // (`stado_product::callers::configuration`).
    let candidate = if root_already_current {
        &destination
    } else {
        &staged
    };
    if stado_product::callers::configuration::replaces_running_program(&destination) {
        if let Err(refusal) =
            stado_product::callers::configuration::refuse_unaccepted(candidate, &name)
        {
            discard_stages(&staged, release_version_stage.as_deref());
            return Err(CmdError::refused(format!("{refusal:#}")));
        }
    }
    let resident_host = if stado_version.is_some() {
        match unit_migration::admit(candidate, &destination, &name, &home) {
            Ok(host) => host,
            Err(refusal) => {
                discard_stages(&staged, release_version_stage.as_deref());
                return Err(refusal);
            }
        }
    } else {
        None
    };
    // A program installed products call is replaced only by one that still
    // answers every command they were recorded running; the same check
    // `stado product install` makes before it places a program.
    if !root_already_current {
        let refused = stado_product::common::Runtime::new(None).and_then(|runtime| {
            stado_product::callers::refuse_removed(&runtime, &destination, &staged)
        });
        if let Err(refusal) = refused {
            discard_stages(&staged, release_version_stage.as_deref());
            let refusal = CmdError::refused(format!("{refusal:#}"));
            return Err(match resident_host {
                Some(_) => unit_migration::restored_after(&destination, &name, &home, refusal),
                None => refusal,
            });
        }
    }
    // Leave the receipt the fleet's provenance check reads (`attestation`),
    // before the install replaces the name.
    attestation::stage(
        VERB,
        &name,
        stado_version.as_deref(),
        if root_already_current {
            &destination
        } else {
            &staged
        },
    );
    activation::place(
        &directory,
        &name,
        &staged,
        release_version_stage.as_deref(),
        root_already_current,
        resident_host.as_deref(),
    )
    .await?;
    if stado_version.is_some() {
        converge_service_local_stado_readers(VERB, &destination, &reader_archive).await?;
    }
    if root_already_current {
        println!(
            "verified {} already matches the delivered release archive",
            destination.display()
        );
    } else {
        println!(
            "installed {} from the delivered release archive",
            destination.display()
        );
    }
    if let Some(version) = stado_version.as_deref().filter(|_| declare) {
        declare_delivered_version(&name, version).await?;
    }
    Ok(())
}

/// Remove the staged program and the staged release version a refused
/// install leaves, and say each one that stays: the next install of the same
/// release stages under the same names.
fn discard_stages(staged: &Path, release_version_stage: Option<&Path>) {
    for path in std::iter::once(staged).chain(release_version_stage) {
        if let Some(left) = crate::primitives::staging::remove(path) {
            eprintln!("{VERB}: {left}");
        }
    }
}
