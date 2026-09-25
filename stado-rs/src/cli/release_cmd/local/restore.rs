//! `stado release restore-local` — reinstall a Stado release an earlier
//! delivery retained on this very host.
//!
//! A delivery keeps each Stado archive it installed under
//! `$HOME/.stado/releases/stado/<version>/<platform>/`. When the installed
//! Stado cannot serve — its rules refuse this host's configuration and every
//! boundary of the object API answers `503 object authorization unavailable`
//! — the registry, the release records and `release rollback` are behind that
//! same closed API, and `release host-state --apply` refuses to downgrade a
//! host that runs a newer version than it declares. This command needs none of
//! them: it reads the retained archive from local disk and installs it through
//! the same checked install and reader convergence as `install-local`, so the
//! restored binary validates this host's configuration before it replaces
//! anything. Run it on the host with `stado host run-attached`.

use clap::Args;

use crate::cli::CmdError;

use super::install::install_archive;

#[derive(Args)]
pub struct ReleaseRestoreLocalArgs {
    /// Exact retained Stado version to reinstall.
    #[arg(long)]
    version: String,
}

pub(in crate::cli::release_cmd) async fn restore_local(
    args: &ReleaseRestoreLocalArgs,
) -> Result<(), CmdError> {
    use sha2::Digest as _;

    let version = args.version.trim();
    if !crate::deploy::host_release::is_exact_semver(version) {
        return Err(CmdError::click(format!(
            "release restore-local: {version:?} is not an exact semantic version"
        )));
    }
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::click(error.to_string()))?;
    let retained = crate::config_file::expand_tilde("~")
        .join(".stado")
        .join("releases")
        .join("stado");
    let archive = retained
        .join(version)
        .join(platform)
        .join(crate::deploy::host_release::READER_ARCHIVE_NAME);
    if !std::fs::symlink_metadata(&archive).is_ok_and(|metadata| metadata.file_type().is_file()) {
        let mut available: Vec<String> = std::fs::read_dir(&retained)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .path()
                            .join(platform)
                            .join(crate::deploy::host_release::READER_ARCHIVE_NAME)
                            .is_file()
                    })
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        available.sort();
        return Err(CmdError::click(format!(
            "release restore-local: this host retains no Stado {version} archive at {}; \
             retained versions for {platform}: {}",
            archive.display(),
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        )));
    }
    let bytes = std::fs::read(&archive).map_err(|error| {
        CmdError::click(format!(
            "release restore-local: cannot read retained archive {}: {error}",
            archive.display()
        ))
    })?;
    let digest = hex::encode(sha2::Sha256::digest(&bytes));
    drop(bytes);
    println!(
        "release restore-local: reinstalling Stado {version} from {} (sha256 {digest})",
        archive.display()
    );
    install_archive(
        "stado".to_string(),
        "bin/stado",
        &archive.to_string_lossy(),
        &digest,
        Some(version.to_string()),
        false,
    )
    .await
}
