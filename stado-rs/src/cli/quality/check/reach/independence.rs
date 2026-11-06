//! The product-independence half of the reach check: a revision whose added
//! lines reach another catalog product the catalog does not show the product
//! running without is refused, naming every line and the declaration that
//! would show it (`stado_product::added_independence`).
//!
//! `stado product dependencies` reports the same judgement over every line
//! of every product; this step keeps a push, a change hand-off or a delivery
//! from adding to it. Only the lines the revision adds against the published
//! main are read, so a line already on main is never a later push's refusal.

use std::path::Path;

use stado_product::{Added, IndependenceVerdict};

use crate::cli::quality::Report;
use crate::cli::CmdError;

/// Refuse `revision` of `product`, exported at `tree`, when a line of
/// `added` reaches a catalog product `product` cannot be shown to run
/// without; say why when the lines are not judged.
pub(super) async fn check(
    tree: &Path,
    added: Vec<Added>,
    revision: &str,
    product: &str,
    report: Report,
) -> Result<(), CmdError> {
    let (tree, owned) = (tree.to_path_buf(), product.to_owned());
    let judged = tokio::task::spawn_blocking(move || {
        let runtime = stado_product::common::Runtime::new(None)?;
        stado_product::added_independence(&runtime, &owned, &tree, &added)
    })
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "reading whether the lines {revision} adds keep {product} running on its own did \
             not finish: {error}"
        ))
    })?
    .map_err(|error| {
        CmdError::declaration(format!(
            "whether the lines {revision} adds keep {product} running on its own cannot be \
             read: {error:#}"
        ))
    })?;
    match judged {
        IndependenceVerdict::Independent => Ok(()),
        IndependenceVerdict::Unjudged(why) => {
            report.say(&format!(
                "stado quality check: the lines {revision} adds are not judged for product \
                 independence: {why}"
            ));
            Ok(())
        }
        IndependenceVerdict::Refused(refused) => Err(CmdError::refused(format!(
            "{revision} of {product} adds lines that reach another product the catalog does \
             not show {product} running without:\n{refused}"
        ))
        .detailing(serde_json::json!({ "refused": refused.lines().collect::<Vec<_>>() }))),
    }
}
