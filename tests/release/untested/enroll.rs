//! A required platform that declares no post-build tests is named before
//! anything is built: `stado release catalog enroll` refuses its manifest
//! with the platform and the sentence the qualification reads, and takes no
//! fleet step for it, while the same manifest with a declared test passes
//! that check.
//!
//! One isolated home per case: a local store, a product checkout written
//! here and the real `stado` executable. The manifest takes its schema
//! version from Stado's own manifest.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "release-untested-enroll-test";
const MANIFEST: &str = ".wisent-release.json";
const UNTESTED: &str = "required platform linux-amd64 declares no post-build tests";

struct Deployment {
    repository: PathBuf,
    root: PathBuf,
}

impl Deployment {
    fn start(case: &str) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/release-untested-enroll")
            .join(format!("{case}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        fs::create_dir_all(root.join("store")).expect("create the local store");
        Self { repository, root }
    }

    fn enroll(&self, checkout: &Path) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .args(["release", "catalog", "enroll"])
            .arg(checkout)
            .output()
            .expect("run the real stado");
        fs::write(self.root.join("enroll.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }

    /// A product checkout whose one required platform declares `tests`.
    fn checkout(&self, tests: Value) -> PathBuf {
        let checkout = self.root.join("checkout");
        fs::create_dir_all(&checkout).expect("create the checkout");
        let own: Value = serde_json::from_slice(
            &fs::read(self.repository.join(MANIFEST)).expect("read Stado's own manifest"),
        )
        .expect("Stado's own manifest is JSON");
        fs::write(checkout.join("VERSION"), "0.1.0\n").expect("write the version");
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {"kind": "text", "path": "VERSION"},
            "platforms": {
                "linux-amd64": {
                    "runner_platform": "linux-amd64",
                    "quality": [],
                    "build": {"argv": ["cat", "VERSION"]},
                    "tests": tests,
                    "stage": {"VERSION": "VERSION"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        fs::write(
            checkout.join(MANIFEST),
            serde_json::to_vec_pretty(&manifest).expect("encode the manifest"),
        )
        .expect("write the manifest");
        checkout
    }

    /// Every file the local store holds, by its path below the store.
    fn stored(&self) -> Vec<String> {
        let store = self.root.join("store");
        let mut found = Vec::new();
        let mut pending = vec![store.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read the local store") {
                let path = entry.expect("read a store entry").path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(
                        path.strip_prefix(&store)
                            .expect("a store entry lies in the store")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        found
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_required_platform_without_tests_is_refused_at_enroll_before_any_fleet_step() {
    let deployment = Deployment::start("untested");
    let checkout = deployment.checkout(json!([]));
    let output = deployment.enroll(&checkout);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{said}");
    assert!(said.contains(PRODUCT) && said.contains(UNTESTED), "{said}");
    assert!(said.contains("awaiting_tests"), "{said}");
    let stored = deployment.stored();
    assert!(
        stored.iter().all(|path| !path.contains("release-catalog")),
        "a refused manifest left a catalog entry: {stored:?}"
    );
}

#[test]
fn a_required_platform_with_a_test_passes_the_check() {
    let deployment = Deployment::start("tested");
    let checkout = deployment.checkout(json!([{"name": "version", "argv": ["cat", "VERSION"]}]));
    let output = deployment.enroll(&checkout);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !said.contains("declares no post-build tests"),
        "a tested platform was refused as untested: {said}"
    );
}
