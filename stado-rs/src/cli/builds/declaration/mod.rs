//! The recipe declaration: the words a recipe may carry (`checks`) and the
//! fenced read-modify-writes that declare one (`add`), change one (`edit`)
//! and remove or switch one off (`state`). Every write here goes through the
//! compare-and-swap path the module doc describes.

mod add;
mod checks;
mod edit;
mod state;

pub(in crate::cli::builds) use add::add;
pub(in crate::cli::builds) use checks::canonical_platforms;
pub(in crate::cli::builds) use edit::{edit, RecipeEdit};
pub(in crate::cli::builds) use state::{remove, set_enabled};

/// What turning `auto_declare` on means, said the same way by `add` and
/// `edit` so the operator reads one sentence about one behaviour.
const AUTO_DECLARE_ON: &str = "auto-declare on — a successful tagged build declares that version \
                               on every matching host (signed promotion stays `stado release \
                               promote`)";
