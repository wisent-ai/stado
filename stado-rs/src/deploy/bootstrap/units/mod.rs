//! The remote account a bootstrap provisions: the user its ssh destination
//! names and that user's home, where the workload grant file is written.

/// The account an ssh destination names before its `@`, or root when it
/// names none.
fn remote_user(ssh_target: &str) -> String {
    match ssh_target.split_once('@') {
        Some((user, _)) => user.to_string(),
        None => "root".to_string(),
    }
}

/// The remote account's home, for a path a remote file must carry absolute:
/// `/root` for root, `/home/<user>` otherwise.
pub(super) fn remote_home(ssh_target: &str) -> String {
    match remote_user(ssh_target).as_str() {
        "root" => "/root".to_string(),
        user => format!("/home/{user}"),
    }
}
