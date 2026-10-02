//! Exercise configuration initialization through the real binary with a test-owned HOME.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Isolated {
    root: PathBuf,
    home: PathBuf,
}

impl Isolated {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("config-init");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir(&root).expect("create a new isolated case directory");
        let home = root.join("home");
        std::fs::create_dir(&home).expect("create the isolated home");
        Self { root, home }
    }

    fn default_profile(&self) -> PathBuf {
        self.home.join(".stado/config.json")
    }

    fn run(&self, selected: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(["config", "init"])
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env_remove("STADO_CONFIG");
        if let Some(selected) = selected {
            command.env("STADO_CONFIG", selected);
        }
        command
            .output()
            .expect("run the real configuration initializer")
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn expect_success(output: &Output) {
    assert!(
        output.status.success(),
        "config init failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_explicit_profile_does_not_overwrite_or_refuse_an_existing_default() {
    let isolated = Isolated::new("explicit-profile");
    let default = isolated.default_profile();
    std::fs::create_dir_all(default.parent().expect("default profile parent"))
        .expect("create the default profile directory");
    let sentinel = b"{\"operatorData\":\"preserve this profile\"}\n";
    std::fs::write(&default, sentinel).expect("write the test-owned default profile");
    let selected = isolated.root.join("profiles/database.json");
    let output = isolated.run(Some(selected.to_str().expect("UTF-8 profile path")));
    expect_success(&output);
    let profile: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&selected).expect("read the explicitly selected profile"),
    )
    .expect("parse the initialized profile");
    assert!(
        profile.is_object(),
        "the initialized profile is not a document"
    );
    assert_eq!(
        std::fs::read(&default).expect("read the default profile"),
        sentinel
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        selected.to_str().unwrap()
    );
}

#[test]
fn an_existing_explicit_profile_is_refused_without_creating_a_default() {
    let isolated = Isolated::new("existing-profile");
    let selected = isolated.root.join("database.json");
    let sentinel = b"{\"operatorData\":\"preserve this profile\"}\n";
    std::fs::write(&selected, sentinel).expect("write the existing profile");
    let output = isolated.run(Some(selected.to_str().expect("UTF-8 profile path")));
    assert!(
        !output.status.success(),
        "config init overwrote an existing profile"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("config file already exists"), "{stderr}");
    assert!(stderr.contains(selected.to_str().unwrap()), "{stderr}");
    assert_eq!(
        std::fs::read(&selected).expect("read the existing profile"),
        sentinel
    );
    assert!(!isolated.default_profile().exists());
    assert!(!isolated.home.join(".stado/local-storage").exists());
}

#[test]
fn a_home_relative_selector_uses_the_same_expansion_as_configuration_reads() {
    let isolated = Isolated::new("home-relative-profile");
    let output = isolated.run(Some(" ~/profiles/database.json "));
    expect_success(&output);
    let selected = isolated.home.join("profiles/database.json");
    assert!(
        selected.is_file(),
        "the selected home-relative profile was not created"
    );
    assert!(!isolated.default_profile().exists());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        selected.to_str().unwrap()
    );
}

#[test]
fn initialization_without_a_selector_uses_the_default_profile() {
    let isolated = Isolated::new("default-profile");
    let output = isolated.run(None);
    expect_success(&output);
    assert!(isolated.default_profile().is_file());
    assert!(isolated
        .home
        .join(".stado/local-storage/registry.json")
        .is_file());
}
