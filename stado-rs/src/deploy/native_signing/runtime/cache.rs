use super::{Prepared, Receipt, PRODUCT, VERSION};
use anyhow::{ensure, Context, Result};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

fn owned(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect native SDK cache {}", path.display()))?;
    ensure!(
        !metadata.file_type().is_symlink()
            && metadata.uid() == nix::unistd::geteuid().as_raw()
            && metadata.permissions().mode() & 0o022 == 0,
        "native SDK cache is symlinked, writable by another account, or not owned by this account: {}",
        path.display()
    );
    Ok(metadata)
}

pub(super) fn regular(path: &Path) -> Result<fs::Metadata> {
    let metadata = owned(path)?;
    ensure!(
        metadata.is_file(),
        "native SDK cache is not a regular file: {}",
        path.display()
    );
    Ok(metadata)
}

pub(super) fn root(platform: &str) -> Result<(PathBuf, File)> {
    ensure!(
        matches!(platform, "darwin-arm64" | "linux-amd64"),
        "unsupported native SDK platform {platform}"
    );
    let mut parent = PathBuf::from(std::env::var_os("HOME").context("HOME is unset")?);
    ensure!(
        parent.is_absolute() && owned(&parent)?.is_dir(),
        "native SDK requires an owned absolute HOME directory"
    );
    for component in [".stado", "cache", "product-sdk", VERSION] {
        parent.push(component);
        match fs::symlink_metadata(&parent) {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {
                match fs::DirBuilder::new().mode(0o700).create(&parent) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("create native SDK cache {}", parent.display())
                        })
                    }
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect native SDK cache {}", parent.display()))
            }
        }
        ensure!(
            owned(&parent)?.is_dir(),
            "native SDK cache parent is not a directory: {}",
            parent.display()
        );
    }
    let lock_path = parent.join(format!("{platform}.lock"));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&lock_path)?;
    regular(&lock_path)?;
    lock.try_lock_exclusive()
        .with_context(|| format!("another native SDK operation owns {}", lock_path.display()))?;
    Ok((parent.join(platform), lock))
}

pub(super) fn native(program: &Path, platform: &str) -> Result<()> {
    ensure!(
        regular(program)?.permissions().mode() & 0o111 != 0,
        "native SDK is not executable: {}",
        program.display()
    );
    let mut magic = [0_u8; 4];
    File::open(program)?.read_exact(&mut magic)?;
    let native = match platform {
        "linux-amd64" => magic == *b"\x7fELF",
        "darwin-arm64" => matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
                | [0xca, 0xfe, 0xba, 0xbf]
                | [0xbf, 0xba, 0xfe, 0xca]
        ),
        _ => false,
    };
    ensure!(
        native,
        "qualified SDK entry is not a native {platform} executable: {}",
        program.display()
    );
    Ok(())
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn read(root: &Path, platform: &str) -> Result<Option<Prepared>> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect SDK release {}", root.display()))
        }
        Ok(_) => ensure!(
            owned(root)?.is_dir(),
            "SDK release cache is not a directory: {}",
            root.display()
        ),
    }
    let record = root.join("sdk-receipt.json");
    regular(&record)?;
    let receipt: Receipt = serde_json::from_slice(&fs::read(&record)?)
        .with_context(|| format!("read native SDK provenance {}", record.display()))?;
    ensure!(
        receipt.schema_version == 1 && receipt.version == VERSION && receipt.platform == platform,
        "native SDK cache records another version, platform or schema: {}",
        record.display()
    );
    crate::release_control::CoordinateRevision::new(
        PRODUCT,
        VERSION,
        platform,
        &receipt.artifact.source_revision,
    )
    .map_err(anyhow::Error::msg)?;
    let base = crate::release_control::release_base(PRODUCT, VERSION, platform)
        .map_err(anyhow::Error::msg)?;
    ensure!(
        receipt.artifact.manifest_uri
            == format!("{base}/{}", crate::release_control::RELEASE_MANIFEST_NAME)
            && receipt.artifact.signature_uri
                == format!("{base}/{}", crate::release_control::RELEASE_SIGNATURE_NAME)
            && receipt.artifact.archive_uri
                == format!("{base}/{}", crate::release_control::RELEASE_ARCHIVE_NAME)
            && digest(&receipt.artifact.manifest_sha256)
            && digest(&receipt.artifact.artifact_sha256)
            && !receipt.artifact.key_id.is_empty(),
        "native SDK cache does not record its qualified release coordinate: {}",
        record.display()
    );
    let program = root.join(PRODUCT);
    native(&program, platform)?;
    let (bytes, sha256) =
        crate::release_control::sha256_file(&program).map_err(anyhow::Error::msg)?;
    ensure!(bytes == receipt.executable_bytes && sha256 == receipt.executable_sha256,
        "native SDK cache bytes changed: {}; expected {} bytes sha256 {}, observed {bytes} bytes sha256 {sha256}; no unverified executable was run",
        program.display(), receipt.executable_bytes, receipt.executable_sha256);
    Ok(Some(Prepared { program, receipt }))
}
