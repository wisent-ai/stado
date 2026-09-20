//! The delivery fence, driven as the real binary against an isolated store.
//!
//! On 2026-09-20 the 0.21.35 delivery to charless-mac-mini refused with
//! `delivery stado 0.21.35 darwin-arm64 does not match its current published
//! run; refusing the stale coordinate` and said nothing about what it had
//! read, so the release was finished by hand through the host declaration
//! instead. The fence is right to refuse; it was wrong to refuse in the dark.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

const PRODUCT: &str = "ci-delivery-probe";
const VERSION: &str = "1.0.0";
const PLATFORM: &str = "darwin-arm64";
const RUN_ID: &str = "run-0000000000000000";
/// The only schema version the run document and the delivery request are
/// written at; the product refuses any other.
const SCHEMA_VERSION: u32 = 1;
/// The digests the published run carries, and the one the stale request names.
const PUBLISHED_ARTIFACT: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const PUBLISHED_MANIFEST: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const STALE_ARTIFACT: &str = "3333333333333333333333333333333333333333333333333333333333333333";
const SOURCE_SHA: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const SOURCE_URI: &str = "stado://releases/ci-delivery-probe/1.0.0/source.tar.gz";
const MANIFEST_URI: &str = "stado://releases/ci-delivery-probe/1.0.0/manifest.json";

struct Fleet {
    home: PathBuf,
    storage: PathBuf,
}

impl Fleet {
    /// One store holding a single published run of the fixture product, and
    /// nothing else.
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release-delivery-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("release-delivery-")
            .tempdir_in(root)
            .unwrap()
            .keep();
        let storage = home.join("store");
        let run_directory = storage.join("runs/release-pipeline").join(RUN_ID);
        fs::create_dir_all(&run_directory).unwrap();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(home.join("revision.txt"), revision.stdout).unwrap();
        fs::write(
            run_directory.join("run.json"),
            serde_json::to_vec_pretty(&run_document()).unwrap(),
        )
        .unwrap();
        Self { home, storage }
    }

    /// A delivery request that names the published run and disagrees with it.
    fn stale_request(&self) -> PathBuf {
        let path = self.home.join("delivery-request.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&request_document()).unwrap(),
        )
        .unwrap();
        path
    }

    fn deliver(&self, request: &Path) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["release", "delivery-worker", "--request"])
            .arg(request)
            .current_dir(&self.home)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .output()
            .unwrap();
        let evidence = self.home.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join("delivery.stdout"), &output.stdout).unwrap();
        fs::write(evidence.join("delivery.stderr"), &output.stderr).unwrap();
        fs::write(
            evidence.join("delivery.exit"),
            format!("{:?}", output.status.code()),
        )
        .unwrap();
        output
    }
}

/// The run the store publishes: built and published for one platform, and
/// already past delivering, because a run in a later state is exactly what
/// the fence has to be able to say out loud.
fn run_document() -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "run_id": RUN_ID,
        "product": PRODUCT,
        "version": VERSION,
        "channel": "candidate",
        "source_commit": "0000000000000000000000000000000000000000",
        "source_sha256": SOURCE_SHA,
        "source_uri": SOURCE_URI,
        "manifest_sha256": PUBLISHED_MANIFEST,
        "manifest_uri": MANIFEST_URI,
        "state": "failed",
        "platforms": {
            PLATFORM: {
                "platform": PLATFORM,
                "builder": "nobody",
                "job_id": "job-0000",
                "output_prefix": "runs/release-pipeline/run-0000000000000000/darwin-arm64",
                "state": "published",
                "artifact_sha256": PUBLISHED_ARTIFACT,
                "release_manifest_sha256": PUBLISHED_MANIFEST
            }
        },
        "deliveries": {},
        "created_at": "2026-09-20T00:00:00+00:00",
        "updated_at": "2026-09-20T00:00:01+00:00"
    })
}

fn request_document() -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "run_id": RUN_ID,
        "name": "fleet-probe",
        "product": PRODUCT,
        "version": VERSION,
        "platform": PLATFORM,
        "argv": ["/usr/bin/true"],
        "required": true,
        "secret_env": {},
        "source_path": "source.tar.gz",
        "source_uri": SOURCE_URI,
        "source_sha256": SOURCE_SHA,
        "archive_path": "artifact.tar.gz",
        "archive_uri": "stado://releases/ci-delivery-probe/1.0.0/darwin-arm64.tar.gz",
        "archive_sha256": STALE_ARTIFACT,
        "manifest_uri": MANIFEST_URI,
        "manifest_sha256": PUBLISHED_MANIFEST
    })
}

#[test]
fn a_refused_delivery_names_the_run_and_what_disagreed() {
    let fleet = Fleet::new();
    let request = fleet.stale_request();

    let output = fleet.deliver(&request);

    assert!(
        !output.status.success(),
        "a coordinate the run does not carry must be refused"
    );
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains(RUN_ID),
        "the refusal must name the run it read:\n{said}"
    );
    assert!(
        said.contains("the run is Failed, not delivering"),
        "the refusal must name the run's state:\n{said}"
    );
    assert!(
        said.contains(STALE_ARTIFACT) && said.contains(PUBLISHED_ARTIFACT),
        "the refusal must name both digests, the run's and the delivery's:\n{said}"
    );
}
