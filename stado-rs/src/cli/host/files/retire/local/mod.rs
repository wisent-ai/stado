//! The device-local filesystem half of `space file retire`.

pub(in crate::cli::host) mod commit;

use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use crate::cli::CmdError;

use crate::cli::host::files::retire::fs::dirs::{
    open_directory_at, open_or_create_directory_at, open_source_at,
};
use crate::cli::host::files::retire::fs::{hash_open_file, open_home_directory};
use crate::cli::host::files::retire::{
    retire_file_binding, retire_refused, safe_backup_product, RetireFileBinding, RetireFileOutcome,
    RetireFileRequest,
};

/// Run the device-local filesystem half of `space file retire`.
///
/// Every path component is opened with `O_NOFOLLOW`, held by descriptor through
/// the mutation, and checked against the approved account uid. The source is
/// hashed through an open descriptor; the kernel rename is no-replace and
/// therefore cannot copy on `EXDEV` or overwrite a collision. The destination
/// must resolve to the same inode, size, mode, and digest. Any mismatch triggers
/// an atomic no-replace rollback before the command returns an error.
pub(super) fn retire_file_local_document(
    request: &RetireFileRequest<'_>,
    binding: Option<&RetireFileBinding>,
) -> Result<RetireFileOutcome, CmdError> {
    let RetireFileRequest {
        path,
        product,
        dry_run,
        ..
    } = *request;
    if !path.starts_with('/')
        || path.split('/').any(|component| component == "..")
        || path.chars().any(char::is_control)
    {
        return Err(CmdError::usage(
            "path must be absolute, contain no '..' component, and carry no control character",
        ));
    }
    if !safe_backup_product(product) {
        return Err(CmdError::usage(
            "product must be 1-128 ASCII letters, digits, dots, underscores, or dashes and start with a letter or digit",
        ));
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| retire_refused("HOME is not an absolute path"))?;
    let source = PathBuf::from(path);
    let approved = [
        (OsStr::new(".stado"), home.join(".stado/bin")),
        (OsStr::new(".local"), home.join(".local/bin")),
        (OsStr::new(".cargo"), home.join(".cargo/bin")),
    ];
    let (source_scope, _) = approved
        .iter()
        .find(|(_, root)| source.parent() == Some(root.as_path()))
        .ok_or_else(|| {
            retire_refused("source is not a direct child of an approved user bin root")
        })?;
    let source_name = source
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| retire_refused("source has no basename"))?;
    let uid = unsafe { nix::libc::geteuid() };
    let home_directory = open_home_directory(&home, uid)?;
    let Some(scope_directory) = open_directory_at(home_directory.as_raw_fd(), source_scope, uid)?
    else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("approved source root does not exist".to_string()),
        });
    };
    let Some(source_directory) =
        open_directory_at(scope_directory.as_raw_fd(), OsStr::new("bin"), uid)?
    else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("approved source root does not exist".to_string()),
        });
    };
    let Some(mut source_file) = open_source_at(source_directory.as_raw_fd(), source_name)? else {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("source does not exist".to_string()),
        });
    };
    let source_metadata = source_file
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect source: {error}")))?;
    if !source_metadata.is_file() {
        return Err(retire_refused("source is not a regular file"));
    }
    if source_metadata.uid() != uid {
        return Err(retire_refused(
            "source is not owned by the approved account",
        ));
    }
    let size = source_metadata.len();
    let mode = format!("{:04o}", source_metadata.mode() & 0o7777);
    let sha256 = hash_open_file(&mut source_file)?;
    if let Some(binding) = binding {
        if size != binding.expected_size {
            return Err(retire_refused(
                "source size differs from the reviewed dry-run receipt",
            ));
        }
        if mode != binding.expected_mode {
            return Err(retire_refused(
                "source mode differs from the reviewed dry-run receipt",
            ));
        }
        if sha256 != binding.expected_sha256 {
            return Err(retire_refused(
                "source SHA-256 differs from the reviewed dry-run receipt",
            ));
        }
    }
    let transaction = binding
        .map(|binding| binding.transaction.clone())
        .unwrap_or_else(|| {
            format!(
                "{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
                uuid::Uuid::new_v4().simple()
            )
        });

    let destination_parts = [
        OsStr::new(".stado"),
        OsStr::new("products"),
        OsStr::new(product),
        OsStr::new("backups"),
    ];
    let mut destination_directories = Vec::<File>::with_capacity(destination_parts.len());
    let mut destination_parent_fd = home_directory.as_raw_fd();
    let mut destination_device = home_directory
        .metadata()
        .map_err(|error| retire_refused(format!("cannot inspect HOME: {error}")))?
        .dev();
    let mut missing_ancestor = false;
    for component in destination_parts {
        if missing_ancestor {
            continue;
        }
        match open_or_create_directory_at(destination_parent_fd, component, uid, !dry_run)? {
            Some(directory) => {
                destination_device = directory
                    .metadata()
                    .map_err(|error| {
                        retire_refused(format!("cannot inspect backup ancestor: {error}"))
                    })?
                    .dev();
                destination_directories.push(directory);
                destination_parent_fd = destination_directories
                    .last()
                    .expect("just pushed destination directory")
                    .as_raw_fd();
            }
            None => missing_ancestor = true,
        }
    }
    if source_metadata.dev() != destination_device {
        return Err(retire_refused(
            "source and backup tree are not on one filesystem, so an atomic move is impossible",
        ));
    }

    let destination = home
        .join(".stado/products")
        .join(product)
        .join("backups")
        .join(&transaction)
        .join(source_name);
    if dry_run {
        return Ok(RetireFileOutcome {
            target: String::new(),
            source: path.to_string(),
            destination: Some(destination.to_string_lossy().into_owned()),
            transaction: Some(transaction.clone()),
            status: "ready".to_string(),
            size: Some(size),
            sha256: Some(sha256),
            mode: Some(mode),
            detail: None,
        });
    }

    commit::commit_retirement(
        path,
        &source_metadata,
        &destination_directories,
        &source_directory,
        source_name,
        transaction,
        &destination,
        size,
        sha256,
        mode,
        uid,
    )
}

/// Hidden device-local endpoint used by the public target-resolving command.
pub fn retire_file_local(
    request: RetireFileRequest<'_>,
    json_output: bool,
) -> Result<(), CmdError> {
    let binding = retire_file_binding(&request)?;
    let outcome = retire_file_local_document(&request, binding.as_ref())?;
    if json_output {
        println!("{}", serde_json::to_string(&outcome)?);
    } else {
        println!(
            "{} {} -> {}",
            outcome.status,
            outcome.source,
            outcome.destination.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}
