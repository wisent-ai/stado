//! The two write bodies: `deploy` installs a unit, `ensure` converges one.

use std::sync::LazyLock;

mod deploy_body;
mod ensure_head;
mod ensure_tail;

pub(crate) use deploy_body::*;

use ensure_head::ENSURE_BODY_HEAD;
use ensure_tail::ENSURE_BODY_TAIL;

/// The `service ensure` body, rejoined from the two halves it is stored in.
///
/// One script and one string: `ensure_head.rs` carries it up to the `else` of
/// the activation branch and `ensure_tail.rs` carries the rest, because no
/// source file in this tree may exceed 300 lines and the body alone is 419.
/// They are concatenated once, here, so every caller reads exactly the bytes
/// the single constant carried.
pub(crate) static ENSURE_BODY: LazyLock<String> =
    LazyLock::new(|| format!("{ENSURE_BODY_HEAD}{ENSURE_BODY_TAIL}"));
