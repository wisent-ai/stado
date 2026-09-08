//! `stado storage archive`: one directory as a deterministic .tar.gz.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageArchiveArgs {
    /// Directory whose contents become the archive root.
    source: String,
    /// New .tar.gz output path. Refuses to overwrite.
    output: String,
    #[arg(long)]
    json: bool,
}

const ARCHIVE_MAX_ENTRIES: usize = 1_000_000;
const ARCHIVE_MAX_PATH_BYTES: usize = 4 * 1024;
const ARCHIVE_MAX_MEMBER_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const ARCHIVE_MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024 * 1024;

fn sorted_archive_paths(
    root: &std::path::Path,
    directory: &std::path::Path,
    paths: &mut Vec<std::path::PathBuf>,
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
        if paths.len() >= ARCHIVE_MAX_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive source exceeds the one-million-entry limit",
            ));
        }
        if relative.as_os_str().as_encoded_bytes().len() > ARCHIVE_MAX_PATH_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("archive member path exceeds 4096 bytes: {}", path.display()),
            ));
        }
        paths.push(path.clone());
        if metadata.is_dir() {
            sorted_archive_paths(root, &path, paths)?;
        }
    }
    Ok(())
}

pub(in crate::cli::storage) fn archive(args: &StorageArchiveArgs) -> Result<(), CmdError> {
    let source = std::path::Path::new(&args.source);
    let metadata = std::fs::symlink_metadata(source)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "archive source must be a real directory: {}",
            source.display()
        )));
    }
    let source = source.canonicalize()?;
    let output = std::path::Path::new(&args.output);
    if output.try_exists()? {
        return Err(CmdError::click(format!(
            "refusing to overwrite archive {}",
            output.display()
        )));
    }
    let output_parent = output
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .canonicalize()?;
    if output_parent.starts_with(&source) {
        return Err(CmdError::click(
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
        sorted_archive_paths(&source, &source, &mut paths)?;
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
                if member_bytes > ARCHIVE_MAX_MEMBER_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("archive member exceeds the 8 GiB limit: {}", path.display()),
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
                if total_member_bytes > ARCHIVE_MAX_TOTAL_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "archive source exceeds the 32 GiB uncompressed limit",
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
        )));
    }
    let mut file = std::fs::File::open(output)?;
    let mut hasher = Sha256::new();
    let mut buffer = [u8::MIN; u16::MAX as usize];
    loop {
        let read = file.read(&mut buffer)?;
        if read == usize::default() {
            break;
        }
        hasher.update(&buffer[..read]);
    }
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
            }))?
        );
    } else {
        println!("{} bytes sha256={} {}", bytes, sha256, output.display());
    }
    Ok(())
}
