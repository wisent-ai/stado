//! `stado config get` through the real binary against a profile `config init`
//! wrote in a test-owned HOME: a dotted key `config set` wrote reads back as
//! it was written, a resolved key answers, and a key neither resolves nor
//! holds is refused naming the file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Isolated {
    root: PathBuf,
    home: PathBuf,
}

impl Isolated {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/config-get");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir(&root).expect("create a new isolated case directory");
        let home = root.join("home");
        std::fs::create_dir(&home).expect("create the isolated home");
        Self { root, home }
    }

    fn stado(&self, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env_remove("STADO_CONFIG")
            .output()
            .expect("run the real stado")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn text(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stado refused: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).expect("UTF-8 answer").trim().to_string()
}

#[test]
fn a_dotted_key_set_in_the_file_reads_back_as_written() {
    let isolated = Isolated::new("dotted-key");
    text(&isolated.stado(&["config", "init"]));
    // The value is whatever the caller states; the journey states a word, so
    // the answer is compared byte for byte with what was set.
    let ceiling = std::env::var("STADO_CONFIG_GET_TEST_CEILING")
        .expect("STADO_CONFIG_GET_TEST_CEILING states the load ceiling the journey sets and reads back");
    text(&isolated.stado(&["config", "set", "host.scan_load_ceiling", &ceiling]));
    assert_eq!(text(&isolated.stado(&["config", "get", "host.scan_load_ceiling"])), ceiling);
    let document = std::env::var("STADO_CONFIG_GET_TEST_DOCUMENT")
        .expect("STADO_CONFIG_GET_TEST_DOCUMENT states the machine.source_limits JSON the journey sets and reads back");
    text(&isolated.stado(&["config", "set", "machine.source_limits", &document]));
    let read: serde_json::Value =
        serde_json::from_str(&text(&isolated.stado(&["config", "get", "machine.source_limits"]))).expect("JSON");
    assert_eq!(read, serde_json::from_str::<serde_json::Value>(&document).expect("JSON"));
}

#[test]
fn a_resolved_key_answers_and_an_unknown_key_is_refused_naming_the_file() {
    let isolated = Isolated::new("resolved-and-unknown");
    text(&isolated.stado(&["config", "init"]));
    let shown: serde_json::Value =
        serde_json::from_str(&text(&isolated.stado(&["config", "show", "--json"]))).expect("JSON");
    let file = shown["file"].as_str().expect("config show names the file");
    let project = shown["resolved"]["project"].as_str().expect("config init declares a project");
    assert_eq!(text(&isolated.stado(&["config", "get", "project"])), project);
    let refused = isolated.stado(&["config", "get", "nobody.declares.this"]);
    assert!(!refused.status.success(), "an unknown key must be refused");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains(file), "the refusal names the file: {stderr}");
    assert!(stderr.contains("holds no nobody.declares.this"), "the refusal names the key: {stderr}");
}
