//! What each host confirms about the identity it is declared to hold.

mod accounts;
mod local;
mod session;

// `drivable_session` names `super::service::host_sudo_password` on the same path it
// used while this module was one file, so the import keeps that path resolving.
use crate::cli::service;

pub(super) use accounts::{observe_apple_accounts, observe_user_apple_accounts};
pub(super) use local::{is_local_target, local_apple_accounts, probes_own_user};
pub(super) use session::drivable_session;
