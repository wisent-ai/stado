//! How the operator wants to be asked, through the real `stado` binary
//! against one isolated deployment: a local store holding a registry.
//!
//! With no choice recorded, `alerts preferences` and `alerts send` refuse and
//! name the command that records one. A channel Stado cannot page through and
//! a channel named twice are refused before anything is written. A recorded
//! choice is the registry's `operator_contact.channels`, in the operator's
//! order, and every malformed form of that field is refused with what is
//! wrong. `alerts send` with a chosen channel whose material is absent names
//! the operator's choice and the missing material instead of sending nowhere.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/alerts-operator-contact")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        let deployment = Self { root };
        let registry: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        deployment.write_registry(&registry);
        deployment
    }

    fn registry_path(&self) -> PathBuf {
        self.root.join("store/ecosystem/probierz/registry.json")
    }

    fn write_registry(&self, registry: &Value) {
        fs::write(
            self.registry_path(),
            serde_json::to_vec_pretty(registry).unwrap(),
        )
        .unwrap();
    }

    fn registry(&self) -> Value {
        serde_json::from_slice(&fs::read(self.registry_path()).unwrap()).unwrap()
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn refused(output: &Output, cause: &str) {
    let text = said(output);
    assert!(
        !output.status.success(),
        "expected a refusal naming {cause:?}, got success: {text}"
    );
    assert!(
        text.contains(cause),
        "the refusal does not name {cause:?}: {text}"
    );
}

#[test]
fn the_operator_chooses_how_he_is_asked_and_every_wrong_choice_names_its_cause() {
    let deployment = Deployment::start();

    let none = deployment.stado(&["alerts", "preferences"]);
    refused(&none, "the operator has chosen no contact channel");
    refused(&none, "stado alerts preferences set --channel");
    let unpaged = deployment.stado(&["alerts", "send", "a test page"]);
    refused(&unpaged, "the operator has chosen no contact channel");

    let before = deployment.registry();
    refused(
        &deployment.stado(&["alerts", "preferences", "set", "--channel", "pigeon"]),
        "\"pigeon\" is not a channel Stado pages through",
    );
    refused(
        &deployment.stado(&[
            "alerts",
            "preferences",
            "set",
            "--channel",
            "resend",
            "--channel",
            "resend",
        ]),
        "--channel resend is named twice",
    );
    assert_eq!(
        deployment.registry(),
        before,
        "a refused choice changed the registry"
    );

    let chosen = deployment.stado(&[
        "alerts",
        "preferences",
        "set",
        "--channel",
        "slack",
        "--channel",
        "resend",
    ]);
    assert!(chosen.status.success(), "{}", said(&chosen));
    assert_eq!(
        deployment.registry()["operator_contact"],
        json!({ "channels": ["slack", "resend"] })
    );
    let shown = deployment.stado(&["alerts", "preferences", "show", "--json"]);
    assert!(shown.status.success(), "{}", said(&shown));
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown, json!({ "channels": ["slack", "resend"] }));

    let paged = deployment.stado(&["alerts", "send", "a test page"]);
    refused(&paged, "the operator chose [slack,resend]");
    refused(&paged, "slack-configuration");

    for (field, cause) in [
        (
            json!({}),
            "the registry's operator_contact has no channels list",
        ),
        (
            json!({ "channels": "resend" }),
            "the registry's operator_contact has no channels list",
        ),
        (
            json!({ "channels": [] }),
            "the registry's operator_contact.channels is empty",
        ),
        (
            json!({ "channels": ["pigeon"] }),
            "names \"pigeon\", a channel Stado cannot page through",
        ),
        (
            json!({ "channels": [true] }),
            "holds true, which is not a channel name",
        ),
    ] {
        let mut registry = deployment.registry();
        registry["operator_contact"] = field;
        deployment.write_registry(&registry);
        refused(&deployment.stado(&["alerts", "preferences", "show"]), cause);
    }

    fs::remove_dir_all(&deployment.root).unwrap();
}
