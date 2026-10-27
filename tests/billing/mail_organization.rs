//! `stado billing watch` reads provider mail from the Skrzynka organization
//! it is told, and assumes none.
//!
//! One isolated deployment: a local store and a home without Skrzynka. A
//! watch pass without `--mail-organization` is refused before it reads or
//! stores anything, naming the option. A pass with it runs whole against the
//! local store; Skrzynka cannot be reached from that home, so the mail sweep
//! is reported unavailable with the cause — and its query names the
//! organization the operator gave, which is what the sweep would have read;
//! no notice was read, so none was recorded as a defect, and the pass and its
//! stored snapshot both carry that empty register.
//! The window is the caller's (`STADO_TEST_MAIL_DAYS`, refused by name when
//! missing), as the command's own `--mail-days` assumes none. The commands,
//! their exit statuses and outputs are kept in the deployment's
//! `report.json` under the checkout's `.build`.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const ORGANIZATION: &str = "example-organization";

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/billing-mail-organization")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION").expect("declare the committed candidate"),
                "commands": [],
                "outcome": "failed",
            }),
        }
    }

    fn run(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn the_watch_reads_mail_only_from_the_organization_it_is_given() {
    let days = std::env::var("STADO_TEST_MAIL_DAYS")
        .expect("STADO_TEST_MAIL_DAYS: the mail window to read");
    let mut deployment = Deployment::start();
    let init = deployment.run(&["config", "init"]);
    assert!(init.status.success(), "{init:?}");

    let unnamed = deployment.run(&["billing", "watch", "--mail-days", &days, "--json"]);
    assert!(
        !unnamed.status.success(),
        "a pass without an organization ran: {unnamed:?}"
    );
    assert!(
        String::from_utf8_lossy(&unnamed.stderr).contains("--mail-organization"),
        "the refusal names the missing option: {unnamed:?}"
    );

    let named = deployment.run(&[
        "billing",
        "watch",
        "--mail-days",
        &days,
        "--mail-organization",
        ORGANIZATION,
        "--json",
    ]);
    assert!(named.status.success(), "{named:?}");
    let pass: Value =
        serde_json::from_slice(&named.stdout).expect("the pass prints its report as JSON");
    let mail = &pass["mail"];
    assert_eq!(
        mail["status"], "unavailable",
        "Skrzynka is not reachable from this home: {pass}"
    );
    let Some(query) = mail["query"].as_str() else {
        panic!("the sweep names what it would read: {pass}");
    };
    assert!(
        query.starts_with(&format!("skrzynka organization {ORGANIZATION}:"))
            && query.contains(&format!("in the last {days} days")),
        "{pass}"
    );
    assert!(
        mail["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "the cause is said: {pass}"
    );
    // No notice could be read, so none was recorded as a defect; the pass
    // still says so, and the stored snapshot carries the register the next
    // pass reads before it records a notice, so a notice is recorded once.
    assert_eq!(
        pass["notices_recorded"],
        json!([]),
        "an unreadable mailbox records no defect: {pass}"
    );
    assert_eq!(
        pass["snapshot"]["provider_notices_recorded"],
        json!({}),
        "the snapshot carries the recorded-notice register: {pass}"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
