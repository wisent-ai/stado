//! The skrzynka alert channel, through the real `stado` binary and the real
//! `skrzynka` on PATH, against one isolated deployment: a local store holding
//! a registry, Skrzynka's database under the deployment's own home, and
//! Skrzynka's credentials file (absent, so holding no item) in place of a
//! Skarbiec.
//!
//! The operator's choice is the skrzynka channel. With no mailbox declared the
//! page is refused naming the missing keys. With the keys naming a mailbox the
//! isolated organization does not hold, `alerts send` fails, the refusal
//! carries Skrzynka's own answer for that mailbox, and Skrzynka records no
//! outbound row for it.
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Deployment {
    root: PathBuf,
    organization: String,
    /// A mailbox id the isolated organization does not hold. Skrzynka
    /// resolves the mailbox before it reads the destination, so the
    /// destination is never checked or written to.
    mailbox: String,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/alerts-skrzynka")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        let registry: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        fs::write(
            root.join("store/ecosystem/probierz/registry.json"),
            serde_json::to_vec_pretty(&registry).unwrap(),
        )
        .unwrap();
        Self {
            root,
            organization: uuid::Uuid::new_v4().to_string(),
            mailbox: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env(
                "SKRZYNKA_CREDENTIALS_FILE",
                self.root.join("skrzynka-credentials.json"),
            )
            .stdin(Stdio::null());
        command
    }

    fn stado(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_stado"), args)
            .output()
            .unwrap()
    }

    fn paged(&self, message: &str) -> Output {
        self.command(env!("CARGO_BIN_EXE_stado"), &["alerts", "send", message])
            .env("WC_EMAIL_TO", &self.organization)
            .env("WC_ALERT_SKRZYNKA_MAILBOX", &self.mailbox)
            .env("WC_ALERT_SKRZYNKA_ORGANIZATION", &self.organization)
            .output()
            .unwrap()
    }

    fn outbound(&self) -> Output {
        self.command(
            "skrzynka",
            &["--organization", &self.organization, "message", "outbound"],
        )
        .output()
        .unwrap()
    }
}

fn said(output: &Output) -> String {
    format!(
        "exit {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn refused(output: &Output, cause: &str) {
    let text = said(output);
    assert!(
        !output.status.success(),
        "expected a refusal naming {cause:?}, got success:\n{text}"
    );
    assert!(text.contains(cause), "expected {cause:?} in:\n{text}");
}

#[test]
fn a_page_on_the_skrzynka_channel_goes_only_through_skrzynka() {
    let deployment = Deployment::start();
    let chosen = deployment.stado(&["alerts", "preferences", "set", "--channel", "skrzynka"]);
    assert!(chosen.status.success(), "{}", said(&chosen));

    let undeclared = deployment.stado(&["alerts", "send", "an undeclared page"]);
    refused(&undeclared, "skrzynka-configuration");
    refused(&undeclared, "alerts.skrzynka_mailbox");
    refused(&undeclared, "alerts.skrzynka_organization");

    let unheld = deployment.paged("a page from a mailbox nobody holds");
    refused(&unheld, "every alert channel refused this message");
    refused(
        &unheld,
        &format!(
            "skrzynka message send --mailbox {} exited",
            deployment.mailbox
        ),
    );

    let outbound = deployment.outbound();
    assert!(outbound.status.success(), "{}", said(&outbound));
    let rows: Value = serde_json::from_slice(&outbound.stdout).unwrap();
    assert_eq!(
        rows,
        Value::Array(Vec::new()),
        "Skrzynka recorded outbound mail for a mailbox it does not hold"
    );

    fs::remove_dir_all(&deployment.root).unwrap();
}
