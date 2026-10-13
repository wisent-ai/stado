//! Who and what may reach a host: the ssh key it trusts, the resolver key it
//! serves with, the forwards opened to it, and the accounts taken off it.

pub mod forward;
pub(crate) mod native;
pub mod resolver_key;
pub mod ssh_key;
pub mod user_delete;

/// The port a native SSH session dials when its destination names none: SSH's
/// IANA-assigned one. A destination names another as known_hosts writes a
/// server off that port, `user@[host]:port`.
pub use native::SSH_PORT;

/// A registry destination as OpenSSH's `ssh` reads it. `ssh` takes no port
/// in a `user@host` destination, so one naming its port the way known_hosts
/// writes it, `user@[host]:port`, is given as the `ssh://user@host:port` URI
/// `ssh` accepts (an IPv6 host keeps its brackets); every other destination
/// is given as it is. The native session reads the known_hosts form itself.
pub fn ssh_destination(target: &str) -> String {
    let (user, host) = match target.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, target),
    };
    let Some((inner, port)) = host
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
    else {
        return target.to_string();
    };
    let host = match inner.contains(':') {
        true => format!("[{inner}]"),
        false => inner.to_string(),
    };
    match user {
        Some(user) => format!("ssh://{user}@{host}:{port}"),
        None => format!("ssh://{host}:{port}"),
    }
}
