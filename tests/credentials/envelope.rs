//! `stado credentials get` through the real binary against a test-owned file
//! store: a field whose stored text is an encrypted `{"v":…,"c":…}` envelope
//! is refused with the item, the field and the version, and a plain value
//! next to it is still printed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use base64::Engine as _;

struct Isolated {
    root: PathBuf,
}

impl Isolated {
    fn new(case: &str, items: serde_json::Value) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("credentials-envelope");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir(&root).expect("create a new isolated case directory");
        std::fs::create_dir(root.join("home")).expect("create the isolated home");
        let store = root.join("store.json");
        std::fs::write(&store, items.to_string()).expect("write the file store");
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o600))
            .expect("make the file store owner-only");
        let config = serde_json::json!({
            "credentials": {"store": format!("file://{}", store.display())}
        });
        std::fs::write(root.join("config.json"), config.to_string()).expect("write the config");
        Self { root }
    }

    fn get(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["credentials", "get"])
            .args(args)
            .current_dir(&self.root)
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("config.json"))
            .env_remove("STADO_CREDENTIALS_STORE")
            .output()
            .expect("run the real stado binary")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn envelope() -> String {
    base64::engine::general_purpose::STANDARD.encode(r#"{"v":"v2","c":"QUJDREVGR0g="}"#)
}

#[test]
fn a_stored_envelope_is_refused_and_a_plain_value_is_printed() {
    let isolated = Isolated::new(
        "field",
        serde_json::json!({
            "ENCRYPTED_URL": {"value": envelope()},
            "PLAIN_URL": {"value": "https://project.example"}
        }),
    );

    let refused = isolated.get(&["ENCRYPTED_URL", "--field", "value"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "an envelope was accepted: {stderr}");
    assert!(refused.stdout.is_empty(), "an envelope was printed");
    for expected in ["ENCRYPTED_URL", "value", "v2 ciphertext envelope"] {
        assert!(stderr.contains(expected), "refusal lacks {expected:?}: {stderr}");
    }

    let printed = isolated.get(&["PLAIN_URL", "--field", "value"]);
    assert!(
        printed.status.success(),
        "a plain value was refused: {}",
        String::from_utf8_lossy(&printed.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&printed.stdout).trim_end(), "https://project.example");
}

#[test]
fn a_whole_item_read_refuses_an_envelope_value() {
    let isolated = Isolated::new("item", serde_json::json!({"ENCRYPTED_URL": {"value": envelope()}}));
    let refused = isolated.get(&["ENCRYPTED_URL"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "an envelope was accepted: {stderr}");
    assert!(stderr.contains("v2 ciphertext envelope"), "{stderr}");
}
