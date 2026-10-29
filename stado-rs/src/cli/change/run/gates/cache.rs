//! The action cache: a gate shard that passed on a tree is not run again on
//! the same tree. The key is the SHA-256 of what decides the shard's
//! verdict — the Stado build that ran it (its locks, module and format
//! checks and the scripts its hosts run are that build's own), the
//! committed tree, the platform, the gate and the exact argv — so the same
//! candidate written again on an unchanged tip, a resubmitted commit, or a
//! sibling queue's identical tree is answered at once, and a pass an
//! earlier Stado judged is run again. Only a pass is kept: a refusal is run
//! again, so a flaky test is never cached into a verdict. Kept under the
//! queue's directory, `cache/<key>.json`.

use sha2::{Digest, Sha256};

use crate::cli::change::record::{root, Shard};
use crate::cli::CmdError;
use crate::primitives::document;

const WHAT: &str = "submit queue action cache entry";

/// The key of `argv` of `gate` for `platform` over `tree`, run by this
/// Stado build.
pub fn key(tree: &str, platform: &str, gate: &str, argv: &[String]) -> String {
    let mut hash = Sha256::new();
    let build = crate::binary::build_identity::BUILD_IDENTITY;
    for part in [build, tree, platform, gate] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    for word in argv {
        hash.update(word.as_bytes());
        hash.update([0]);
    }
    hex::encode(hash.finalize())
}

fn path(key: &str) -> Result<std::path::PathBuf, CmdError> {
    Ok(root()?.join("cache").join(format!("{key}.json")))
}

/// The passing shard kept under `key`, marked as answered from the cache.
pub fn hit(key: &str) -> Result<Option<Shard>, CmdError> {
    let kept: Option<Shard> = document::read(&path(key)?, WHAT).map_err(CmdError::click)?;
    Ok(kept.map(|mut shard| {
        shard.cached = true;
        shard
    }))
}

/// Keep `shard` under `key` when it passed.
pub fn keep(key: &str, shard: &Shard) -> Result<(), CmdError> {
    if shard.exit_code != 0 {
        return Ok(());
    }
    let path = path(key)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    document::write(&path, shard, WHAT).map_err(CmdError::click)
}
