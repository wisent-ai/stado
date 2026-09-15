//! Publish real release objects without borrowing or rewriting another identity.

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use serde_json::{json, Value};
use stado::remote::object_store::ObjectRef;

use super::{
    host::{said, IsolatedHost, TARGET},
    servers::Server,
};

const ITEM: &str = "stado-release-publisher";
const PUBLISHER: &str = "isolated-release-publisher";
const VERIFIER: &str = stado::config::RELEASE_API_VERIFIER_CONSUMER;
const CONTROL_PLANE: &str = "isolated-control-plane";
const CONTENT: &[u8] = b"release bytes owned by this isolated publication journey";

struct Publication {
    api: Server,
    _broker: Server,
    host: IsolatedHost,
    config: PathBuf,
    source: PathBuf,
}

fn mint(host: &IsolatedHost, consumer: &str, capability: &str) -> PathBuf {
    let output = host.run(
        &[
            "credentials",
            "token",
            "mint",
            consumer,
            "--host",
            TARGET,
            "--capabilities",
            capability,
            "--audience",
            consumer,
            "--token-file-name",
            consumer,
            "--json",
        ],
        None,
    );
    assert!(output.status.success(), "{}", said(&output));
    host.home.join(".stado").join(consumer)
}

impl Publication {
    fn new() -> Self {
        let host = IsolatedHost::new(true);
        super::put(&host);
        let payload = json!({
            "schema": "skarbiec.item.v2", "kind": "token",
            "fields": {"token": "isolated-product-publication-bearer"}, "context": {},
        });
        let seeded = host.run(
            &[
                "credentials",
                "item",
                "put",
                ITEM,
                "--host",
                TARGET,
                "--type",
                "token",
                "--json",
            ],
            Some(&payload.to_string()),
        );
        assert!(seeded.status.success(), "{}", said(&seeded));

        // The other two identities CAN read the bearer. Falling back to either
        // would therefore turn the publication refusals below into real writes.
        let control = mint(&host, CONTROL_PLANE, &format!("read:{ITEM}#token"));
        let verifier = mint(&host, VERIFIER, &format!("read:{ITEM}#token"));
        let publisher = mint(&host, PUBLISHER, &format!("read:{}#password", super::ITEM));
        let broker = Server::broker(&host);
        let config = host.home.join(".config/stado/config.json");
        let mut document: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        document["secrets"]["skarbiec"] = json!({
            "url": broker.url, "consumer": CONTROL_PLANE, "token_file": control,
            "vault_file": host.vault_path(),
        });
        document["release"] = json!({"publisher_skarbiec": {
            "url": broker.url, "consumer": PUBLISHER, "token_file": publisher,
        }});
        let publishers = stado::config::ACTIVE_RELEASE_PUBLISHERS.iter().map(|product| (
            (*product).to_string(),
            json!({"item": format!("{product}-release-publisher"), "prefix": format!("{product}/")}),
        )).collect::<serde_json::Map<String, Value>>();
        document["release_api"] = json!({
            "publishers": publishers,
            "skarbiec": {"url": broker.url, "consumer": VERIFIER, "token_file": verifier},
        });
        fs::write(&config, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        let api = Server::object_api(&host);
        let source = host.home.join("publication.bin");
        fs::write(&source, CONTENT).unwrap();
        Self {
            api,
            _broker: broker,
            host,
            config,
            source,
        }
    }

    fn object(&self, name: &str) -> ObjectRef {
        ObjectRef::new("releases", &format!("stado/isolated-publication/{name}")).unwrap()
    }

    fn persisted(&self, name: &str) -> PathBuf {
        self.host.storage.join(self.object(name).storage_path())
    }

    fn publish(&self, name: &str) -> Output {
        let uri = self.object(name).to_string();
        let output = self
            .host
            .command()
            .env("STADO_CONFIG", &self.config)
            .env("STADO_API_URL", &self.api.url)
            .args([
                "storage",
                "put",
                &uri,
                self.source.to_str().unwrap(),
                "--json",
            ])
            .output()
            .unwrap();
        println!("stado storage put {uri}\n{}", said(&output));
        output
    }

    fn authorize(&self) {
        let token = self.host.home.join(".stado").join(PUBLISHER);
        let output = self.host.run(
            &[
                "credentials",
                "grant",
                "item-read",
                PUBLISHER,
                ITEM,
                "--host",
                TARGET,
                "--field",
                "token",
                "--token-file",
                token.to_str().unwrap(),
                "--json",
            ],
            None,
        );
        assert!(output.status.success(), "{}", said(&output));
    }
}

#[test]
fn publication_uses_its_own_grant_and_cannot_restore_a_revoked_permission() {
    let publication = Publication::new();
    let before = publication.host.vault_bytes();
    let refused = publication.publish("first.bin");
    assert!(!refused.status.success(), "{}", said(&refused));
    assert_eq!(super::report(&refused)["error_code"], "auth");
    assert_eq!(publication.host.vault_bytes(), before);
    assert!(!publication.persisted("first.bin").exists());

    publication.authorize();
    let authorized = publication.host.vault_bytes();
    let published = publication.publish("first.bin");
    assert!(published.status.success(), "{}", said(&published));
    assert_eq!(
        fs::read(publication.persisted("first.bin")).unwrap(),
        CONTENT
    );
    assert_eq!(publication.host.vault_bytes(), authorized);

    let revoked = publication
        .host
        .broker_command()
        .args(["grant", "revoke", PUBLISHER])
        .output()
        .unwrap();
    assert!(revoked.status.success(), "{}", said(&revoked));
    let revoked_state = publication.host.vault_bytes();
    let refused = publication.publish("after-revocation.bin");
    assert!(!refused.status.success(), "{}", said(&refused));
    assert_eq!(super::report(&refused)["error_code"], "auth");
    assert_eq!(publication.host.vault_bytes(), revoked_state);
    assert!(!publication.persisted("after-revocation.bin").exists());
    assert_eq!(
        fs::read(publication.persisted("first.bin")).unwrap(),
        CONTENT
    );
}

#[test]
fn a_publisher_cannot_use_the_release_verifiers_identity() {
    let publication = Publication::new();
    let mut document: Value =
        serde_json::from_slice(&fs::read(&publication.config).unwrap()).unwrap();
    document["release"]["publisher_skarbiec"] = document["release_api"]["skarbiec"].clone();
    fs::write(
        &publication.config,
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    let before = publication.host.vault_bytes();
    let refused = publication.publish("verifier.bin");
    assert!(!refused.status.success(), "{}", said(&refused));
    let refusal = super::report(&refused);
    assert_eq!(refusal["error_code"], "refused");
    assert_eq!(refusal["retryable"], false);
    assert_eq!(publication.host.vault_bytes(), before);
    assert!(!publication.persisted("verifier.bin").exists());
}
