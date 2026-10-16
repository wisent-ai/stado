//! A required platform without post-build tests is refused by the real
//! catalog, build and release commands before it writes fleet state. The
//! retained qualification run binds the native executable to its committed
//! source and records the exact refusals and observed local store.
//!
//! One isolated home per case: a local store, a product checkout written
//! here and the real `stado` executable. The manifest takes its schema
//! version from Stado's own manifest.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null());
        command
    }

    fn enroll(&self, checkout: &Path) -> Output {
        let output = self
            .command()
            .args(["release", "catalog", "enroll"])
            .arg(checkout)
            .output()
            .expect("run the real stado");
        fs::write(self.root.join("enroll.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }

    fn build(&self, checkout: &Path) -> Output {
        let version = fs::read_to_string(checkout.join("VERSION")).expect("read declared version");
        let output = self
            .command()
            .args(["build", "submit", "--source"])
            .arg(checkout)
            .args(["--version", version.trim()])
            .output()
            .expect("run the real build command");
        fs::write(self.root.join("build.stderr"), &output.stderr)
            .expect("keep the build refusal beside the run");
        output
    }

    fn release(&self, checkout: &Path) -> Output {
        let version = fs::read_to_string(checkout.join("VERSION")).expect("read declared version");
        let output = self
            .command()
            .args(["release", "submit", "--source"])
            .arg(checkout)
            .args(["--version", version.trim()])
            .output()
            .expect("run the real release command");
        fs::write(self.root.join("release.stderr"), &output.stderr)
            .expect("keep the release refusal beside the run");
        output
    }

    fn commit(&self, checkout: &Path) {
        for args in [
            &["init", "-q"][..],
            &["add", "."][..],
            &[
                "-c",
                "user.name=Stado Test",
                "-c",
                "user.email=stado-test@example.invalid",
                "commit",
                "-qm",
                "Declare isolated release",
            ][..],
        ] {
            let result = Command::new("git")
                .current_dir(checkout)
                .args(args)
                .output()
                .expect("run Git in the isolated checkout");
            assert!(
                result.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let revision = Command::new("git")
            .current_dir(checkout)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("read fixture source revision");
        assert!(
            revision.status.success(),
            "fixture revision could not be read"
        );
        fs::write(self.root.join("fixture.revision"), revision.stdout)
            .expect("retain exact fixture source revision");
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
#[ignore = "Requires a committed candidate and its exact native build"]
fn build_and_release_refuse_untested_platform_before_fleet_writes() {
    let candidate = std::env::var("STADO_TEST_SOURCE_REVISION")
        .expect("declare the candidate's full source revision");
    let deployment = Deployment::start("build-untested");
    let source = Command::new("git")
        .current_dir(&deployment.repository)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read source revision");
    assert!(source.status.success(), "cannot identify candidate source");
    assert_eq!(String::from_utf8_lossy(&source.stdout).trim(), candidate);
    let clean = Command::new("git")
        .current_dir(&deployment.repository)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .output()
        .expect("read source cleanliness");
    assert!(
        clean.status.success() && clean.stdout.is_empty(),
        "candidate source must be clean and committed"
    );
    let version = deployment
        .command()
        .arg("--version")
        .output()
        .expect("read native build identity");
    let identity = String::from_utf8_lossy(&version.stdout);
    assert!(
        version.status.success() && identity.contains(&format!("(rev {candidate})")),
        "native build is not from {candidate}: {identity}"
    );

    let checkout = deployment.checkout(json!([]));
    deployment.commit(&checkout);
    let output = deployment.build(&checkout);
    let release = deployment.release(&checkout);
    let release_said = String::from_utf8_lossy(&release.stderr);
    let said = String::from_utf8_lossy(&output.stderr);
    let stored = deployment.stored();
    let binary = env!("CARGO_BIN_EXE_stado");
    let report = json!({
        "source_revision": candidate,
        "fixture_revision": fs::read_to_string(deployment.root.join("fixture.revision"))
            .expect("read committed fixture revision").trim(),
        "binary": binary,
        "binary_sha256": hex::encode(Sha256::digest(fs::read(binary).expect("read native binary"))),
        "commands": [
            {"argv": ["stado", "--version"], "exit_status": version.status.code(),
             "stdout": identity.as_ref()},
            {"argv": ["stado", "build", "submit", "--source", checkout.to_string_lossy().as_ref(),
                       "--version", fs::read_to_string(checkout.join("VERSION"))
                           .expect("read declared version").trim()],
             "exit_status": output.status.code(), "stderr": said.as_ref()},
            {"argv": ["stado", "release", "submit", "--source",
                       checkout.to_string_lossy().as_ref(), "--version",
                       fs::read_to_string(checkout.join("VERSION"))
                           .expect("read declared version").trim()],
             "exit_status": release.status.code(), "stderr": release_said.as_ref()}
        ],
        "observed_final_state": {"store_objects": stored}
    });
    fs::write(
        deployment.root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .expect("retain real build refusal");
    assert!(
        !output.status.success(),
        "untested build was queued: {said}"
    );
    assert!(
        said.contains(PRODUCT) && said.contains(UNTESTED) && said.contains("awaiting_tests"),
        "build refusal did not name its cause and qualification consequence: {said}"
    );
    assert!(
        !release.status.success(),
        "untested release was submitted: {release_said}"
    );
    assert!(
        release_said.contains(PRODUCT)
            && release_said.contains(UNTESTED)
            && release_said.contains("awaiting_tests"),
        "release refusal did not name its cause: {release_said}"
    );
    assert!(stored.is_empty(), "refusal wrote fleet state: {stored:?}");
}
