//! `service file sync`.

use super::*;
use nix::sys::stat::Mode;

/// Where the bytes a service file receives come from.
enum FileSource<'a> {
    /// An absolute regular file on this operator host, owner-only unless the
    /// target is executable.
    File(&'a str),
    /// Standard input (`--source-file -`, the Unix convention), so content
    /// such as a secret never lands on a disk of this host on its way.
    Stdin,
}

impl<'a> FileSource<'a> {
    fn named(source_file: &'a str) -> Self {
        if source_file == "-" {
            Self::Stdin
        } else {
            Self::File(source_file)
        }
    }
}

pub(crate) struct FileSyncOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) source_file: &'a str,
    pub(crate) target_file: &'a str,
    pub(crate) executable: bool,
    pub(crate) as_json: bool,
}

/// The target's mode: read and write for its owner alone, plus execute for
/// an executable (`S_IRUSR | S_IWUSR` and `S_IRWXU`, sys/stat.h).
fn target_mode(executable: bool) -> u32 {
    let mode = if executable {
        Mode::S_IRWXU
    } else {
        Mode::S_IRUSR | Mode::S_IWUSR
    };
    crate::primitives::file_mode::widen(mode.bits())
}

/// The source's bytes and the words a refusal names it with.
fn source_content(
    source: &FileSource<'_>,
    executable: bool,
) -> Result<(Vec<u8>, String), CmdError> {
    match source {
        FileSource::Stdin => {
            use std::io::{IsTerminal, Read};
            let mut input = std::io::stdin();
            if input.is_terminal() {
                return Err(CmdError::usage(
                    "--source-file - reads the file's content from a pipe or redirect, and standard input is a terminal",
                ));
            }
            let mut content = Vec::new();
            input.read_to_end(&mut content).map_err(|error| {
                CmdError::click(format!("cannot read standard input: {error}"))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
            Ok((content, String::from("standard input")))
        }
        FileSource::File(source_file) => {
            let source = std::path::Path::new(source_file);
            if !source.is_absolute() {
                return Err(CmdError::usage("--source-file must be absolute"));
            }
            let metadata = std::fs::symlink_metadata(source).map_err(|error| {
                CmdError::click(format!("cannot read {source_file}: {error}"))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(CmdError::usage(format!(
                    "{source_file} must be a regular file, not a symlink"
                )));
            }
            // The permission bits of st_mode; the file type bits above them
            // are no permission and are dropped.
            let permissions =
                Mode::from_bits_truncate(metadata.permissions().mode() as nix::libc::mode_t);
            if !executable && permissions.intersects(Mode::S_IRWXG | Mode::S_IRWXO) {
                return Err(CmdError::usage(format!(
                    "{source_file} must be owner-only unless --executable is set"
                )));
            }
            let content = std::fs::read(source).map_err(|error| {
                CmdError::click(format!("cannot read {source_file}: {error}"))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
            Ok((content, (*source_file).to_owned()))
        }
    }
}

pub(crate) async fn file_sync(options: FileSyncOptions<'_>) -> Result<(), CmdError> {
    let FileSyncOptions {
        name,
        host,
        source_file,
        target_file,
        executable,
        as_json,
    } = options;
    let (content, named) = source_content(&FileSource::named(source_file), executable)?;
    if content.is_empty() {
        return Err(CmdError::usage(format!("{named} is empty")));
    }
    let mode = target_mode(executable);

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = crate::cli::canonical_host(&declared.host).await?;
        let synced = service::sync_service_file(&target, target_file, &content, mode, &runner)
            .await
            .map_err(click)?;
        if !synced.succeeded("file_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            reported_cell(&synced.status),
            reported_cell(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "target_file": target_file,
            "mode": format!("{mode:04o}"),
            "sync": synced.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "file sync")
}
