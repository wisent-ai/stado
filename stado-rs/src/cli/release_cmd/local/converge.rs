//! `stado release converge-local-readers` — reconcile the live readers of a
//! native binary that is already installed on this host.

use crate::cli::CmdError;

use super::{converge_service_local_stado_readers, ReleaseConvergeLocalReadersArgs};

/// Reconcile every live reader of one already-installed native binary.
pub(in crate::cli::release_cmd) async fn converge_local_readers(
    args: &ReleaseConvergeLocalReadersArgs,
) -> Result<(), CmdError> {
    use sha2::Digest as _;
    use std::io::Read as _;

    if args.name != "stado" {
        return Err(CmdError::click(
            "release converge-local-readers only supports the Stado product",
        ));
    }
    if !crate::deploy::host_release::is_sha256(&args.sha256) {
        return Err(CmdError::click(
            "release converge-local-readers requires a lowercase SHA-256",
        ));
    }
    let mut archive = std::fs::File::open(&args.archive).map_err(|error| {
        CmdError::click(format!(
            "release converge-local-readers: cannot open verified archive {}: {error}",
            args.archive.display()
        ))
    })?;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = archive.read(&mut buffer).map_err(|error| {
            CmdError::click(format!(
                "release converge-local-readers: cannot read verified archive {}: {error}",
                args.archive.display()
            ))
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual = hex::encode(digest.finalize());
    if actual != args.sha256 {
        return Err(CmdError::click(format!(
            "release converge-local-readers: archive digest mismatch: expected {}, got {actual}",
            args.sha256
        )));
    }

    let directory = crate::config_file::expand_tilde("~").join(".stado/bin");
    let executable = directory.join(&args.name);
    let mut log = |message: &str| println!("{message}");
    // The queue agent recycles itself only when `stado.release-version`
    // names a version other than the one it was compiled from, and only
    // `release install-local` wrote that file. A `stado product install`
    // left it at 0.22.0 under a 0.22.5 binary on 2026-09-26, so the object
    // API that carries the agent stayed on the replaced image indefinitely.
    // This command is the installed binary when it runs from the install, so
    // its own version is the installed one.
    let running = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| {
            CmdError::click(format!(
                "release converge-local-readers: cannot resolve its own executable: {error}"
            ))
        })?;
    if std::fs::canonicalize(&executable).ok().as_deref() == Some(running.as_path()) {
        let marker = directory.join("stado.release-version");
        let staged = directory.join("stado.release-version.release-incoming");
        std::fs::write(&staged, format!("{}\n", env!("CARGO_PKG_VERSION")))
            .and_then(|()| std::fs::rename(&staged, &marker))
            .map_err(|error| {
                CmdError::click(format!(
                    "release converge-local-readers: cannot record the installed Stado release \
                     coordinate at {}: {error}",
                    marker.display()
                ))
            })?;
        log(&format!(
            "release converge-local-readers: {} names {}, so queue agents on an older image \
             recycle themselves after their active jobs",
            marker.display(),
            env!("CARGO_PKG_VERSION")
        ));
    }
    crate::self_update::recycle_replaced_units(
        "release converge-local-readers",
        &directory,
        std::slice::from_ref(&args.name),
        &mut log,
    )
    .await
    .map_err(CmdError::click)?;
    converge_service_local_stado_readers(
        "release converge-local-readers",
        &executable,
        args.archive.as_path(),
    )
    .await
}
