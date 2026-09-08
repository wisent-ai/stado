//! The artifact the builder leaves behind: the staged tree it packages, and
//! the receipt that stands for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use flate2::{Compression, GzBuilder};

use crate::cli::CmdError;
use crate::release_pipeline::BuildReceipt;

fn collect(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let path = root.join(relative);
    // The recipe's stage map is a declaration about what the build produces, and
    // this is where the two are compared. A bare `?` here reported only
    // `No such file or directory (os error 2)`, so a stage entry whose producer
    // had been deleted looked identical to a broken builder.
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "staged path {} declared by the recipe is not there: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "staged path is a symlink: {}",
            path.display()
        )));
    }
    if metadata.is_file() {
        out.push(relative.into());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(CmdError::click("staged path is not regular"));
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        collect(root, &relative.join(entry.file_name()), out)?
    }
    Ok(())
}
pub(super) fn package(
    source: &Path,
    stage: &BTreeMap<String, String>,
) -> Result<Vec<u8>, CmdError> {
    let gz = GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), Compression::best());
    let mut archive = tar::Builder::new(gz);
    for (from, to) in stage {
        let base = Path::new(from);
        let mut paths = Vec::new();
        collect(source, base, &mut paths)?;
        for path in paths {
            let suffix = path.strip_prefix(base).unwrap_or(Path::new(""));
            let destination = if suffix.as_os_str().is_empty() {
                PathBuf::from(to)
            } else {
                Path::new(to).join(suffix)
            };
            let bytes = std::fs::read(source.join(&path))?;
            let metadata = std::fs::metadata(source.join(&path))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                header.set_mode(metadata.permissions().mode() & 0o777);
            }
            #[cfg(not(unix))]
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, destination, bytes.as_slice())?
        }
    }
    Ok(archive.into_inner()?.finish()?)
}
pub(super) fn write_receipt(receipt: &BuildReceipt) -> Result<(), CmdError> {
    std::fs::create_dir_all("output")?;
    std::fs::write("output/receipt.json", serde_json::to_vec(receipt)?)?;
    Ok(())
}
