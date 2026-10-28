//! The SHA-256 of a file's bytes, streamed through the hasher with
//! `std::io::copy`, so no caller picks a buffer size and every artifact,
//! replica and archive digest in the crate is computed one way.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

/// What one streamed read of a file established: how many bytes it held and
/// their SHA-256 in lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256Digest {
    pub bytes: u64,
    pub sha256: String,
}

/// The SHA-256 of everything `reader` yields.
pub fn sha256_reader(reader: &mut impl Read) -> std::io::Result<Sha256Digest> {
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(reader, &mut hasher)?;
    Ok(Sha256Digest {
        bytes,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// The SHA-256 of the file at `path`, or the operating system's error naming
/// the path that could not be opened or read.
pub fn sha256_file(path: &Path) -> std::io::Result<Sha256Digest> {
    let named = |error: std::io::Error| {
        std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
    };
    let mut file = std::fs::File::open(path).map_err(named)?;
    sha256_reader(&mut file).map_err(named)
}
