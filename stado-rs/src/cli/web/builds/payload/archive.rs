//! The reproducible tarball: a header with everything about the builder
//! erased, one top-level directory, and the digest of the bytes on disk.

use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::cli::web::builds::contract::package::kind::Kind;
use crate::cli::web::builds::payload::launcher::{launcher, server_launcher};
use crate::cli::web::builds::payload::members::{members, static_members};
use crate::cli::web::builds::payload::static_server::static_server;
use crate::cli::web::builds::payload::{STATIC_SERVER, STATIC_SITE_DIR};
use crate::cli::web::LAUNCHER;
use crate::cli::CmdError;

/// A file's mode as the artifact records it: nothing of the builder's umask
/// survives, only whether the file is executable.
#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o100 == 0o100 {
        0o755
    } else {
        0o644
    }
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0o644
}

/// A header with everything about the builder erased: no owner, no owner name,
/// no modification time. Two builders with different user ids produce the same
/// bytes.
fn header(entry_type: tar::EntryType, mode: u32, size: u64) -> Result<tar::Header, CmdError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_size(size);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_username("")?;
    header.set_groupname("")?;
    Ok(header)
}

/// Write the runnable tarball: one top-level directory, the site or the build
/// output, and the generated launcher.
///
/// A served product's members are relative to the checkout and keep their
/// paths. A static site's members are relative to its own root and land under
/// one fixed directory, so the launcher names `site` whatever the recipe
/// called it on the builder.
pub(in crate::cli::web::builds) fn stage(
    source: &Path,
    site: &Path,
    kind: Kind,
    tarball: &Path,
    root: &str,
) -> Result<(), CmdError> {
    let (members, base, inner) = match kind {
        Kind::Server => (members(source)?, source, None),
        Kind::Static => (static_members(site)?, site, Some(STATIC_SITE_DIR)),
    };
    let file = std::fs::File::create(tarball).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", tarball.display()))
    })?;
    // gzip's own header carries a modification time, and it is set to zero for
    // the same reason every entry's is: the artifact's bytes must depend on the
    // commit and nothing else. No file name is stored either, since that would
    // record the builder's path.
    let encoder = flate2::GzBuilder::new().mtime(0).write(
        std::io::BufWriter::new(file),
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(encoder);
    archive.follow_symlinks(false);

    let top = PathBuf::from(root);
    archive.append_data(
        &mut header(tar::EntryType::Directory, 0o755, 0)?,
        &top,
        std::io::empty(),
    )?;
    let prefix = match inner {
        Some(inner) => {
            let directory = top.join(inner);
            archive.append_data(
                &mut header(tar::EntryType::Directory, 0o755, 0)?,
                &directory,
                std::io::empty(),
            )?;
            directory
        }
        None => top.clone(),
    };
    for member in &members {
        let relative = member.strip_prefix(base).map_err(|_| {
            CmdError::click(format!(
                "{} is not inside {}",
                member.display(),
                base.display()
            ))
        })?;
        let name = prefix.join(relative);
        let metadata = std::fs::symlink_metadata(member)?;
        let kind = metadata.file_type();
        if kind.is_symlink() {
            // A symlink travels as a symlink and is never followed.
            // `node_modules/.bin` is a directory of them, and the scripts they
            // point at resolve their own `require` paths relative to where the
            // link's target lives, so a dereferenced copy would start and then
            // fail to find its own package. A link can also point back into
            // node_modules, which is how following one walks forever. The mode
            // is fixed rather than copied because no extractor applies a
            // symlink's mode, and lstat reports a different one on Darwin than
            // on Linux -- which would be enough to make the same commit build
            // to two different tarballs on two builders.
            let target = std::fs::read_link(member)?;
            let mut entry = header(tar::EntryType::Symlink, 0o777, 0)?;
            archive.append_link(&mut entry, &name, &target)?;
        } else if kind.is_dir() {
            archive.append_data(
                &mut header(tar::EntryType::Directory, 0o755, 0)?,
                &name,
                std::io::empty(),
            )?;
        } else if kind.is_file() {
            let mut handle = std::fs::File::open(member).map_err(|error| {
                CmdError::click(format!("cannot read {}: {error}", member.display()))
            })?;
            let mut entry = header(
                tar::EntryType::Regular,
                file_mode(&metadata),
                metadata.len(),
            )?;
            archive.append_data(&mut entry, &name, &mut handle)?;
        } else {
            // A socket or a fifo in the tree is not something tar can carry
            // faithfully, and silently dropping it would produce an artifact
            // whose contents nobody declared.
            return Err(CmdError::click(format!(
                "{} is neither a file, a directory nor a symlink and cannot be staged",
                member.display()
            )));
        }
    }

    // The launcher and, for a static site, the server it runs both live under
    // the archive's own root — never under the site directory, where they
    // would be two files the public could fetch.
    archive.append_data(
        &mut header(tar::EntryType::Directory, 0o755, 0)?,
        top.join("bin"),
        std::io::empty(),
    )?;
    let script = match kind {
        Kind::Server => server_launcher(),
        Kind::Static => launcher(Kind::Static, STATIC_SITE_DIR),
    };
    let mut entry = header(tar::EntryType::Regular, 0o755, script.len() as u64)?;
    archive.append_data(&mut entry, top.join(LAUNCHER), script.as_bytes())?;
    if kind == Kind::Static {
        let server = static_server();
        let mut entry = header(tar::EntryType::Regular, 0o644, server.len() as u64)?;
        archive.append_data(&mut entry, top.join(STATIC_SERVER), server.as_bytes())?;
    }

    let mut writer = archive.into_inner()?.finish()?;
    writer.flush()?;
    Ok(())
}

/// The artifact's sha256, read back from the file that was just written so the
/// digest describes the bytes on disk rather than the bytes we meant to write.
pub(in crate::cli::web::builds) fn digest(tarball: &Path) -> Result<String, CmdError> {
    let mut file = std::fs::File::open(tarball)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", tarball.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}
