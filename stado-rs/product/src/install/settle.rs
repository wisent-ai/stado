//! What an installation leaves of its build once it is recorded installed.
//!
//! A source build's run holds the placements' sources: once installed they
//! are read by nothing, and in a checkout they sit where the janitor may not
//! reach, so the run's directories go and its evidence stays
//! ([`crate::common::runs::shed`]). A build tree the next build of the same
//! workspace reuses (a desktop product's SwiftPM `.build`) stays, tagged as a
//! cache so the janitor reclaims it under disk pressure; SwiftPM writes no tag
//! of its own. Either step that fails is named in the installation's state.

use std::path::Path;

use serde_json::json;

use crate::state::ProductState;

pub(super) fn settle(installed: &mut ProductState, scratch: Option<&Path>, cache: Option<&Path>) {
    if let Some(run) = scratch {
        if let Err(error) = crate::common::runs::shed(run) {
            installed
                .extra
                .insert("scratch_kept".to_owned(), json!(format!("{error:#}")));
        }
    }
    if let Some(tree) = cache.filter(|tree| tree.is_dir()) {
        if let Err(error) = crate::common::tag_cache(tree, "stado product install") {
            installed
                .extra
                .insert("cache_untagged".to_owned(), json!(format!("{error:#}")));
        }
    }
}
