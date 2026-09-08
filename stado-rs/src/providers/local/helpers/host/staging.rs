//! How much disk the agent's staging tree is holding.

use std::path::Path;

/// Total size of files under d in GB. 0 if dir missing.
/// Python `_staging_size_gb`.
pub fn staging_size_gb(d: &Path) -> f64 {
    if !d.is_dir() {
        return 0.0;
    }
    dir_size_bytes(d) as f64 / 1024f64.powi(3)
}

fn dir_size_bytes(d: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(d) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size_bytes(&path);
        } else if let Ok(md) = path.metadata() {
            total += md.len();
        }
    }
    total
}
