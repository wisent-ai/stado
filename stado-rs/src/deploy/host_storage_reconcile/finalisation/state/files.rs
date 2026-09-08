use super::*;

pub(in crate::deploy::host_storage_reconcile) fn atomic_bytes_file(
    path: &Path,
    encoded: &[u8],
    label: &str,
) -> Result<(), DeployError> {
    let parent = path
        .parent()
        .ok_or_else(|| DeployError(format!("{label} has no parent directory")))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    let parent_metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| DeployError(format!("cannot inspect {}: {error}", parent.display())))?;
    if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{label} parent is not a regular directory: {}",
            parent.display()
        )));
    }
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(format!("cannot protect {}: {error}", parent.display())))?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() || metadata.file_type().is_symlink() => {
            return Err(DeployError(format!(
                "{label} collides with a non-regular file: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DeployError(format!(
                "cannot inspect {}: {error}",
                path.display()
            )));
        }
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| DeployError(format!("{label} has an invalid file name")))?;
    let temporary = parent.join(format!(".{file_name}.{}.new", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", temporary.display())))?;
    file.write_all(encoded)
        .map_err(|error| DeployError(format!("cannot write {label}: {error}")))?;
    file.sync_all()
        .map_err(|error| DeployError(format!("cannot sync {label}: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| DeployError(format!("cannot publish {label}: {error}")))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DeployError(format!("cannot sync {}: {error}", parent.display())))?;
    Ok(())
}

pub(in crate::deploy::host_storage_reconcile) fn atomic_owner(
    path: &Path,
    owner: &Value,
) -> Result<(), DeployError> {
    let parent = path
        .parent()
        .ok_or_else(|| DeployError("operation owner has no parent directory".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", parent.display())))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| DeployError(format!("cannot protect {}: {error}", parent.display())))?;
    let temporary = parent.join(format!(".operation-owner.{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| DeployError(format!("cannot create {}: {error}", temporary.display())))?;
    serde_json::to_writer(&mut file, owner)
        .map_err(|error| DeployError(format!("cannot encode operation owner: {error}")))?;
    file.write_all(b"\n")
        .map_err(|error| DeployError(format!("cannot finish operation owner: {error}")))?;
    file.sync_all()
        .map_err(|error| DeployError(format!("cannot sync operation owner: {error}")))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| DeployError(format!("cannot publish operation owner: {error}")))?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| DeployError(format!("cannot sync {}: {error}", parent.display())))?;
    Ok(())
}
