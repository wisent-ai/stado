//! A real OpenSSH server for a journey's isolated home, standing for a fleet
//! host, which the journey stops with SIGSTOP once it has answered: the
//! kernel still completes every TCP connection into the stopped server's
//! backlog and nothing ever sends a banner, as on a host whose sshd hangs.
//!
//! The server runs as the account the test runs as, on a loopback port, with
//! its own host key, its own authorized key and a forced `HOME` of the run's
//! directory, so a command it runs never reads or writes the account's own
//! state. The client side lives in the run's `HOME` too: `~/.ssh/id_ed25519`
//! and a `known_hosts` line for `[<loopback>]:<port>`, which is how the
//! product trusts a server off SSH's own port.
use std::io::{BufRead, BufReader, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

/// The OpenSSH server every supported platform installs at this path.
const SSHD: &str = "/usr/sbin/sshd";

pub(crate) struct HungSsh {
    child: Child,
    /// The destination the product dials: `[<loopback>]:<port>`.
    pub(crate) destination: String,
    log: PathBuf,
}

fn keygen(path: &Path) {
    let output = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "journey", "-f"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .expect("run ssh-keygen");
    assert!(
        output.status.success(),
        "ssh-keygen -f {} failed ({}): {}",
        path.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn public(path: &Path) -> String {
    let public = path.with_extension("pub");
    match std::fs::read_to_string(&public) {
        Ok(text) => text.trim().to_string(),
        Err(error) => panic!("read {}: {error}", public.display()),
    }
}

impl HungSsh {
    /// Serve `root`'s client account on `port` and return once the server
    /// says it listens there; its own log line is the event waited for.
    pub(crate) fn serve(root: &Path, port: u16) -> Self {
        let loopback = Ipv4Addr::LOCALHOST;
        let directory = root.join("sshd");
        let client = root.join(".ssh");
        std::fs::create_dir_all(&directory).expect("create the server's directory");
        std::fs::create_dir_all(&client).expect("create the client's .ssh");
        let host_key = directory.join("host_key");
        let user_key = client.join("id_ed25519");
        keygen(&host_key);
        keygen(&user_key);
        std::fs::write(
            client.join("known_hosts"),
            format!("[{loopback}]:{port} {}\n", public(&host_key)),
        )
        .expect("write the client's known_hosts");
        let path = std::env::var("PATH").expect("the test runs with a PATH");
        std::fs::write(
            directory.join("authorized_keys"),
            format!(
                "environment=\"HOME={}\",environment=\"PATH={path}\" {}\n",
                root.display(),
                public(&user_key)
            ),
        )
        .expect("write the server's authorized_keys");
        let config = directory.join("sshd_config");
        std::fs::write(
            &config,
            format!(
                "Port {port}\nListenAddress {loopback}\nHostKey {}\nPidFile {}\nAuthorizedKeysFile {}\n\
                 PasswordAuthentication no\nKbdInteractiveAuthentication no\nUsePAM no\nStrictModes no\n\
                 PermitUserEnvironment yes\nLogLevel INFO\n",
                host_key.display(),
                directory.join("sshd.pid").display(),
                directory.join("authorized_keys").display(),
            ),
        )
        .expect("write the server's config");
        let log = directory.join("sshd.log");
        let spawned = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => panic!("start {SSHD}: {error}"),
        };
        let stderr = child.stderr.take().expect("the server's stderr is piped");
        listening(
            stderr,
            &format!("Server listening on {loopback} port {port}"),
            &log,
        );
        Self {
            child,
            destination: format!("[{loopback}]:{port}"),
            log,
        }
    }

    /// Stop the server: from now on it answers nothing it accepts.
    pub(crate) fn hang(&self) {
        let pid = Pid::from_raw(i32::try_from(self.child.id()).expect("a pid fits an i32"));
        if let Err(error) = kill(pid, Signal::SIGSTOP) {
            panic!("stop the SSH server {pid}: {error}");
        }
    }

    /// The server's own log, kept beside the run.
    pub(crate) fn log(&self) -> &Path {
        &self.log
    }
}

/// Copy the server's log into `log` until it prints `wanted`, then keep
/// copying it there; a server that ends first fails the journey with
/// everything it said. A later read or write of the log that fails is
/// written to the test's own stderr with its cause.
fn listening(stderr: ChildStderr, wanted: &str, log: &Path) {
    let mut file = std::fs::File::create(log).expect("create the server's log");
    let mut lines = BufReader::new(stderr).lines();
    let mut said = String::new();
    loop {
        match lines.next() {
            Some(Ok(line)) => {
                writeln!(file, "{line}").expect("keep the server's log");
                said.push_str(&line);
                said.push('\n');
                if line.contains(wanted) {
                    break;
                }
            }
            Some(Err(error)) => panic!("read the SSH server's log: {error}; it said: {said}"),
            None => panic!("the SSH server ended before it said {wanted:?}: {said}"),
        }
    }
    let kept = log.to_path_buf();
    std::thread::spawn(move || {
        for line in lines {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    eprintln!(
                        "reading the SSH server's log into {} stopped: {error}",
                        kept.display()
                    );
                    return;
                }
            };
            if let Err(error) = writeln!(file, "{line}") {
                eprintln!(
                    "keeping the SSH server's log in {} stopped: {error}",
                    kept.display()
                );
                return;
            }
        }
    });
}

impl Drop for HungSsh {
    fn drop(&mut self) {
        let pid = Pid::from_raw(i32::try_from(self.child.id()).expect("a pid fits an i32"));
        // A stopped server cannot act on the kill until it runs again.
        let resumed = kill(pid, Signal::SIGCONT);
        let killed = self.child.kill();
        let waited = self.child.wait();
        if !std::thread::panicking() {
            resumed.expect("resume the SSH server before ending it");
            killed.expect("end the SSH server");
            waited.expect("reap the SSH server");
        }
    }
}
