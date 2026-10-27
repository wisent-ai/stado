use super::*;

pub(in crate::deploy::host_storage_reconcile) fn write_json_evidence<T: Serialize + ?Sized>(
    transaction: &str,
    file_name: &str,
    value: &T,
    label: &str,
    replace: bool,
) -> Result<ImmutableEvidenceReference, DeployError> {
    verify_resident_lock(transaction)?;
    let path = transaction_directory(transaction)?.join(file_name);
    let encoded = encoded_json(value, label)?;
    if replace {
        atomic_bytes_file(&path, &encoded, label)?;
    } else {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => {
                if read_regular_file(&path, label)? != encoded {
                    return Err(DeployError(format!(
                        "{label} changed after its immutable publication"
                    ))
                    .stating(crate::primitives::failure::FailureCode::Refused));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                atomic_bytes_file(&path, &encoded, label)?;
            }
            Err(error) => {
                return Err(DeployError::io(format!(
                    "cannot inspect {}",
                    path.display()
                ))(error));
            }
        }
    }
    evidence_reference(&path, &encoded, label)
}

pub(in crate::deploy::host_storage_reconcile) fn read_json_evidence(
    transaction: &str,
    file_name: &str,
    reference: &ImmutableEvidenceReference,
    label: &str,
) -> Result<Value, DeployError> {
    verify_resident_lock(transaction)?;
    let path = transaction_directory(transaction)?.join(file_name);
    if path.to_str() != Some(reference.path.as_str()) {
        return Err(DeployError(format!(
            "{label} reference does not name its canonical transaction file"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let canonical = std::fs::symlink_metadata(&path).map_err(DeployError::io(format!(
        "cannot inspect {}",
        path.display()
    )))?;
    if !canonical.file_type().is_file() || canonical.file_type().is_symlink() {
        return Err(
            DeployError(format!("{label} is not a regular file: {}", path.display()))
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
    }
    let mut file = std::fs::File::open(&path)
        .map_err(DeployError::io(format!("cannot open {}", path.display())))?;
    let opened = file.metadata().map_err(DeployError::io(format!(
        "cannot inspect {}",
        path.display()
    )))?;
    if opened.dev() != canonical.dev() || opened.ino() != canonical.ino() {
        return Err(DeployError(format!(
            "{label} changed while its canonical file was opened"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    // The opened handle is hashed, not the path: it is the file whose device
    // and inode were just checked against the canonical one.
    let observed = crate::primitives::digest::sha256_reader(&mut file)
        .map_err(DeployError::io(format!("cannot hash {label}")))?;
    if observed.bytes != reference.bytes || observed.sha256 != reference.sha256 {
        return Err(
            DeployError(format!("{label} bytes differ from their durable reference"))
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
    }
    std::io::Seek::rewind(&mut file).map_err(DeployError::io(format!("cannot rewind {label}")))?;
    serde_json::from_reader(file).map_err(|error| {
        DeployError(format!("{label} is invalid: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
