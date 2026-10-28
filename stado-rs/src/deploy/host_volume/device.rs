//! The device name `stado space volume mount` takes.

/// A device name the program will accept: one `/dev` leaf such as `sdb1` or
/// `nvme0n1p2`, never a path and never a word the shell could read.
pub fn validate_device(device: &str) -> Result<(), String> {
    let acceptable = !device.is_empty()
        && device
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if acceptable {
        Ok(())
    } else {
        Err(format!(
            "--device names one /dev leaf such as sdb1 or nvme0n1p2, not {device:?}"
        ))
    }
}
