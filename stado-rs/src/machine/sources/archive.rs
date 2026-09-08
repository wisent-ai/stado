//! The safety audit a staged tar.gz must pass before anything unpacks it.

use std::io::{BufReader, Read};
use std::path::Path;

use crate::machine::contract::encoding::py_repr;
use crate::machine::MachineError;

use super::{unsafe_archive_name, MAX_SOURCE_EXTRACTED_BYTES, MAX_SOURCE_MEMBERS};

pub(in crate::machine) fn validate_staged_source_archive(path: &Path) -> Result<(), MachineError> {
    fn invalid(msg: impl Into<String>) -> MachineError {
        MachineError::new("INVALID_SOURCE_ARCHIVE", msg)
    }
    let tar_invalid = |error: std::io::Error| invalid(format!("invalid tar.gz archive: {error}"));
    let file = std::fs::File::open(path)
        .map_err(|error| invalid(format!("staged source archive is not readable: {error}")))?;
    let decoder = flate2::bufread::GzDecoder::new(BufReader::new(file));
    let mut archive = tar::Archive::new(decoder);
    let mut total_size: u64 = 0;
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let entries = archive.entries().map_err(tar_invalid)?;
    for (index, entry) in entries.enumerate() {
        if index as u64 >= MAX_SOURCE_MEMBERS {
            return Err(invalid("source archive has too many entries"));
        }
        let entry = entry.map_err(tar_invalid)?;
        let path_bytes = entry.path_bytes();
        let name = std::str::from_utf8(path_bytes.as_ref())
            .map_err(|_| invalid("source archive entry name is not UTF-8"))?
            .to_string();
        if unsafe_archive_name(&name) {
            return Err(invalid(format!("unsafe archive entry: {}", py_repr(&name))));
        }
        if !seen.insert(name.clone()) {
            return Err(invalid(format!(
                "duplicate archive entry: {}",
                py_repr(&name)
            )));
        }
        let entry_type = entry.header().entry_type();
        let is_dir = entry_type == tar::EntryType::Directory;
        let is_reg = entry_type == tar::EntryType::Regular;
        if !is_dir && !is_reg {
            return Err(invalid(format!(
                "non-regular archive entry: {}",
                py_repr(&name)
            )));
        }
        if is_reg {
            let entry_size = entry.header().size().map_err(tar_invalid)?;
            total_size = total_size
                .checked_add(entry_size)
                .ok_or_else(|| invalid("source archive extracted size overflows"))?;
            if total_size > MAX_SOURCE_EXTRACTED_BYTES {
                return Err(invalid("source archive expands beyond the safety limit"));
            }
        }
    }
    let mut decoder = archive.into_inner();
    let mut trailing = [0u8; 8192];
    let mut trailing_bytes = 0u64;
    loop {
        let read = decoder.read(&mut trailing).map_err(tar_invalid)?;
        if read == 0 {
            break;
        }
        trailing_bytes = trailing_bytes
            .checked_add(read as u64)
            .ok_or_else(|| invalid("source archive trailing size overflows"))?;
        if trailing_bytes > 1024 * 1024 || trailing[..read].iter().any(|byte| *byte != 0) {
            return Err(invalid(
                "source archive contains a trailing or second tar payload",
            ));
        }
    }
    let mut compressed = decoder.into_inner();
    let mut extra = [0u8; 1];
    if compressed.read(&mut extra).map_err(tar_invalid)? != 0 {
        return Err(invalid(
            "source archive contains trailing or multiple gzip payloads",
        ));
    }
    Ok(())
}
