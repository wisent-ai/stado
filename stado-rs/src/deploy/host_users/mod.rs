//! Create standard or administrator accounts on registry-managed hosts.
//!
//! Port of `stado/deploy/host_users.py`. Passwords travel only on SSH
//! stdin. They are never placed in the local SSH argv, the remote command
//! string, registry data, or command output.

mod provision;
mod remote;
mod select;
mod validate;

pub use provision::{provision_users, HostUserResult, ProvisionOptions};
pub use remote::{
    parse_status, remote_command, ssh_argv, REMOTE_CREATE_SCRIPT, SSH_TIMEOUT_SECONDS,
    STATUS_PREFIX,
};
pub use select::select_targets;
pub use validate::{validate_password, validate_shell, validate_ssh_target, validate_username};
