//! The three subcommands, one per file: stand the entrance up and prove it,
//! report what is published and whether it still answers, and take it down
//! again.

pub(in crate::cli::fleet::ingress) mod down;
pub(in crate::cli::fleet::ingress) mod status;
pub(in crate::cli::fleet::ingress) mod up;
