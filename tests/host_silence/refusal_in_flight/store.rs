//! A real fleet store for the host-silence journeys: one deployment with a
//! real vault (`../../storage/vault_consultation/fixture.rs`) and two real
//! object APIs on it, the store the resolver reads and the service its
//! adapter relays to. The store is asked once so it is known to answer, and
//! is stopped with SIGSTOP when the journey wants it to answer nothing.
#[path = "../../storage/vault_consultation/fixture.rs"]
// The store journeys use the deployment's services, not its vault probes.
#[allow(dead_code)]
mod fixture;

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const STORE: &str = "store";
const RELAYED: &str = "service";

pub(crate) struct Store {
    deployment: fixture::Deployment,
    pub(crate) address: String,
    pub(crate) service: String,
    /// The namespace item's token, which the store authorizes.
    pub(crate) token_file: PathBuf,
}

/// The namespace the resolver reads the store under.
pub(crate) const NAMESPACE: &str = fixture::NAMESPACE;

impl Store {
    /// Start the deployment and write the resolver's bearer to `token_file`.
    pub(crate) fn serve(token_file: PathBuf) -> Self {
        let mut deployment = fixture::Deployment::start("host-silence-store");
        let token = format!("store-{}", uuid::Uuid::new_v4().simple());
        let mut init = deployment.skarbiec();
        init.args(["init", "host-silence-owner"]);
        deployment.must("skarbiec init", init);
        deployment.set_token(&token);
        let grant = deployment.root.join("verifier.token");
        deployment.issue_grant(&grant);
        let mut vault = deployment.skarbiec();
        vault.args(["serve", "--port", "0"]);
        let vault_address = deployment.spawn("skarbiec", vault, "listening on http://");
        let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
            .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
        let mut init = deployment.stado();
        init.args(["config", "init"]);
        deployment.must("stado config init", init);
        let mut set = deployment.stado();
        set.args(["config", "set", "dashboard.request_limits", &limits]);
        deployment.must("stado config set dashboard.request_limits", set);
        let address = object_api(&mut deployment, STORE, &vault_address, &grant);
        let service = object_api(&mut deployment, RELAYED, &vault_address, &grant);
        std::fs::write(&token_file, &token).expect("write the resolver's store bearer");
        fixture::owner_only(&token_file);
        let runtime = tokio::runtime::Runtime::new().expect("start a runtime for one request");
        let (status, body) =
            runtime.block_on(fixture::get_object(format!("http://{address}"), token));
        assert_eq!(
            status,
            reqwest::StatusCode::NOT_FOUND,
            "the live store authorizes the bearer and answers an absent object: {body}"
        );
        Self {
            deployment,
            address,
            service,
            token_file,
        }
    }

    /// Stop the store: from now on it accepts connections and answers none.
    pub(crate) fn hang(&self) {
        self.deployment.hang(STORE);
    }

    /// What the report keeps of the store.
    pub(crate) fn describe(&self) -> Value {
        json!({
            "address": self.address,
            "service": self.service,
            "evidence": self.deployment.root,
        })
    }
}

/// Start one real object API of `deployment` as `label`, verifying its
/// namespace against the vault at `vault`, and answer its address.
fn object_api(
    deployment: &mut fixture::Deployment,
    label: &str,
    vault: &str,
    grant: &Path,
) -> String {
    let namespaces = json!({
        fixture::NAMESPACE: {"item": fixture::ITEM, "prefix_policies": [{"prefix": "probe/", "actions": ["get"]}]}
    });
    let loopback = Ipv4Addr::LOCALHOST.to_string();
    let mut api = deployment.stado();
    api.env("WC_SKARBIEC_URL", format!("http://{vault}"))
        .env("WC_SKARBIEC_CONSUMER", fixture::CONSUMER)
        .env("WC_SKARBIEC_TOKEN_FILE", grant)
        .env("WC_OBJECT_API_NAMESPACES", namespaces.to_string())
        .args([
            "serve",
            "--api",
            "--bind",
            &loopback,
            "--port",
            "0",
            "--api-local-store",
        ])
        .arg(deployment.root.join(format!("{label}-objects")));
    deployment.spawn(label, api, "[dashboard] listening on http://")
}
