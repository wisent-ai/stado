//! `stado release install-local` — verify the delivered archive, install one
//! member, and reconcile every reader of the name it replaced.

mod config_gate;

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

use super::retain::{
    declare_delivered_version, extract_member, retain_archive, retained_archive_path,
};
use super::{converge_service_local_stado_readers, regular_file_matches, ReleaseInstallLocalArgs};

/// Verify the delivered archive against the delivery contract's digest,
/// extract one member, and install it under `$HOME/.stado/bin` by rename —
/// Linux refuses to write into a running executable (ETXTBSY) but allows
/// replacing the name, and a dated backup is kept beside it.
pub(in crate::cli::release_cmd) async fn install_local(
    args: &ReleaseInstallLocalArgs,
) -> Result<(), CmdError> {
    let name = if args.name.is_empty() {
        args.member
            .rsplit('/')
            .next()
            .unwrap_or(args.member.as_str())
            .to_string()
    } else {
        args.name.clone()
    };
    // The version a delivery of this very product carries. Every product the
    // release agent delivers through this command is declared under the
    // host's `managed_versions` afterwards; before, only Stado was, so a
    // delivered Skarbiec left the host declaring the version it replaced and
    // `release version show` read the host as ahead of its declaration.
    let delivered_version = if std::env::var("WISENT_PRODUCT").ok().as_deref() == Some(&name) {
        let version = std::env::var("WISENT_VERSION").map_err(|_| {
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
    let archive = std::env::var("WISENT_RELEASE_ARCHIVE")
        .map_err(|_| CmdError::click("WISENT_RELEASE_ARCHIVE is not set; this command is the delivery contract's local endpoint").stating(crate::primitives::failure::FailureCode::Config))?;
    let expected = std::env::var("WISENT_RELEASE_SHA256")
        .map_err(|_| CmdError::click("WISENT_RELEASE_SHA256 is not set; this command is the delivery contract's local endpoint").stating(crate::primitives::failure::FailureCode::Config))?;
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
/// and reconcile every reader of the replaced name. `install-local` feeds it
/// the delivery contract's archive; `restore-local` feeds it an archive an
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
        if destination.exists() {
            let stamp = chrono::Utc::now().format("%Y%m%d");
            let backup = directory.join(format!("{name}.release-backup-{stamp}"));
            if !backup.exists() {
                std::fs::copy(&destination, &backup).map_err(|error| {
                    CmdError::click(format!("cannot back up {name}: {error}"))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?;
            }
        }
        std::fs::write(&staged, &content).map_err(|error| {
            CmdError::click(format!("cannot stage {name}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(
                |error| {
                    CmdError::click(format!("cannot mark {name} executable: {error}"))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                },
            )?;
        }
    }
    // An incoming Stado reads this host's configuration with its own rules,
    // and a rule the configuration does not meet closes the served
    // boundaries only once that binary runs. Stado 0.22.0 replaced the
    // control host's binary and every boundary of its object API answered
    // `503 object authorization unavailable`, leaving nothing through which
    // Stado could read, repair or roll itself back. So the incoming binary
    // validates the configuration first, after its own migrations, and one it
    // refuses is not installed.
    if stado_version.is_some() && !root_already_current {
        if let Err(refusal) = config_gate::admit(&staged, &name) {
            let _ = std::fs::remove_file(&staged);
            if let Some(path) = &release_version_stage {
                let _ = std::fs::remove_file(path);
            }
            return Err(refusal);
        }
    }
    // A program installed products call is replaced only by one that still
    // answers every command they were recorded running; the same check
    // `stado product install` makes before it places a program.
    if !root_already_current {
        let refused = stado_product::common::Runtime::new(None).and_then(|runtime| {
            stado_product::callers::refuse_removed(&runtime, &destination, &staged)
        });
        if let Err(refusal) = refused {
            let _ = std::fs::remove_file(&staged);
            if let Some(path) = &release_version_stage {
                let _ = std::fs::remove_file(path);
            }
            return Err(CmdError::refused(format!("{refusal:#}")));
        }
    }
    // Leave the receipt the fleet's provenance check reads, before the
    // install replaces the name.
    //
    // `cli::service_converge::attest_installed` decides provenance by byte
    // comparing the installed file against
    // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, which
    // `deploy::host_release` writes when IT delivers. This command is the
    // other delivery endpoint and it staged nothing, so a binary it installed
    // read `unattested` forever after — even though the archive was verified
    // against the contract digest a hundred lines above.
    //
    // A host whose deliveries plainly ran — its `~/.stado/bin` carries this
    // command's dated backups and its `stado.release-version` handshake —
    // would otherwise hold nothing recent under `~/.stado/releases/stado`.
    // `stado release version show` then reports the host's binary as bytes the
    // fleet cannot attest, and the remediation it prints — deliver a
    // published version — is the thing that has just happened.
    //
    // Never fatal: the archive is verified and the install is the point, so a
    // receipt that cannot be written is named and the delivery continues.
    match (
        stado_version.clone().or_else(|| {
            std::env::var("WISENT_VERSION")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        }),
        crate::self_update::platform_triple_short(),
    ) {
        (Some(version), Ok(platform)) => {
            let attestation_source = if root_already_current {
                &destination
            } else {
                &staged
            };
            if let Err(error) = crate::self_update::stage_for_attestation(
                &name,
                &version,
                platform,
                attestation_source,
            ) {
                println!(
                    "release install-local: {name} {version} root bytes are verified but its \
                     attestation copy could not be staged, so `stado release version show` will \
                     read it as unattested: {error}"
                );
            }
        }
        (None, _) => println!(
            "release install-local: WISENT_VERSION is unset, so no attestation copy was staged \
             and `stado release version show` will read {name} as unattested"
        ),
        (_, Err(error)) => println!(
            "release install-local: this platform has no release triple ({error}), so no \
             attestation copy was staged for {name}"
        ),
    }
    let mut in_flight_log = |message: &str| println!("release install-local: {message}");
    let in_flight = crate::self_update::ReplacementInFlight::begin(
        &directory,
        std::slice::from_ref(&name),
        &mut in_flight_log,
    );
    if !root_already_current {
        std::fs::rename(&staged, &destination).map_err(|error| {
            CmdError::click(format!("cannot install {name}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    if let Some(staged_version) = release_version_stage {
        let installed_version = directory.join("stado.release-version");
        std::fs::rename(&staged_version, &installed_version).map_err(|error| {
            CmdError::click(format!(
                "cannot activate the installed Stado release coordinate: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    // The handshake above is the queue agent's, and only the queue agent
    // implements it: `providers::local::agent` is the sole reader of
    // `stado.release-version`. Every other unit launched from this directory
    // — the disk-cleanup janitor, the resolver, the health beacon — keeps
    // executing the inode it started with, for as long as it lives, because
    // nothing tells launchd or systemd that the file underneath changed.
    //
    // Installation is not activation: every non-agent reader must execute the
    // delivered image before it can consume that release's configuration schema.
    //
    // In place, and never the agent: see `self_update::recycle_replaced_units`.
    // Run this image check even when the pathname already matches the delivered
    // bytes. A prior attempt can replace the root and fail after recycling only
    // some global readers; matching processes are skipped, while any remaining
    // process on the replaced inode still has to converge before resume succeeds.
    let mut recycle_log = |message: &str| println!("{message}");
    crate::self_update::recycle_replaced_units(
        "release install-local",
        &directory,
        std::slice::from_ref(&name),
        &mut recycle_log,
    )
    .await
    // Every way the recycle fails is the host's service manager or process
    // table refusing to restart a reader: the host's outage.
    .map_err(CmdError::unreachable)?;
    drop(in_flight);
    if stado_version.is_some() {
        converge_service_local_stado_readers(
            "release install-local",
            &destination,
            &reader_archive,
        )
        .await?;
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
