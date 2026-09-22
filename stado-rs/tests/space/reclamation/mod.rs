//! Reclaiming space for real, in a scope the case created itself.
//!
//! Both applying cases select one stage by name, and both stages sweep only
//! roots below the fixture's `HOME`: `build_scratch` takes stale trees under
//! `$HOME/.stado/build-work`, and `registry_cleanup` runs this binary's own
//! janitor, whose single declared cleaner is rooted at the fixture's
//! build-cache directory. Before anything is applied the dry run's paths are
//! read and the case refuses to continue unless every one of them is inside
//! its own tempdir, so a stage that ever widened its enumeration fails the
//! test instead of deleting an operator's files.
//!
//! Removal is proved by reading the filesystem — the payload exists before and
//! is gone after — and the byte figure by the janitor's own state document,
//! which has to agree with `stat`'s allocated blocks and with `du -sk`.

use std::path::Path;

/// Payload sizes for the scopes these cases create inside their own tempdir.
/// Small enough to write quickly, large enough that `du` and `stat` report
/// several whole allocation blocks rather than a rounding artefact.
pub(super) const SCRATCH_MIB: usize = 4;
pub(super) const CACHE_MIB: usize = 8;

/// Refuse to apply anything unless every path the preview named is inside the
/// tempdir this case owns.
pub(super) fn assert_inside(root: &Path, paths: &[String]) {
    let root = root.to_string_lossy().to_string();
    for path in paths {
        assert!(
            path.starts_with(&root),
            "the preview named {path}, which is outside this test's tempdir {root}; refusing to apply"
        );
    }
}


mod janitor;
mod scratch;
