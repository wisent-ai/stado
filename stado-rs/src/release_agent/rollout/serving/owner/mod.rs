//! Who owns the stable bind right now.
//!
//! Two halves of one question: what the kernel says about a process, and
//! whether the process holding this product's stable bind is its own release
//! proxy, the product serving directly, or something else entirely.

pub(crate) mod discover;
pub(crate) mod process;
