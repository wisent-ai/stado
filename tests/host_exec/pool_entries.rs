//! `stado host exec TARGET -- start-with-skarbiec subscription list --json`
//! (and `--refresh-usage`) and `-- skarbiec list` pass the allowlist, a
//! spelling that would apply a membership document is refused naming them,
//! and on the dedicated host the fixture names the pool read answers the
//! gateway's own pool document.
//!
//! The allowlist cases run the real `stado` against an isolated local store
//! under the build directory whose registry knows no host, so an admitted
//! spelling goes on to the registry and is refused there by name, while an
//! unapproved one is refused by the allowlist with the approved spellings.
//! The live read needs `STADO_REGISTRY_AUTHORITY_FIXTURE`, the owner-only
//! JSON file the authority-read journey reads, naming a dedicated host that
//! serves Brama; no fixture is a failed run, never a passed one.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "example-host";
const REFUSAL: &str = "is not an approved host-exec command";
const APPROVED_PREFIX: &str = "approved commands: ";
const POOL: &str = "start-with-skarbiec subscription list --json";
const POOL_REFRESHED: &str = "start-with-skarbiec subscription list --json --refresh-usage";
const VAULT_INVENTORY: &str = "skarbiec list";
const APPLY: &str = "start-with-skarbiec subscription list --json --apply";

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-exec-pool");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        Self { root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Run `stado host exec TARGET -- <spelling>`, the spelling split into
    /// its words the way an operator types them.
    fn run(&self, target: &str, spelling: &str) -> Output {
        let mut arguments = vec!["host", "exec", target, "--"];
        arguments.extend(spelling.split(' '));
        let output = self
            .command()
            .args(&arguments)
            .output()
            .expect("run the real stado");
        let name = spelling.replace(' ', "-");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn applying_a_membership_document_is_refused_naming_the_pool_reads() {
    let store = Store::new("apply");
    let output = store.run(TARGET, APPLY);
    let said = stderr(&output);
    assert!(
        !output.status.success() && said.contains(REFUSAL),
        "a spelling that would apply a document is refused: {said}"
    );
    let approved = match said
        .lines()
        .find_map(|line| line.split_once(APPROVED_PREFIX).map(|(_, rest)| rest))
    {
        Some(approved) => approved,
        None => panic!("the refusal carries the approved spellings: {said}"),
    };
    let spellings: Vec<&str> = approved.split(", ").map(str::trim).collect();
    for spelling in [POOL, POOL_REFRESHED, VAULT_INVENTORY] {
        assert!(
            spellings.contains(&spelling),
            "{spelling} is an approved spelling: {approved}"
        );
    }
}

/// An admitted spelling goes on past the allowlist to resolve its target,
/// which this store's empty registry cannot: the command still fails, but
/// not with the allowlist's refusal.
fn admitted(store: &Store, spelling: &str) {
    let output = store.run(TARGET, spelling);
    let said = stderr(&output);
    assert!(
        !said.contains(REFUSAL),
        "{spelling} is admitted by the allowlist: {said}"
    );
    assert!(
        !output.status.success(),
        "{spelling} went on to the registry, which this isolated store cannot answer: {said}"
    );
}

#[test]
fn the_pool_reads_and_the_vault_inventory_pass_the_allowlist() {
    admitted(&Store::new("pool"), POOL);
    admitted(&Store::new("pool-refreshed"), POOL_REFRESHED);
    admitted(&Store::new("vault-inventory"), VAULT_INVENTORY);
}

/// The dedicated host the fixture names, as a registry target this store
/// knows, with its SSH identity and trust.
struct Fixture {
    target: String,
    ssh_key_file: PathBuf,
}

impl Fixture {
    fn into_store(store: &Store) -> Self {
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
        std::fs::create_dir_all(store.root.join(".ssh")).unwrap();
        std::fs::copy(
            field("known_hosts_file"),
            store.root.join(".ssh/known_hosts"),
        )
        .unwrap();
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let targets = document["targets"].as_array_mut().unwrap();
        let reader = targets.first_mut().unwrap();
        reader["hostnames"] = json!([hostname()]);
        let host = targets.last_mut().unwrap();
        host["name"] = json!(field("target"));
        host["ssh"] = json!(field("ssh"));
        let registry = store.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        let imported = store
            .command()
            .args(["registry", "import", &registry.to_string_lossy()])
            .output()
            .unwrap();
        assert!(
            imported.status.success(),
            "the fixture host enters the registry: {}",
            String::from_utf8_lossy(&imported.stderr)
        );
        Self {
            target: field("target"),
            ssh_key_file: PathBuf::from(field("ssh_key_file")),
        }
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

#[test]
fn the_pool_of_the_gateway_on_the_dedicated_host_is_read_as_its_own_document() {
    let store = Store::new("live");
    let init = store.command().args(["config", "init"]).output().unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let fixture = Fixture::into_store(&store);
    let mut arguments = vec!["host", "exec", fixture.target.as_str(), "--"];
    arguments.extend(POOL.split(' '));
    let output = store
        .command()
        .args(&arguments)
        .env("STADO_RESOLVER_SSH_KEY_FILE", &fixture.ssh_key_file)
        .output()
        .unwrap();
    std::fs::write(store.root.join("live-pool.stdout"), &output.stdout).unwrap();
    std::fs::write(store.root.join("live-pool.stderr"), &output.stderr).unwrap();
    let said = stderr(&output);
    assert!(
        output.status.success(),
        "the gateway's pool is read on its host: {said}"
    );
    let document: Value = match serde_json::from_slice(&output.stdout) {
        Ok(document) => document,
        Err(error) => panic!(
            "the host answers the pool document ({error}): {}{said}",
            String::from_utf8_lossy(&output.stdout)
        ),
    };
    assert!(
        document.get("subscriptions").is_some_and(Value::is_array),
        "the document lists the pool's members under subscriptions: {document}"
    );
}
