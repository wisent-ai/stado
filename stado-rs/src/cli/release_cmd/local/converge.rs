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
