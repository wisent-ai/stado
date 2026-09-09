//! What the publisher runner lifecycle refuses before it reaches a machine.
//!
//! Each case drives the built binary against an isolated canonical registry
//! this module wrote, holding a host whose ssh destination RFC 2606 reserves,
//! so no fleet machine is contacted and no GitHub repository is touched. Every
//! sentence asserted here was copied from a live run.
//!
//! [`crate::developer_id`] holds the one case that issues a certificate, which
//! needs a person at an Apple prompt and is gated.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};
use stado::targets::REGISTRY_SCHEMA_VERSION;

/// The declared profile under judgement, and the declaration a refusal must
/// send the operator to.
pub(crate) const PROFILE: &str = "publisher";
const PROFILE_DECLARATION: &str = "stado-rs/data/runner-profiles.json";

/// A profile name the shipped declaration does not carry.
const UNDECLARED_PROFILE: &str = "no-such-runner-profile";

/// A host the seeded registry does not declare.
const UNDECLARED_HOST: &str = "no-such-publisher-host";

/// The declared macOS host these cases name. Its ssh destination cannot
/// resolve, so the lifecycle stops at the brokered key and never logs in.
const MACOS_HOST: &str = "unreachable-publisher-host";

struct Fixture {
    home: tempfile::TempDir,
    store: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("an isolated home exists");
        let store = home.path().join("store");
        std::fs::create_dir_all(&store).expect("the isolated store exists");
        let document = json!({
            "schema_version": REGISTRY_SCHEMA_VERSION,
            "targets": [{
                "name": MACOS_HOST,
                "kind": "local",
                "ssh": format!("nobody@{MACOS_HOST}.invalid"),
                "release_platform": "darwin-arm64",
                "hostnames": [format!("{MACOS_HOST}.local")],
            }],
            "coordinators": [],
        });
        std::fs::write(
            store.join("registry.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(&document).expect("serialize the registry")
            ),
        )
        .expect("seed the isolated registry");
        Self { home, store }
    }

    fn stado(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env("HOME", self.home.path())
            .env("STADO_CONFIG", self.home.path().join("no-such-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.store)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .output()
            .expect("the built Stado binary starts")
    }

    /// The runner state directory the lifecycle creates on a host it reached.
    /// Its absence is how "no runner was installed" is read here.
    fn runner_state(&self) -> PathBuf {
        self.home.path().join("actions-runner-stado-publisher")
    }
}

pub(crate) fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// The typed refusal document a `--json` lifecycle command printed.
fn refusal(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("expected one JSON refusal ({error}):\n{}", said(output)))
}

/// A host the registry does not declare is refused as a missing declaration,
/// naming the registry that would carry it — not as an unreachable machine, and
/// not as a credential problem.
#[test]
fn installing_on_an_undeclared_host_is_refused_by_the_registry_and_installs_nothing() {
    let fixture = Fixture::new();
    let refused = fixture.stado(&[
        "runner",
        "install",
        UNDECLARED_HOST,
        "--profile",
        PROFILE,
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    let document = refusal(&refused);
    assert_eq!(document["status"], "error");
    assert_eq!(document["failure_point"], "cli.runner.install");
    assert_eq!(document["retryable"], false);
    assert_eq!(
        document["message"],
        format!(
            "{UNDECLARED_HOST} declares no host target; add it to the canonical fleet registry"
        ),
    );
    assert!(
        !fixture.runner_state().exists(),
        "a refused install left runner state at {}",
        fixture.runner_state().display(),
    );
}

/// The same name, read instead of installed: the refusal is the same sentence
/// under the read's own failure point, so an operator is not told the two
/// commands disagree about which hosts exist.
#[test]
fn reading_an_undeclared_host_is_refused_with_the_same_sentence_under_the_read() {
    let fixture = Fixture::new();
    let refused = fixture.stado(&[
        "runner",
        "status",
        UNDECLARED_HOST,
        "--profile",
        PROFILE,
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    let document = refusal(&refused);
    assert_eq!(document["failure_point"], "cli.runner.status");
    assert_eq!(
        document["message"],
        format!(
            "{UNDECLARED_HOST} declares no host target; add it to the canonical fleet registry"
        ),
    );
}

/// A profile the shipped declaration does not carry is refused by name with the
/// file that declares profiles, so adding one is a declaration change and never
/// another flag.
#[test]
fn an_undeclared_profile_is_refused_by_name_with_the_declaration_that_carries_profiles() {
    let fixture = Fixture::new();
    let refused = fixture.stado(&[
        "runner",
        "install",
        MACOS_HOST,
        "--profile",
        UNDECLARED_PROFILE,
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert_eq!(
        refusal(&refused)["message"],
        format!(
            "runner profile '{UNDECLARED_PROFILE}' is not declared; \
             add it to {PROFILE_DECLARATION}"
        ),
    );
    assert!(!fixture.runner_state().exists());
}

/// A declared macOS host the product cannot reach: the read stops at the
/// brokered host key, says which consumer wanted which field of which item, and
/// installs nothing. This is the wall in front of every real publisher host, so
/// it is the one an operator meets when a key is missing.
#[test]
fn a_declared_host_whose_key_cannot_be_read_names_the_credential_and_installs_nothing() {
    let fixture = Fixture::new();
    let refused = fixture.stado(&["runner", "status", MACOS_HOST, "--profile", PROFILE, "--json"]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    let document = refusal(&refused);
    assert_eq!(document["failure_point"], "cli.runner.status");
    assert_eq!(document["error_code"], "not_found");
    let message = document["message"]
        .as_str()
        .expect("the refusal carries one sentence");
    assert!(
        message.contains(&format!("stado-ssh-{MACOS_HOST}"))
            && message.contains("private_key")
            && message.contains("local-operator"),
        "the refusal must name the consumer, the item and the field: {document}",
    );
    assert!(!fixture.runner_state().exists());
}
