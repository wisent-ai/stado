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

/// A registry destination as OpenSSH's `ssh` reads it, with the options its
/// transport needs before it. `ssh` takes no port in a `user@host`
/// destination, so one naming its port the way known_hosts writes it,
/// `user@[host]:port`, is given as the `ssh://user@host:port` URI `ssh`
/// accepts (an IPv6 host keeps its brackets); every other destination is
/// given as it is. A host on Stado's own network (`<host>.stado.internal`)
/// is reached through `ProxyCommand=stado network connect %h %p`, run by
/// this very binary. The native session reads the known_hosts form itself.
pub fn ssh_destination_args(target: &str) -> Vec<String> {
    let (user, host) = match target.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, target),
    };
    let mut argv = Vec::with_capacity(3);
    let bare = match host.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((inner, _)) => inner,
        None => host,
    };
    if crate::network::network_host(bare).is_some() {
        // This binary, so the proxy speaks the same protocol version; a
        // process the kernel cannot name its executable for asks the shell
        // for `stado` on its PATH, which is where every install puts it.
        let program = match std::env::current_exe() {
            Ok(path) => crate::deploy::shlex_quote(&path.to_string_lossy()),
            Err(_) => "stado".to_string(),
        };
        argv.push("-o".to_string());
        argv.push(format!("ProxyCommand={program} network connect %h %p"));
    }
    let Some((inner, port)) = host
        .strip_prefix('[')
        .and_then(|rest| rest.split_once("]:"))
    else {
        argv.push(target.to_string());
        return argv;
    };
    let host = if inner.contains(':') {
        format!("[{inner}]")
    } else {
        inner.to_string()
    };
    argv.push(match user {
        Some(user) => format!("ssh://{user}@{host}:{port}"),
        None => format!("ssh://{host}:{port}"),
    });
    argv
}
