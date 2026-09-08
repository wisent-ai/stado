//! What the read-only commands print: the run columns `list` and `status`
//! share, and the two reports themselves.

mod list;
mod status;

pub(in crate::cli::builds) use list::list;
pub(in crate::cli::builds) use status::status;

use crate::targets::{BuildRecipe, BuildRun};

/// The platforms a recipe has something to say about: the ones it declares,
/// then any platform that still carries a recorded run after being dropped
/// from the declaration — the run happened, and hiding it would hide it for
/// good.
fn reported_platforms(recipe: &BuildRecipe) -> Vec<String> {
    let mut platforms = recipe.platforms.clone();
    for platform in recipe.runs.keys() {
        if !platforms.iter().any(|declared| declared == platform) {
            platforms.push(platform.clone());
        }
    }
    platforms
}

/// The run columns `list` and `status` share: platform, status, version,
/// declared, when. A platform with no recorded run reads `never`, not blank.
///
/// `when` is padded to the widest timestamp
/// [`crate::models::isoformat_utc`] emits, so `status` can append its own
/// column after it; a caller for whom `when` IS the last column trims the
/// row.
fn run_row(platform: &str, run: Option<&BuildRun>) -> String {
    match run {
        Some(run) => format!(
            "{platform:<14} {:<10} {:<14} {:<9} {:<32}",
            run.status,
            run.version.as_deref().unwrap_or("-"),
            run.declared,
            run.at
        ),
        None => format!(
            "{platform:<14} {:<10} {:<14} {:<9} {:<32}",
            "never", "-", "-", "-"
        ),
    }
}

/// The header for [`run_row`]'s columns, padded by the same widths so the two
/// cannot drift apart.
fn run_header() -> String {
    format!(
        "  {:<14} {:<10} {:<14} {:<9} {:<32}",
        "PLATFORM", "STATUS", "VERSION", "DECLARED", "WHEN"
    )
}
