//! `stado release local restore` — reinstall a Stado release an earlier
//! delivery retained on this very host.
//!
//! A delivery keeps each Stado archive it installed under
//! `$HOME/.stado/releases/stado/<version>/<platform>/`. When the installed
//! Stado cannot serve — its rules refuse this host's configuration and every
//! boundary of the object API answers `503 object authorization unavailable`
//! — the registry, the release records and `release rollback` are behind that
//! same closed API, and `release version converge` refuses to downgrade a
//! host that runs a newer version than it declares. This command needs none of
//! them: it reads the retained archive from local disk and installs it through
//! the same checked install and reader convergence as `release local install`, so the
//! restored binary validates this host's configuration before it replaces
//! anything. Run it on the host with `stado host run attach`.

use clap::Args;

use crate::cli::CmdError;

use super::install::install_archive;

#[derive(Args)]
pub struct ReleaseRestoreLocalArgs {
    /// Exact retained Stado version to reinstall.
    #[arg(
        long,
        required_unless_present = "previous",
        conflicts_with = "previous"
    )]
    version: Option<String>,
    /// Reinstall the release this host ran before the installed one: the
    /// retained version whose staged `stado` is byte for byte the rollback
    /// target `~/.stado/bin/stado.previous` that install keeps and reclaim
    /// never removes. For a host whose installed Stado cannot serve and whose
    /// operator cannot name the last good version from a record it holds.
    #[arg(long)]
    previous: bool,
}

pub(in crate::cli::release_cmd) async fn restore_local(
    args: &ReleaseRestoreLocalArgs,
) -> Result<(), CmdError> {
    use sha2::Digest as _;

    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let retained = crate::config_file::expand_tilde("~")
        .join(".stado")
        .join("releases")
        .join("stado");
    let chosen = match (&args.version, args.previous) {
        (Some(version), false) => version.trim().to_string(),
        (None, true) => previous_retained(&retained, platform)?,
        _ => {
            return Err(CmdError::usage(
                "release local restore takes exactly one of --version VERSION or --previous",
            ))
        }
    };
    let version = chosen.as_str();
    if !crate::deploy::host_release::is_exact_semver(version) {
        return Err(CmdError::usage(format!(
            "release local restore: {version:?} is not an exact semantic version"
        )));
    }

    let archive = retained
        .join(version)
        .join(platform)
        .join(crate::deploy::host_release::READER_ARCHIVE_NAME);
    if !std::fs::symlink_metadata(&archive).is_ok_and(|metadata| metadata.file_type().is_file()) {
        // The retained versions are listed for the operator; a directory or
        // entry that cannot be read is named rather than reported as none.
        let available = match std::fs::read_dir(&retained) {
            Ok(entries) => {
                let mut versions: Vec<String> = Vec::new();
                let mut unreadable: Vec<String> = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            if entry
                                .path()
                                .join(platform)
                                .join(crate::deploy::host_release::READER_ARCHIVE_NAME)
                                .is_file()
                            {
                                versions.push(entry.file_name().to_string_lossy().into_owned());
                            }
                        }
                        Err(error) => unreadable.push(error.to_string()),
                    }
                }
                versions.sort();
                let listed = if versions.is_empty() {
                    "none".to_string()
                } else {
                    versions.join(", ")
                };
                if unreadable.is_empty() {
                    listed
                } else {
                    format!(
                        "{listed}; entries of {} could not be read: {}",
                        retained.display(),
                        unreadable.join("; ")
                    )
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "none".to_string(),
            Err(error) => format!(
                "unknown, {} could not be listed: {error}",
                retained.display()
            ),
        };
        return Err(CmdError::click(format!(
            "release local restore: this host retains no Stado {version} archive at {}; \
             retained versions for {platform}: {available}",
            archive.display()
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let bytes = std::fs::read(&archive).map_err(|error| {
        CmdError::click(format!(
            "release local restore: cannot read retained archive {}: {error}",
            archive.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    // Retained archives keep the layout their delivery used, which is not
    // always `bin/stado`: the host release path retains the published archive
    // whole. The member is the one regular file named `stado`.
    let mut members = Vec::new();
    let mut bundle = tar::Archive::new(flate2::read::GzDecoder::new(std::io::Cursor::new(&bytes)));
    for entry in bundle.entries().map_err(|error| {
        CmdError::click(format!(
            "release local restore: unreadable retained archive: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })? {
        let entry = entry.map_err(|error| {
            CmdError::click(format!(
                "release local restore: unreadable archive entry: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| {
                CmdError::click(format!(
                    "release local restore: unreadable archive path: {error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?
            .to_string_lossy()
            .into_owned();
        if path.rsplit('/').next() == Some("stado") {
            members.push(path);
        }
    }
    drop(bundle);
    drop(bytes);
    let [member] = members.as_slice() else {
        return Err(CmdError::click(format!(
            "release local restore: {} must carry exactly one regular file named stado, \
             found {members:?}",
            archive.display()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    };
    println!(
        "release local restore: reinstalling Stado {version} member {member} from {} (sha256 {digest})",
        archive.display()
    );
    install_archive(
        "stado".to_string(),
        member,
        &archive.to_string_lossy(),
        &digest,
        Some(version.to_string()),
        false,
    )
    .await
}

/// The version `--previous` restores: the retained version under
/// `retained/<version>/<platform>/` whose staged `stado` is byte for byte
/// `~/.stado/bin/stado.previous`, the binary install moved aside when it
/// installed the current one. Refused, naming what was read, when that
/// rollback target is absent or unreadable, or no retained version stages it.
fn previous_retained(retained: &std::path::Path, platform: &str) -> Result<String, CmdError> {
    let target = crate::config_file::expand_tilde("~")
        .join(".stado")
        .join("bin")
        .join("stado.previous");
    let held = std::fs::read(&target).map_err(|error| {
        CmdError::click(format!(
            "release local restore --previous: the rollback target {} cannot be read: {error}; name the version with --version",
            target.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let entries = std::fs::read_dir(retained).map_err(|error| {
        CmdError::click(format!(
            "release local restore --previous: {} cannot be listed: {error}",
            retained.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            CmdError::click(format!(
                "release local restore --previous: an entry of {} cannot be read: {error}",
                retained.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        let copy = entry.path().join(platform).join("stado");
        match std::fs::read(&copy) {
            Ok(staged) if staged == held => {
                return Ok(entry.file_name().to_string_lossy().into_owned())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CmdError::click(format!(
                    "release local restore --previous: {} cannot be read: {error}",
                    copy.display()
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind())))
            }
        }
    }
    Err(CmdError::click(format!(
        "release local restore --previous: no {platform} release under {} stages the rollback target {}; name the version with --version",
        retained.display(),
        target.display()
    ))
    .stating(crate::primitives::failure::FailureCode::NotFound))
}
