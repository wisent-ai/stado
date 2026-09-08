//! The real listeners a beacon publication has to walk, and the real vault
//! they authorize against.
//!
//! `stado host publish-beacon` is not a local write: it sends the merged
//! document to the Stado host-health route, and that route compares the
//! bearer it was given against `stado-host-health-api/token` read out of a
//! vault through the object verifier grant. So the chain is stood up for
//! real — a Skarbiec vault created with real GnuPG keys, the real Skarbiec
//! broker serving on loopback, and the real Stado dashboard serving on
//! loopback over a local storage backend — and the product walks it.

use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// The vault item and field the host-health route authorizes against, and the
/// consumer the dashboard reads it through. All three are product contract.
const ITEM: &str = "stado-host-health-api";
const FIELD: &str = "token";
const VERIFIER: &str = "stado-object-api-verifier";

/// The owner the fixture's own vault is initialised under. The address is in
/// the reserved `.invalid` TLD, so nothing about it can resolve anywhere.
const OWNER: &str = "Stado beacon area <beacon-area@example.invalid>";

/// The bearer this fixture provisions. It exists only inside one tempdir's
/// vault and one owner-only file, both of which go away with the case.
const BEARER: &str = "beacon-area-publisher-bearer";

/// Where the product's own tools live. GnuPG is found through the first two
/// entries on a Homebrew machine; the beacon's probes resolve out of the
/// system directories. Nothing belonging to a fixture is on it.
pub const SYSTEM_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

const OWNER_ONLY: u32 = 0o600;
pub const OWNER_ONLY_DIRECTORY: u32 = 0o700;

/// How long a fixture waits for a listener it started to accept. Generous:
/// the machine may be compiling another area at the same time.
const LISTENER_WAIT: Duration = Duration::from_secs(60);

/// One isolated vault holding the route's bearer, plus the grant the
/// dashboard reads it through.
pub struct Vault {
    broker: PathBuf,
    home: PathBuf,
    gnupg: PathBuf,
    path: PathBuf,
    grant: String,
}

impl Vault {
    /// Create the vault with real GnuPG keys, store the route's bearer in it,
    /// and mint the object verifier grant.
    pub fn provision(broker: PathBuf, home: &Path, gnupg: &Path, path: PathBuf) -> Self {
        let mut vault = Self {
            broker,
            home: home.to_path_buf(),
            gnupg: gnupg.to_path_buf(),
            path,
            grant: String::new(),
        };
        vault.ran(&["init", OWNER], "initialise an isolated vault");
        assert!(
            vault.path.is_file(),
            "the real broker reported success without writing {}",
            vault.path.display()
        );
        vault.ran(
            &["set", ITEM, "--type", "token", &format!("{FIELD}={BEARER}")],
            "store the host-health bearer",
        );
        let minted = vault.ran(
            &[
                "grant",
                "issue",
                VERIFIER,
                "--capabilities",
                &format!("read:{ITEM}#{FIELD}"),
            ],
            "mint the object verifier grant",
        );
        let report: Value = serde_json::from_slice(&minted).expect("the mint report is JSON");
        vault.grant = report["token"]
            .as_str()
            .expect("the grant is shown exactly once")
            .to_string();
        vault
    }

    /// The bearer a publisher must present.
    pub fn bearer(&self) -> &str {
        BEARER
    }

    /// The grant the dashboard reads the bearer through.
    pub fn grant(&self) -> &str {
        &self.grant
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.broker);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("GNUPGHOME", &self.gnupg)
            .env("SKARBIEC_VAULT_FILE", &self.path)
            .env(
                "SKARBIEC_AUDIT_FILE",
                self.home.join("skarbiec-audit.jsonl"),
            )
            .stdin(Stdio::null());
        command
    }

    fn ran(&self, arguments: &[&str], purpose: &str) -> Vec<u8> {
        let output = self
            .command()
            .args(arguments)
            .output()
            .unwrap_or_else(|error| panic!("blocked: the real broker could not run: {error}"));
        assert!(
            output.status.success(),
            "blocked: the real Skarbiec broker could not {purpose}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output.stdout
    }
}

pub fn start_skarbiec(vault: &Vault, port: u16) -> Child {
    vault
        .command()
        .args(["serve", "--port", &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the real Skarbiec listener starts")
}

pub fn start_dashboard(
    home: &Path,
    storage: &Path,
    root: &Path,
    port: u16,
    skarbiec_port: u16,
    verifier_token: &Path,
) -> Child {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args([
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .env_clear()
        .env("HOME", home)
        .env("PATH", SYSTEM_PATH)
        .env("STADO_CONFIG", root.join("no-such-config.json"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env(
            "WC_OBJECT_SKARBIEC_URL",
            format!("http://127.0.0.1:{skarbiec_port}"),
        )
        .env("WC_OBJECT_SKARBIEC_TOKEN_FILE", verifier_token)
        // The broad coordinator grant must be a different file from the
        // verifier's; the product refuses to conflate the two.
        .env("WC_SKARBIEC_TOKEN_FILE", root.join("coordinator-token"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the built stado dashboard starts")
}

/// The isolated keyring's agent holds a socket inside the tempdir, so it is
/// stopped before the directory goes away.
pub fn stop_key_agent(gnupg: &Path) {
    let _ = Command::new("gpgconf")
        .env("PATH", SYSTEM_PATH)
        .arg("--homedir")
        .arg(gnupg)
        .args(["--kill", "gpg-agent"])
        .output();
}

pub fn owner_only_file(path: &Path, value: &str) {
    fs::write(path, value).expect("write an owner-only grant file");
    fs::set_permissions(path, fs::Permissions::from_mode(OWNER_ONLY))
        .expect("keep the grant file owner-only");
}

/// A port nothing is listening on, taken by binding and releasing it.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind a loopback port")
        .local_addr()
        .expect("the bound socket has an address")
        .port()
}

pub fn await_listener(port: u16, what: &str) {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + LISTENER_WAIT;
    while Instant::now() < deadline {
        if TcpStream::connect(address).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("blocked: {what} never accepted a connection on 127.0.0.1:{port}");
}
