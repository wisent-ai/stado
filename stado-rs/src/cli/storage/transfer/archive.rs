//! `stado storage archive`: one directory as a deterministic .tar.gz.

use crate::cli::storage::*;
use crate::config::archive_limits::ArchiveLimits;

#[derive(Args, Debug)]
pub struct StorageArchiveArgs {
    /// Directory whose contents become the archive root.
    source: String,
    /// New .tar.gz output path. Refuses to overwrite.
    output: String,
    #[arg(long)]
    json: bool,
}

fn sorted_archive_paths(
    root: &std::path::Path,
    directory: &std::path::Path,
    paths: &mut Vec<std::path::PathBuf>,
    limits: &ArchiveLimits,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| {
        left.strip_prefix(root)
            .unwrap_or(left)
            .cmp(right.strip_prefix(root).unwrap_or(right))
    });
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "archive source contains unsupported symlink: {}",
                    path.display()
                ),
            ));
        }
        let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
        if paths.len() >= limits.entries.get() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("archive entries exceed storage.archive_limits.entries {}: already collected {}, next member {}", limits.entries, paths.len(), path.display()),
            ));
        }
        let path_bytes = relative.as_os_str().as_encoded_bytes().len();
        if path_bytes > limits.path_bytes.get() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("archive path {} has {path_bytes} bytes, exceeding storage.archive_limits.path_bytes {}", path.display(), limits.path_bytes),
            ));
        }
        paths.push(path.clone());
        if metadata.is_dir() {
            sorted_archive_paths(root, &path, paths, limits)?;
        }
    }
    Ok(())
}

pub(in crate::cli::storage) fn archive(args: &StorageArchiveArgs) -> Result<(), CmdError> {
    let limits = ArchiveLimits::read().map_err(CmdError::declaration)?;
    let source = std::path::Path::new(&args.source);
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CmdError::usage(format!(
            "archive source must be a real directory: {}",
            source.display()
        )));
    }
    let source = source.canonicalize()?;
    let output = std::path::Path::new(&args.output);
    if output.try_exists()? {
        return Err(CmdError::refused(format!(
            "refusing to overwrite archive {}",
            output.display()
        )));
    }
    let output_parent = output
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .canonicalize()?;
    if output_parent.starts_with(&source) {
        return Err(CmdError::usage(
            "archive output must be outside the source directory",
        ));
    }
    let mut output_created = false;
    let create_result = (|| -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        output_created = true;
        let encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(file, flate2::Compression::default());
        let mut archive = tar::Builder::new(encoder);
        archive.mode(tar::HeaderMode::Deterministic);
        archive.follow_symlinks(false);
        let mut paths = Vec::new();
        sorted_archive_paths(&source, &source, &mut paths, &limits)?;
        paths.sort_by(|left, right| {
            left.strip_prefix(&source)
                .unwrap_or(left)
                .cmp(right.strip_prefix(&source).unwrap_or(right))
        });
        let mut total_member_bytes = 0_u64;
        for path in paths {
            let name = path.strip_prefix(&source).map_err(std::io::Error::other)?;
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "archive source contains unsupported symlink: {}",
                        path.display()
                    ),
                ));
            }
            let mut open = std::fs::OpenOptions::new();
            open.read(true);
            #[cfg(target_os = "macos")]
            open.custom_flags(0x0000_0100);
            #[cfg(target_os = "linux")]
            open.custom_flags(0x0002_0000);
            let mut member = open.open(&path)?;
            let opened_metadata = member.metadata()?;
            if opened_metadata.is_dir() && metadata.is_dir() {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_uid(0);
                header.set_gid(0);
                header.set_mtime(0);
                header.set_cksum();
                archive.append_data(&mut header, name, std::io::empty())?;
            } else if opened_metadata.is_file() && metadata.is_file() {
                let member_bytes = opened_metadata.len();
                if member_bytes > limits.member_bytes.get() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("archive member {} has {member_bytes} bytes, exceeding storage.archive_limits.member_bytes {}", path.display(), limits.member_bytes),
                    ));
                }
                total_member_bytes =
                    total_member_bytes
                        .checked_add(member_bytes)
                        .ok_or_else(|| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidInput,
                                "archive member byte total overflowed",
                            )
                        })?;
                if total_member_bytes > limits.total_bytes.get() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("archive source has {total_member_bytes} regular-file bytes, exceeding storage.archive_limits.total_bytes {}", limits.total_bytes),
                    ));
                }
                archive.append_file(name, &mut member)?;
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "archive member changed type or is unsupported: {}",
                        path.display()
                    ),
                ));
            }
        }
        let encoder = archive.into_inner()?;
        let file = encoder.finish()?;
        file.sync_all()?;
        drop(file);
        std::fs::File::open(&output_parent)?.sync_all()
    })();
    if let Err(error) = create_result {
        if output_created {
            let _ = std::fs::remove_file(output);
        }
        return Err(CmdError::click(format!(
            "cannot create release archive {}: {error}",
            output.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind())));
    }
    let mut file = std::fs::File::open(output)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let bytes = file.metadata()?.len();
    let sha256 = hex::encode(hasher.finalize());
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": source,
                "output": output,
                "bytes": bytes,
                "sha256": sha256,
                "packing_limits": limits,
            }))?
        );
    } else {
        println!("{} bytes sha256={} {}", bytes, sha256, output.display());
    }
    Ok(())
}
