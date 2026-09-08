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
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                atomic_bytes_file(&path, &encoded, label)?;
            }
            Err(error) => {
                return Err(DeployError(format!(
                    "cannot inspect {}: {error}",
                    path.display()
                )));
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
        )));
    }
    let canonical = std::fs::symlink_metadata(&path)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if !canonical.file_type().is_file() || canonical.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    let mut file = std::fs::File::open(&path)
        .map_err(|error| DeployError(format!("cannot open {}: {error}", path.display())))?;
    let opened = file
        .metadata()
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", path.display())))?;
    if opened.dev() != canonical.dev() || opened.ino() != canonical.ino() {
        return Err(DeployError(format!(
            "{label} changed while its canonical file was opened"
        )));
    }
    let mut hasher = Sha256::new();
    let mut observed_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| DeployError(format!("cannot hash {label}: {error}")))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        observed_bytes += count as u64;
    }
    if observed_bytes != reference.bytes || hex::encode(hasher.finalize()) != reference.sha256 {
        return Err(DeployError(format!(
            "{label} bytes differ from their durable reference"
        )));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| DeployError(format!("cannot rewind {label}: {error}")))?;
    serde_json::from_reader(file)
        .map_err(|error| DeployError(format!("{label} is invalid: {error}")))
}
