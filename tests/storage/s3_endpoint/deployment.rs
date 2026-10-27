//! One isolated deployment for the S3 endpoint journey: a real Skarbiec
//! (`SKARBIEC` names the binary under test) serving a vault of its own, the
//! real `stado serve --api` over a local store verifying one namespace
//! against that vault, and an AWS CLI configuration of its own, so neither
//! the operator's vault nor his AWS profile is read. Every command, its exit
//! status and its stderr stay beside report.json; no secret value does. The
//! commands it runs are built in [`super::commands`].
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Stado's one vault identity; `Client::stado()` refuses every other name.
pub(crate) const CONSUMER: &str = "stado";

/// The role whose vault item holds the namespace's S3 key pair, for a Stado
/// that keeps its queue on this endpoint as an S3 API store.
pub(crate) const KEYS_ROLE: &str = "s3-endpoint-probe-keys";

/// The vault item that plays [`KEYS_ROLE`].
const KEYS_ITEM: &str = "s3-endpoint-probe-keys";

/// The object API's declared actions, read where the product declares them.
const ACTIONS: &str = include_str!("../../../stado-rs/src/config/boundaries/api-actions.json");

/// Take every permission from group and others, the way the keyring and a
/// bearer file have to be held.
pub fn owner_only(path: &Path) {
    let status = Command::new("chmod").arg("go-rwx").arg(path).status().unwrap();
    assert!(status.success(), "chmod go-rwx {}", path.display());
}

/// Every action the object API declares, so the whole namespace is granted
/// all of them: a queue kept on the endpoint writes anywhere in it.
fn object_actions() -> Value {
    let declared: Value = serde_json::from_str(ACTIONS).unwrap();
    declared["object"].clone()
}

pub struct Deployment {
    pub root: PathBuf,
    pub(crate) keyring: PathBuf,
    pub report: Value,
    pub(crate) children: Vec<Child>,
    pub origin: String,
    pub namespace: String,
    pub bearer: String,
    /// The vault's origin, and the file holding Stado's grant to it.
    pub(crate) vault: String,
    pub(crate) token_file: PathBuf,
}

impl Deployment {
    /// Start the vault and the object API serving `namespace`, whose bearer
    /// the vault item `item` holds.
    pub fn start(namespace: &str, item: &str) -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let repository = manifest.parent().unwrap().to_path_buf();
        let run = uuid::Uuid::new_v4().to_string();
        let root = repository.join(".build/storage-s3-endpoint").join(run);
        // gpg-agent's socket lives in the keyring and a Unix socket path is
        // short on macOS, so the keyring sits at a short path of its own.
        let keyring = repository.join(".build/g").join(std::process::id().to_string());
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        fs::create_dir_all(root.join("aws")).unwrap();
        fs::create_dir_all(&keyring).unwrap();
        owner_only(&keyring);
        let mut git = Command::new("git");
        git.current_dir(&repository).args(["rev-parse", "HEAD"]);
        let revision = git.output().unwrap();
        let token_file = root.join("verifier.token");
        let mut deployment = Self {
            root,
            keyring,
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "commands": [],
                "outcome": "failed",
            }),
            children: Vec::new(),
            origin: String::new(),
            namespace: namespace.to_string(),
            bearer: format!("s3-{}", uuid::Uuid::new_v4().simple()),
            vault: String::new(),
            token_file,
        };
        deployment.vault_serving(item);
        deployment.object_api(item);
        deployment
    }

    /// The vault: the namespace's bearer, the key pair the queue reads by
    /// role, and Stado's grant to read both.
    fn vault_serving(&mut self, item: &str) {
        let mut init = self.skarbiec();
        init.args(["init", "s3-endpoint-owner"]);
        self.must("skarbiec init", init);
        let mut set = self.skarbiec();
        set.args(["set", item, "--type", "token", &format!("token={}", self.bearer)]);
        self.must("skarbiec set (token)", set);
        let mut keys = self.skarbiec();
        let tag = format!("stado:role:{KEYS_ROLE}");
        keys.args(["set", KEYS_ITEM, "--type", "access-key", "--tags", &tag])
            .arg(format!("access_key_id={}", self.namespace))
            .arg(format!("secret_access_key={}", self.bearer));
        self.must("skarbiec set (access-key)", keys);
        let reads = [
            format!("read:{item}#token"),
            format!("read:{KEYS_ITEM}#access_key_id"),
            format!("read:{KEYS_ITEM}#secret_access_key"),
        ];
        let mut grant = self.skarbiec();
        grant.args(["grant", "issue", CONSUMER, "--capabilities", &reads.join(",")]);
        let answer = self.must("skarbiec grant issue", grant);
        let issued: Value = serde_json::from_str(&answer).unwrap();
        let token = issued["token"].as_str().expect("grant issue answers the bearer");
        fs::write(&self.token_file, token).unwrap();
        owner_only(&self.token_file);
        let mut vault = self.skarbiec();
        vault.args(["serve", "--port", "0"]);
        let address = self.spawn("skarbiec", vault, "listening on http://");
        self.vault = format!("http://{address}");
    }

    /// The object API over the deployment's local store, serving the
    /// namespace with every declared action.
    fn object_api(&mut self, item: &str) {
        let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
            .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
        let mut init = self.local();
        init.args(["config", "init"]);
        self.must("stado config init", init);
        let mut set = self.local();
        set.args(["config", "set", "dashboard.request_limits", &limits]);
        self.must("stado config set dashboard.request_limits", set);
        let declared = json!({"item": item, "actions": object_actions()});
        let namespaces = json!({ &self.namespace: declared });
        let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
        let mut api = self.vault_client();
        api.env("WC_OBJECT_API_NAMESPACES", namespaces.to_string())
            .args(["serve", "--api", "--bind", &loopback, "--port", "0"])
            .arg("--api-local-store")
            .arg(self.store());
        let address = self.spawn("stado", api, "[dashboard] listening on http://");
        self.origin = format!("http://{address}");
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        for child in &mut self.children {
            if let Err(error) = child.kill() {
                eprintln!("stopping a service of the deployment failed: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("waiting for a service of the deployment failed: {error}");
            }
        }
        let mut agents = Command::new("gpgconf");
        agents.arg("--homedir").arg(&self.keyring).args(["--kill", "all"]);
        if let Err(error) = agents.status() {
            eprintln!("stopping the keyring's agents failed: {error}");
        }
        if let Err(error) = fs::remove_dir_all(&self.keyring) {
            eprintln!("removing the keyring {} failed: {error}", self.keyring.display());
        }
        self.save();
        eprintln!("s3 endpoint evidence: {}", self.root.display());
    }
}
