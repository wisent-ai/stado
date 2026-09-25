use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

pub fn platform() -> Result<String> {
    let operating_system = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => bail!("unsupported release operating system {other}"),
    };
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => bail!("unsupported release architecture {other}"),
    };
    Ok(format!("{operating_system}-{architecture}"))
}

/// `path` itself, once it is known to stay beneath whatever root it is joined to.
pub fn relative(path: &Path) -> Result<&Path> {
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!(
            "path must stay beneath its declared root: {}",
            path.display()
        );
    }
    Ok(path)
}

pub fn unpack(path: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    if destination.symlink_metadata()?.file_type().is_symlink() {
        bail!("archive destination cannot be a symlink");
    }
    let root = destination.canonicalize()?;
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    let stream: Box<dyn Read> = if magic == [0x1f, 0x8b] {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut archive = tar::Archive::new(stream);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let member = entry.path()?.into_owned();
        relative(&member)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
            bail!("archive contains a special file: {}", member.display());
        }
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()?
                .context("archive link has no target")?
                .into_owned();
            let mut components: Vec<_> = if kind.is_symlink() {
                member
                    .parent()
                    .unwrap_or(Path::new(""))
                    .components()
                    .filter(|c| *c != Component::CurDir)
                    .collect()
            } else {
                Vec::new()
            };
            for component in target.components() {
                match component {
                    Component::Normal(_) => components.push(component),
                    Component::CurDir => {}
                    Component::ParentDir if !components.is_empty() => {
                        components.pop();
                    }
                    _ => bail!(
                        "archive link escapes its root: {} -> {}",
                        member.display(),
                        target.display()
                    ),
                }
            }
        }
        if !entry.unpack_in(&root)? {
            bail!("archive member was refused: {}", member.display());
        }
        let output = root.join(&member);
        if output.exists() && !output.canonicalize()?.starts_with(&root) {
            bail!("unpacked member escapes root: {}", member.display());
        }
        #[cfg(unix)]
        if kind.is_file() {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode()? & 0o755;
            fs::set_permissions(output, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

pub fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    let metadata = source.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
            return Ok(());
        }
        #[cfg(not(unix))]
        bail!("symbolic-link copying is unsupported on this platform");
    }
    if metadata.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, metadata.permissions())?;
    } else if metadata.is_file() {
        fs::copy(source, destination)?;
    } else {
        bail!("refusing to copy special file {}", source.display());
    }
    Ok(())
}

pub fn file_members(root: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            result.extend(file_members(&entry.path())?);
        } else {
            result.push(entry.path());
        }
    }
    result.sort();
    Ok(result)
}
