//! `stado resolver relay <service> --consumer <consumer> --bind <address>`
//! carries one caller's connections to a service placed on another host over
//! this process's own SSH session, without this host's resolver: it prints
//! the loopback URL first, a connection dialled there reaches the service on
//! its host, and the relay ends when the caller closes its standard input.
//! A service placed on this host is answered with its own address and no
//! relay; a service whose host's channel is refused is refused by name.
//!
//! The real stado runs with an isolated `HOME` under the checkout's build
//! directory and a local store holding the shared directory template with
//! this machine as the reader, the dedicated host the fixture names as the
//! other target, and a service `elsewhere-api` placed there at that host's
//! own SSH port — the one port a registry target is known to answer on — so
//! the bytes read through the relay are that host's SSH banner.
//!
//! The fixture, `STADO_REGISTRY_AUTHORITY_FIXTURE`, is the owner-only JSON
//! file the authority-read journey reads: `{"dedicated": true, "target":
//! <registry name>, "ssh": <SSH destination>, "ssh_key_file": <private key
//! authorized there>, "known_hosts_file": <OpenSSH known_hosts carrying its
//! host key>}`. No fixture is a failed run, never a passed one.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};
use stado::deploy::host_access::SSH_PORT;

const READER: &str = "marker-host";
const SERVICE: &str = "elsewhere-api";
const CONSUMER: &str = "marker-consumer";
const OWN_SERVICE: &str = "marker-api";
const SSH_BANNER: &str = "SSH-";

struct Fixture {
    target: String,
    ssh: String,
    ssh_key_file: PathBuf,
    known_hosts_file: PathBuf,
}

impl Fixture {
    fn read() -> Self {
        let path = std::env::var("STADO_REGISTRY_AUTHORITY_FIXTURE").expect(
            "STADO_REGISTRY_AUTHORITY_FIXTURE must name the dedicated authority fixture file",
        );
        let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            fixture["dedicated"],
            json!(true),
            "only an explicitly dedicated host fixture is accepted"
        );
        let field = |name: &str| match fixture[name].as_str() {
            Some(value) => value.to_string(),
            None => panic!("the fixture names {name}"),
        };
        Self {
            target: field("target"),
            ssh: field("ssh"),
            ssh_key_file: PathBuf::from(field("ssh_key_file")),
            known_hosts_file: PathBuf::from(field("known_hosts_file")),
        }
    }
}

struct Deployment {
    root: PathBuf,
    fixture: Fixture,
}

impl Deployment {
    fn start() -> Self {
        let fixture = Fixture::read();
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/resolver-relay")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        std::fs::copy(&fixture.known_hosts_file, root.join(".ssh/known_hosts")).unwrap();
        Self { root, fixture }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_RESOLVER_SSH_KEY_FILE", &self.fixture.ssh_key_file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A loopback address this machine just handed out and nothing holds now.
    fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "free-port-local"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader and
    /// the fixture's host as the other target, `elsewhere-api` placed there
    /// at its SSH port and no adapter declared for it here.
    fn import_registry(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets.first_mut().unwrap();
        reader["hostnames"] = json!([hostname()]);
        reader["service_resolver"]["adapters"] = json!([]);
        let other = targets.last_mut().unwrap();
        other["name"] = json!(self.fixture.target);
        other["ssh"] = json!(self.fixture.ssh);
        let banner = format!(
            "http://{}",
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, SSH_PORT)
        );
        document["service_directory"]["services"][SERVICE] = json!({
            "managed_service": SERVICE,
            "active_host": self.fixture.target,
            "endpoints": {self.fixture.target.as_str(): {"url": banner}},
            "consumers": {CONSUMER: {}},
        });
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    /// The relay started with its standard input held by this test; its
    /// first stdout line is the announcement.
    fn relay(&self, service: &str, bind: &str) -> (Child, Value) {
        let mut child = self
            .command()
            .args([
                "resolver",
                "relay",
                service,
                "--consumer",
                CONSUMER,
                "--bind",
                bind,
                "--json",
            ])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut first)
            .unwrap();
        let announced: Value = match serde_json::from_str(&first) {
            Ok(announced) => announced,
            Err(error) => {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "the relay's first line is its announcement ({error}): {first:?}; stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        };
        (child, announced)
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

/// What the relay's listener answers to one connection: the service's first
/// line, as that host sends it.
fn first_line_through(url: &str) -> String {
    let address = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).expect("dial the relay's listener");
    let mut line = String::new();
    BufReader::new(&mut stream).read_line(&mut line).unwrap();
    line
}

fn ended(child: &mut Child, root: &Path) -> Output {
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    std::fs::write(root.join("relay.stderr"), &output.stderr).unwrap();
    output
}

#[test]
fn a_relay_carries_a_connection_to_the_service_on_its_host_and_ends_with_its_owner() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let bind = deployment.free_bind();

    let (mut child, announced) = deployment.relay(SERVICE, &bind);
    assert_eq!(announced["relayed"], json!(true), "{announced}");
    assert_eq!(
        announced["placed_on"],
        json!(deployment.fixture.target),
        "{announced}"
    );
    let url = announced["url"].as_str().unwrap().to_string();
    assert_eq!(
        url,
        format!("http://{bind}"),
        "the relay listens where it was told"
    );

    let banner = first_line_through(&url);
    assert!(
        banner.starts_with(SSH_BANNER),
        "the bytes read through the relay are the host's own SSH banner: {banner:?}"
    );

    let output = ended(&mut child, &deployment.root);
    assert!(
        output.status.success(),
        "the relay ends cleanly when its owner closes its standard input: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let refused = TcpStream::connect(&bind);
    assert!(
        refused.is_err(),
        "nothing listens on the relay's address once its owner let go"
    );
}

#[test]
fn a_service_placed_on_this_host_is_answered_with_its_own_address_and_no_relay() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let bind = deployment.free_bind();

    let (mut child, announced) = deployment.relay(OWN_SERVICE, &bind);
    assert_eq!(announced["relayed"], json!(false), "{announced}");
    assert_eq!(announced["placed_on"], json!(READER), "{announced}");
    let output = ended(&mut child, &deployment.root);
    assert!(
        output.status.success(),
        "a service on this host needs no relay and the command ends: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
