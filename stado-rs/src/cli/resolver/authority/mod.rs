use tokio::process::Command;

pub(in crate::cli::resolver) mod paths;
pub(in crate::cli::resolver) mod refusal;
pub(in crate::cli::resolver) mod tunnel;

pub(crate) fn ssh_command(control_master: &'static str) -> Command {
    let mut command = Command::new("ssh");
    command.args([
        "-T",
        "-F",
        "/dev/null",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=2",
        "-o",
        control_master,
    ]);
    let key_file = std::env::var("STADO_RESOLVER_SSH_KEY_FILE")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".stado").join("resolver-ssh-key"))
                .filter(|path| path.is_file())
        });
    if let Some(key_file) = key_file {
        command
            .args(["-o", "IdentitiesOnly=yes", "-i"])
            .arg(key_file);
    }
    command
}
/// SSH transport for the forwards that carry adapter traffic.
///
/// This used to say that adapter requests "share one authenticated connection",
/// and that sharing "prevents one SSH process and one remote session per object
/// read". Only the second half was ever true: multiplexing shares the session,
/// while `ssh -W` still forked a client process for every request. The forwards
/// this command now starts are one per destination, so the first half is true
/// as well. Authority snapshots still run outside this path, so a stuck control
/// master cannot freeze registry refresh.
/// A unix socket path is capped by the kernel: 104 bytes on macOS, 108 on
/// Linux. `ssh` checks the path it was handed against that cap and refuses the
/// whole invocation with `ControlPath too long (... >= 104 bytes)`, exiting 255
/// before it attempts any connection. Nothing here ever checked, so a host
/// whose `HOME` is long enough -- a service started under
/// `$HOME/.stado/services/<label>/current/<platform>` is exactly that -- could
/// not open a transport at all, and the reason appeared only on ssh's stderr.
const CONTROL_PATH_LIMIT: usize = 104;
/// What `%C` expands to: ssh's connection hash, 40 hex characters.
const CONTROL_PATH_HASH: usize = 40;

fn ssh_proxy_command() -> Command {
    let prefix = std::env::var("HOME")
        .ok()
        .map(|home| format!("{home}/.stado/resolver-ssh-"));
    // Multiplexing was load-bearing while every request opened its own
    // session. A forward is one per destination, so a control master is now an
    // optimisation: when its socket path cannot fit, serve without one rather
    // than refuse to serve.
    match prefix.filter(|prefix| prefix.len() + CONTROL_PATH_HASH <= CONTROL_PATH_LIMIT) {
        Some(prefix) => {
            let mut command = ssh_command("ControlMaster=auto");
            command
                .args(["-o", "ControlPersist=60", "-o"])
                .arg(format!("ControlPath={prefix}%C"));
            command
        }
        None => ssh_command("ControlMaster=no"),
    }
}

/// Drop the SSH control sockets this resolver left behind.
///
/// Multiplexing keeps a master alive for `ControlPersist` after the resolver
/// that opened it is gone. The next instance then attaches to a master whose
/// connection has already died, every proxied request fails with `Broken pipe`,
/// and the adapter answers nothing while looking perfectly healthy -- the same
/// failure the fleet had this morning from an orphaned port forward. Removing
/// the socket file costs nothing: live sessions keep their descriptor, and the
/// next connection opens a fresh master.
pub(crate) fn drop_stale_ssh_sockets() {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let directory = std::path::Path::new(&home).join(".stado");
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("resolver-ssh-") {
            continue;
        }
        // Only sockets. `~/.stado/resolver-ssh-key` and its `.pub` share this
        // prefix, and deleting the resolver's own credential to clean up after
        // it is how a cleanup becomes the outage: the service then fails to
        // authenticate to the authority and exits before it can say why.
        let is_socket = entry
            .file_type()
            .map(|kind| {
                use std::os::unix::fs::FileTypeExt;
                kind.is_socket()
            })
            .unwrap_or(false);
        if !is_socket {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => eprintln!("stado resolver dropped stale ssh control socket {name}"),
            Err(error) => eprintln!("stado resolver could not drop {name}: {error}"),
        }
    }
}
