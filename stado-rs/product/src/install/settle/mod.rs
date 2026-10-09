//! What an installation leaves of its build once it is recorded installed.
//!
//! A source build's run holds the placements' sources: once installed they
//! are read by nothing, so the run's directories go and its evidence stays
//! ([`crate::common::runs::shed`]). A build tree the next build of the same
//! checkout reuses (a desktop product's SwiftPM `.build`) leaves the checkout
//! for its build area and is tagged as a cache there ([`build_tree`]), so the
//! janitor reclaims it under disk pressure. Either step that fails is named
//! in the installation's state.

pub(super) mod build_tree;

use std::path::Path;

use serde_json::json;

use crate::state::ProductState;

pub(super) fn settle(
    installed: &mut ProductState,
    scratch: Option<&Path>,
    checkout: Option<&Path>,
    kept: Option<&Path>,
) {
    if let Some(run) = scratch {
        if let Err(error) = crate::common::runs::shed(run) {
            installed
                .extra
                .insert("scratch_kept".to_owned(), json!(format!("{error:#}")));
        }
    }
    if let (Some(checkout), Some(kept)) = (checkout, kept) {
        if let Err(error) = build_tree::put_away(checkout, kept) {
            installed
                .extra
                .insert("build_tree_left".to_owned(), json!(format!("{error:#}")));
        }
    }
}
