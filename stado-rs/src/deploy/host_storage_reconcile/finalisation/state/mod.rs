use super::*;

mod evidence;
mod files;
mod resident;

pub(in crate::deploy::host_storage_reconcile) use evidence::*;
pub(in crate::deploy::host_storage_reconcile) use files::*;
pub(in crate::deploy::host_storage_reconcile) use resident::*;

pub(in crate::deploy::host_storage_reconcile) fn transaction_directory(
    transaction: &str,
) -> Result<PathBuf, DeployError> {
    let home = crate::config_file::home().map_err(|cause| {
        DeployError(format!("resident transaction worker has no home: {cause}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok(home
        .join(".stado/recovery/storage-root-reconcile")
        .join(transaction))
}

fn encoded_json<T: Serialize + ?Sized>(value: &T, label: &str) -> Result<Vec<u8>, DeployError> {
    let mut encoded = serde_json::to_vec(value)
        .map_err(|error| DeployError(format!("cannot encode {label}: {error}")))?;
    encoded.push(b'\n');
    Ok(encoded)
}

pub(in crate::deploy::host_storage_reconcile) fn atomic_json_file<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    label: &str,
) -> Result<(), DeployError> {
    atomic_bytes_file(path, &encoded_json(value, label)?, label)
}

fn evidence_reference(
    path: &Path,
    encoded: &[u8],
    label: &str,
) -> Result<ImmutableEvidenceReference, DeployError> {
    let path = path
        .to_str()
        .ok_or_else(|| {
            DeployError(format!("{label} path is not valid UTF-8"))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .to_string();
    Ok(ImmutableEvidenceReference {
        path,
        sha256: hex::encode(Sha256::digest(encoded)),
        bytes: encoded.len() as u64,
    })
}

fn read_regular_file(path: &Path, label: &str) -> Result<Vec<u8>, DeployError> {
    let metadata = std::fs::symlink_metadata(path).map_err(DeployError::io(format!(
        "cannot inspect {}",
        path.display()
    )))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(
            DeployError(format!("{label} is not a regular file: {}", path.display()))
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
    }
    std::fs::read(path).map_err(DeployError::io(format!("cannot read {}", path.display())))
}

pub(in crate::deploy::host_storage_reconcile) fn sha256_file(
    path: &Path,
) -> Result<String, DeployError> {
    crate::primitives::digest::sha256_file(path)
        .map(|digest| digest.sha256)
        .map_err(DeployError::io("cannot hash the file".to_string()))
}
