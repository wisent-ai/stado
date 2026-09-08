//! `stado release install-local` — verify the delivered archive, install one
//! member, and reconcile every reader of the name it replaced.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

use super::{converge_service_local_stado_readers, regular_file_matches, ReleaseInstallLocalArgs};

/// Verify the delivered archive against the delivery contract's digest,
/// extract one member, and install it under `$HOME/.stado/bin` by rename —
/// Linux refuses to write into a running executable (ETXTBSY) but allows
/// replacing the name, and a dated backup is kept beside it.
pub(in crate::cli::release_cmd) async fn install_local(
    args: &ReleaseInstallLocalArgs,
) -> Result<(), CmdError> {
    use sha2::Digest as _;
    let name = if args.name.is_empty() {
        args.member
            .rsplit('/')
            .next()
            .unwrap_or(args.member.as_str())
            .to_string()
    } else {
        args.name.clone()
    };
    let home = crate::config_file::expand_tilde("~");
    let stado_version =
        if name == "stado" && std::env::var("WISENT_PRODUCT").ok().as_deref() == Some("stado") {
            let version = std::env::var("WISENT_VERSION")
                .map_err(|_| CmdError::click("WISENT_VERSION is not set for the Stado delivery"))?;
            let version = version.trim();
            if !crate::deploy::host_release::is_exact_semver(version) {
                return Err(CmdError::click(
                    "WISENT_VERSION is not an exact semantic version for the Stado delivery",
                ));
            }
            Some(version.to_string())
        } else {
            None
        };
    let archive = std::env::var("WISENT_RELEASE_ARCHIVE")
        .map_err(|_| CmdError::click("WISENT_RELEASE_ARCHIVE is not set; this command is the delivery contract's local endpoint"))?;
    if stado_version.is_some()
        && !std::fs::symlink_metadata(&archive)
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot inspect delivered Stado archive {archive}: {error}"
                ))
            })?
            .file_type()
            .is_file()
    {
        return Err(CmdError::click(
            "delivered Stado archive must be a regular file, not a symlink",
        ));
    }
    let expected = std::env::var("WISENT_RELEASE_SHA256")
        .map_err(|_| CmdError::click("WISENT_RELEASE_SHA256 is not set; this command is the delivery contract's local endpoint"))?;
    let bytes = std::fs::read(&archive).map_err(|error| {
        CmdError::click(format!("cannot read delivered archive {archive}: {error}"))
    })?;
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(CmdError::click(format!(
            "delivered archive digest mismatch: expected {expected}, got {actual}"
        )));
    }
    let reader_archive = if let Some(version) = stado_version.as_deref() {
        let platform = crate::self_update::platform_triple_short()
            .map_err(|error| CmdError::click(error.to_string()))?;
        let retained_dir = home
            .join(".stado")
            .join("releases")
            .join("stado")
            .join(version)
            .join(platform);
        std::fs::create_dir_all(&retained_dir).map_err(|error| {
            CmdError::click(format!(
                "cannot prepare retained Stado archive directory {}: {error}",
                retained_dir.display()
            ))
        })?;
        retained_dir.join(crate::deploy::host_release::READER_ARCHIVE_NAME)
    } else {
        PathBuf::from(&archive)
    };
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut bundle = tar::Archive::new(decoder);
    let member = args.member.trim_start_matches('/');
    let mut payload: Option<Vec<u8>> = None;
    for entry in bundle
        .entries()
        .map_err(|error| CmdError::click(format!("unreadable release archive: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| CmdError::click(format!("unreadable archive entry: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| CmdError::click(format!("unreadable archive path: {error}")))?
            .to_string_lossy()
            .into_owned();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if path == member || path.ends_with(&format!("/{member}")) {
            let mut content = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut content)
                .map_err(|error| CmdError::click(format!("cannot extract {member}: {error}")))?;
            payload = Some(content);
            break;
        }
    }
    let Some(content) = payload else {
        return Err(CmdError::click(format!(
            "release archive carries no regular member {member}"
        )));
    };
    if stado_version.is_some() && Path::new(&archive) != reader_archive {
        std::fs::rename(&archive, &reader_archive).map_err(|error| {
            CmdError::click(format!(
                "cannot retain delivered Stado archive at {}: {error}",
                reader_archive.display()
            ))
        })?;
    }
    let directory = home.join(".stado").join("bin");
    std::fs::create_dir_all(&directory).map_err(|error| {
        CmdError::click(format!("cannot prepare {}: {error}", directory.display()))
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
                std::fs::copy(&destination, &backup)
                    .map_err(|error| CmdError::click(format!("cannot back up {name}: {error}")))?;
            }
        }
        std::fs::write(&staged, &content)
            .map_err(|error| CmdError::click(format!("cannot stage {name}: {error}")))?;
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(
                |error| CmdError::click(format!("cannot mark {name} executable: {error}")),
            )?;
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
    // lukasz-macbook is the proof. Its `~/.stado/bin` carries this command's
    // own dated backups through 2026-09-02 and its `stado.release-version`
    // handshake, so deliveries plainly ran; `~/.stado/releases/stado` holds
    // 0.13.24 and older, nothing since. `stado service converge` therefore
    // reported the host's binary as bytes the fleet cannot attest, and the
    // remediation it printed — deliver a published version — was the thing
    // that had just happened.
    //
    // Never fatal: the archive is verified and the install is the point, so a
    // receipt that cannot be written is named and the delivery continues.
    match (
        std::env::var("WISENT_VERSION")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
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
                     attestation copy could not be staged, so `stado service converge` will \
                     read it as unattested: {error}"
                );
            }
        }
        (None, _) => println!(
            "release install-local: WISENT_VERSION is unset, so no attestation copy was staged \
             and `stado service converge` will read {name} as unattested"
        ),
        (_, Err(error)) => println!(
            "release install-local: this platform has no release triple ({error}), so no \
             attestation copy was staged for {name}"
        ),
    }
    if !root_already_current {
        std::fs::rename(&staged, &destination)
            .map_err(|error| CmdError::click(format!("cannot install {name}: {error}")))?;
    }
    if let Some(staged_version) = release_version_stage {
        let installed_version = directory.join("stado.release-version");
        std::fs::rename(&staged_version, &installed_version).map_err(|error| {
            CmdError::click(format!(
                "cannot activate the installed Stado release coordinate: {error}"
            ))
        })?;
    }
    // The handshake above is the queue agent's, and only the queue agent
    // implements it: `providers::local::agent` is the sole reader of
    // `stado.release-version`. Every other unit launched from this directory
    // — the disk-cleanup janitor, the resolver, the health beacon — keeps
    // executing the inode it started with, for as long as it lives, because
    // nothing tells launchd or systemd that the file underneath changed.
    //
    // That is how a delivery could succeed and change nothing. On 2026-09-01
    // the janitor on lukasz-macbook was still executing a 68,977,488-byte
    // image of this exact path while the file was 70,265,008 bytes, had
    // answered `invalid_or_unavailable_policy` 8,460 times out of 12,009
    // passes because the policy no longer validated against the code it was
    // compiled from, and the volume had reached 100% full with a janitor
    // running every minute the whole way down.
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
    .map_err(CmdError::click)?;
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
    Ok(())
}
