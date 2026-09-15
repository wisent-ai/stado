//! Real CLI custody and broker reads over an isolated host, vault, and keyring.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::Path;
use std::process::{Child, Output};
use std::time::{Duration, Instant};

use serde_json::json;

use super::{
    host::{said, IsolatedHost, TARGET},
    put, report, ITEM, PASSWORD,
};

const CONSUMER: &str = "credentials-delivery";
const SOURCE: &str = "custody-source";
const DESTINATION: &str = "custody-destination";

fn mint(host: &IsolatedHost) {
    let output = host.run(
        &[
            "credentials",
            "token",
            "mint",
            "--host",
            TARGET,
            CONSUMER,
            "--capabilities",
            &format!("read:{ITEM}#password"),
            "--audience",
            CONSUMER,
            "--token-file-name",
            SOURCE,
            "--json",
        ],
        None,
    );
    assert!(output.status.success(), "{}", said(&output));
}

fn sync(host: &IsolatedHost, source: &str, destination: &str, check: bool) -> Output {
    let source = format!("~/.stado/{source}");
    let destination = format!("~/.stado/{destination}");
    let mut arguments = vec![
        "credentials",
        "token",
        "sync",
        CONSUMER,
        "--from-host",
        TARGET,
        "--host",
        TARGET,
        "--source-token-file",
        &source,
        "--token-file",
        &destination,
        "--json",
    ];
    if check {
        arguments.push("--check");
    }
    let output = host.run(&arguments, None);
    println!("stado {}\n{}", arguments.join(" "), said(&output));
    output
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

struct Broker {
    child: Child,
    url: String,
}

impl Broker {
    fn start(host: &IsolatedHost) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = host
            .broker_command()
            .args(["serve", "--port", &port.to_string()])
            .stdout(fs::File::create(host.home.join("broker.out")).unwrap())
            .stderr(fs::File::create(host.home.join("broker.err")).unwrap())
            .spawn()
            .expect("start the real broker");
        let mut broker = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return broker;
            }
            if broker.child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "real broker did not start: {}",
            fs::read_to_string(host.home.join("broker.err")).unwrap()
        );
    }

    fn read(&self, token_file: &Path) -> (u16, serde_json::Value) {
        let token = fs::read_to_string(token_file).unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let response = reqwest::Client::new()
                .post(format!("{}/v1/items/read", self.url))
                .bearer_auth(token.trim_end())
                .header("x-consumer", CONSUMER)
                .json(&json!({"id": ITEM, "field": "password"}))
                .send()
                .await
                .expect("read through the real authenticated API");
            let status = response.status().as_u16();
            let body = response.json().await.unwrap();
            (status, body)
        })
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn token_delivery_restores_a_real_read_without_changing_grants_and_is_idempotent() {
    let host = IsolatedHost::new(true);
    put(&host);
    mint(&host);
    let source = host.home.join(".stado").join(SOURCE);
    let destination = host.home.join(".stado").join(DESTINATION);
    let source_before = fs::read(&source).unwrap();
    let vault_before = host.vault_bytes();
    write_private(&destination, b"unregistered-isolated-bearer");
    let broker = Broker::start(&host);
    assert_eq!(broker.read(&destination).0, 403);
    assert!(!sync(&host, SOURCE, DESTINATION, true).status.success());
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"unregistered-isolated-bearer"
    );

    let delivered = sync(&host, SOURCE, DESTINATION, false);
    assert!(delivered.status.success(), "{}", said(&delivered));
    assert_eq!(report(&delivered)["status"], "token_synced");
    assert_eq!(fs::metadata(&destination).unwrap().mode() & 0o777, 0o600);
    let (status, body) = broker.read(&destination);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["value"], PASSWORD);
    assert_eq!(host.vault_bytes(), vault_before);
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert!(!said(&delivered).contains(std::str::from_utf8(&source_before).unwrap().trim_end()));

    let inode = fs::metadata(&destination).unwrap().ino();
    let repeated = sync(&host, SOURCE, DESTINATION, false);
    assert!(repeated.status.success(), "{}", said(&repeated));
    assert_eq!(report(&repeated)["status"], "token_unchanged");
    assert_eq!(fs::metadata(&destination).unwrap().ino(), inode);
    assert!(sync(&host, SOURCE, DESTINATION, true).status.success());
}

#[test]
fn a_revoked_grant_cannot_be_restored_by_delivering_its_old_bearer() {
    let host = IsolatedHost::new(true);
    put(&host);
    mint(&host);
    assert!(sync(&host, SOURCE, DESTINATION, false).status.success());
    let destination = host.home.join(".stado").join(DESTINATION);
    let before = fs::read(&destination).unwrap();
    let revoked = host
        .broker_command()
        .args(["grant", "revoke", CONSUMER])
        .output()
        .unwrap();
    assert!(revoked.status.success(), "{}", said(&revoked));
    let vault = host.vault_bytes();
    assert!(!sync(&host, SOURCE, DESTINATION, false).status.success());
    assert_eq!(host.vault_bytes(), vault);
    assert_eq!(fs::read(&destination).unwrap(), before);
    let broker = Broker::start(&host);
    assert_eq!(broker.read(&destination).0, 403);
}

#[test]
fn mismatched_and_symlink_bearers_refuse_without_overwriting_the_destination() {
    let host = IsolatedHost::new(true);
    put(&host);
    mint(&host);
    let root = host.home.join(".stado");
    let original = fs::read(root.join(SOURCE)).unwrap();
    write_private(&root.join(DESTINATION), b"destination-remains-unchanged");
    write_private(&root.join("wrong"), b"unregistered-isolated-bearer");
    let vault = host.vault_bytes();
    assert!(!sync(&host, "wrong", DESTINATION, false).status.success());
    symlink(root.join(SOURCE), root.join("source-link")).unwrap();
    assert!(!sync(&host, "source-link", DESTINATION, false)
        .status
        .success());
    symlink(root.join(DESTINATION), root.join("destination-link")).unwrap();
    assert!(!sync(&host, SOURCE, "destination-link", false)
        .status
        .success());
    assert!(fs::symlink_metadata(root.join("destination-link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(host.vault_bytes(), vault);
    assert_eq!(fs::read(root.join(SOURCE)).unwrap(), original);
    assert_eq!(
        fs::read(root.join(DESTINATION)).unwrap(),
        b"destination-remains-unchanged"
    );
}
