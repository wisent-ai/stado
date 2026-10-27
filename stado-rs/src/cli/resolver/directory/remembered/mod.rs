//! What this machine remembers about reads of a remote registry authority:
//! a read given up on ([`unanswered`]) and a read answered ([`recalled`]),
//! each kept on disk so every next command inside the refresh interval is
//! told at once instead of asking the authority again.

pub(in crate::cli::resolver) mod recalled;
pub(in crate::cli::resolver) mod shared;
pub(in crate::cli::resolver) mod unanswered;
pub(in crate::cli::resolver) mod verbs;
