//! `stado product tree-archive --source DIR --output FILE.tar.gz`: one
//! directory, with its own name as the archive's root, as a reproducible
//! gzip'd tar. byk-ios, jeden-ios, oko-ios and wisent-ios each kept the same
//! `release/archive-tree.py` to stage their `.xcarchive` this way.
//!
//! Entries are in path order with no owner, time zero and file modes reduced
//! to 0755 (owner-executable) or 0644; directories are 0755 and symbolic
//! links are kept as links, so a framework's `Versions/Current` survives.
//! The gzip header carries time zero too, so one tree packs to one digest.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::{Compression, GzBuilder};

const EXECUTABLE: u32 = 0o755;
const READABLE: u32 = 0o644;
const OWNER_EXECUTE: u32 = 0o100;
const TIME_ZERO: u64 = 0;
const NO_OWNER: u64 = 0;

fn walk(root: &Path, directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        out.push(path.strip_prefix(root)?.to_path_buf());
        if entry.file_type()?.is_dir() {
            walk(root, &path, out)?;
        }
    }
    Ok(())
}

fn header(kind: tar::EntryType, mode: u32, size: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_mode(mode);
    header.set_size(size);
    header.set_uid(NO_OWNER);
    header.set_gid(NO_OWNER);
    header.set_mtime(TIME_ZERO);
    header
}

pub fn run(source: &Path, output: &Path) -> Result<i32> {
    let source = source
        .canonicalize()
        .with_context(|| format!("{} is missing", source.display()))?;
    if !source.is_dir() {
        bail!("{} is not a directory", source.display());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .context("the source directory has no UTF-8 name")?
        .to_string();
    let mut relative = Vec::new();
    walk(&source, &source, &mut relative)?;
    let mut entries: Vec<(String, PathBuf)> = relative
        .into_iter()
        .map(|path| {
            let posix = path
                .to_str()
                .map(str::to_string)
                .with_context(|| format!("{} is not UTF-8", path.display()))?;
            Ok((posix, path))
        })
        .collect::<Result<_>>()?;
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file =
        fs::File::create(output).with_context(|| format!("creating {}", output.display()))?;
    let gzip = GzBuilder::new()
        .mtime(TIME_ZERO as u32)
        .write(file, Compression::default());
    let mut archive = tar::Builder::new(gzip);
    archive.follow_symlinks(false);
    let mut root = header(tar::EntryType::Directory, EXECUTABLE, 0);
    archive.append_data(&mut root, format!("{name}/"), std::io::empty())?;
    for (posix, path) in &entries {
        let full = source.join(path);
        let archived = format!("{name}/{posix}");
        let metadata = fs::symlink_metadata(&full)?;
        let kind = metadata.file_type();
        if kind.is_symlink() {
            let target = fs::read_link(&full)?;
            let mut link = header(tar::EntryType::Symlink, EXECUTABLE, 0);
            archive
                .append_link(&mut link, &archived, &target)
                .with_context(|| format!("adding the link {archived}"))?;
        } else if kind.is_dir() {
            let mut directory = header(tar::EntryType::Directory, EXECUTABLE, 0);
            archive.append_data(&mut directory, format!("{archived}/"), std::io::empty())?;
        } else if kind.is_file() {
            let mode = if metadata.permissions().mode() & OWNER_EXECUTE != 0 {
                EXECUTABLE
            } else {
                READABLE
            };
            let mut entry = header(tar::EntryType::Regular, mode, metadata.len());
            let reader = fs::File::open(&full)?;
            archive
                .append_data(&mut entry, &archived, reader)
                .with_context(|| format!("adding {archived}"))?;
        } else {
            bail!(
                "{} is neither a file, a directory nor a link",
                full.display()
            );
        }
    }
    let mut gzip = archive.into_inner()?;
    gzip.flush()?;
    gzip.finish()?;
    println!("archived {} -> {}", source.display(), output.display());
    Ok(0)
}
