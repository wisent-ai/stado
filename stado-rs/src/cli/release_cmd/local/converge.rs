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
        return Err(CmdError::refused(
            "release converge-local-readers only supports the Stado product",
        ));
    }
    if let (Some(archive_path), Some(expected)) = (&args.archive, &args.sha256) {
        if !crate::deploy::host_release::is_sha256(expected) {
            return Err(CmdError::usage(
                "release converge-local-readers requires a lowercase SHA-256",
            ));
        }
        let mut archive = std::fs::File::open(archive_path).map_err(|error| {
            CmdError::click(format!(
                "release converge-local-readers: cannot open verified archive {}: {error}",
                archive_path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        let mut digest = sha2::Sha256::new();
        let mut buffer = [0_u8; 1024 * 1024];
        loop {
            let count = archive.read(&mut buffer).map_err(|error| {
                CmdError::click(format!(
                    "release converge-local-readers: cannot read verified archive {}: {error}",
                    archive_path.display()
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        let actual = hex::encode(digest.finalize());
        if &actual != expected {
            return Err(CmdError::click(format!(
                "release converge-local-readers: archive digest mismatch: expected {expected}, got {actual}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
    }

    // Every reader this command recycles is a `stado serve` process, whose
    // API listener refuses to start without the host's declared request
    // limits. Recycling them onto this image on a host that declares none
    // ended a serving object API for one that crash-looped on 'API request
    // limits are not declared' (d6b3c3ce). So the image's own requirement is
    // read first, before the release-version marker that makes queue agents
    // recycle themselves: a host that cannot run it keeps its running units
    // on the replaced image, and the install fails naming the declaration.
    crate::dashboard::RequestLimits::read().map_err(|error| {
        CmdError::refused(format!(
            "release converge-local-readers: this host's configuration cannot run the installed \
             Stado's API, so no unit is recycled onto it and the running units keep the image \
             they started with: {error}"
        ))
    })?;

    let directory = crate::config_file::expand_tilde("~").join(".stado/bin");
    let executable = directory.join(&args.name);
    let mut log = |message: &str| println!("{message}");
    // The queue agent recycles itself only when `stado.release-version`
    // names a version other than the one it was compiled from, and only
    // `release install-local` wrote that file. A `stado product install`
    // leaves it at the previous version under a newer binary, so the object
    // API that carries the agent stays on the replaced image indefinitely.
    // This command is the installed binary when it runs from the install, so
    // its own version is the installed one.
    let running = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| {
            CmdError::click(format!(
                "release converge-local-readers: cannot resolve its own executable: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
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
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        log(&format!(
            "release converge-local-readers: {} names {}, so queue agents on an older image \
             recycle themselves after their active jobs",
            marker.display(),
            env!("CARGO_PKG_VERSION")
        ));
    }
    let in_flight = crate::self_update::ReplacementInFlight::begin(
        &directory,
        std::slice::from_ref(&args.name),
        &mut log,
    );
    crate::self_update::recycle_replaced_units(
        "release converge-local-readers",
        &directory,
        std::slice::from_ref(&args.name),
        &mut log,
    )
    .await
    // Every way the recycle fails is the host's service manager or process
    // table refusing to restart a reader: the host's outage.
    .map_err(CmdError::unreachable)?;
    drop(in_flight);
    let Some(archive) = args.archive.as_deref() else {
        log(
            "release converge-local-readers: no release archive was given (a source install), \
             so service-local Stado readers keep their installed release until a release \
             install hands them one",
        );
        return Ok(());
    };
    converge_service_local_stado_readers("release converge-local-readers", &executable, archive)
        .await
}
