//! One published version means one build, proved through the real binary.
//!
//! `stado release claim-coordinate` binds an immutable coordinate to exactly
//! one source revision before any artifact byte is written into it. This area
//! exists because 0.13.46 on darwin-arm64 was published twice, from two source
//! revisions, and the objects are immutable: nothing could make that version
//! mean one build afterwards.
//!
//! Every case runs `CARGO_BIN_EXE_stado` against an isolated local object
//! store and reads the claim documents the product itself wrote. No store, no
//! publisher and no revision is simulated.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const PRODUCT: &str = "stado";
const VERSION: &str = "0.0.0-claim-area";
const PLATFORM: &str = "darwin-arm64";
/// Two distinct 40-character object names, so neither can be the other.
const FIRST_REVISION: &str = "1111111111111111111111111111111111111111";
const SECOND_REVISION: &str = "2222222222222222222222222222222222222222";
/// The schema version the product writes into a claim record.
const CLAIM_SCHEMA_VERSION: u64 = 1;
const REFUSED_EXIT: i32 = 1;

/// One isolated store and the retained evidence of what ran against it.
struct Area {
    root: PathBuf,
    storage: PathBuf,
    commands: std::cell::Cell<usize>,
}

impl Area {
    fn new() -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join(".wisent-output/release-claim");
        std::fs::create_dir_all(&evidence).expect("create retained claim evidence directory");
        let root = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(evidence)
            .expect("create the isolated claim area")
            .keep();
        let storage = root.join("storage");
        std::fs::create_dir_all(&storage).expect("create the isolated object store");
        let area = Self {
            root,
            storage,
            commands: std::cell::Cell::new(0),
        };
        let identity = area.stado(&["--version"]);
        assert!(
            identity.status.success(),
            "the product binary did not report its identity"
        );
        eprintln!("release claim evidence: {}", area.root.display());
        area
    }

    fn stado(&self, args: &[&str]) -> Output {
        let index = self.commands.get();
        self.commands.set(index + 1);
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            // A set-but-missing config disables configuration discovery, so
            // the operator's own deployment is never addressed.
            .env("STADO_CONFIG", self.root.join("no-such-config.json"))
            .env("NO_COLOR", "1")
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("STADO_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("the stado binary runs");
        let directory = self.root.join("commands").join(index.to_string());
        std::fs::create_dir_all(&directory).expect("create command evidence directory");
        std::fs::write(directory.join("args"), args.join(" ")).expect("record the command");
        std::fs::write(directory.join("stdout"), &output.stdout).expect("record stdout");
        std::fs::write(directory.join("stderr"), &output.stderr).expect("record stderr");
        std::fs::write(
            directory.join("exit"),
            output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_default(),
        )
        .expect("record the exit status");
        output
    }

    fn claim(&self, revision: &str) -> Output {
        self.stado(&[
            "release",
            "claim-coordinate",
            PRODUCT,
            VERSION,
            PLATFORM,
            "--source-commit",
            revision,
            "--json",
        ])
    }

    /// The claim records the product wrote, version-scoped and platform-scoped.
    fn records(&self) -> (Vec<u8>, Vec<u8>) {
        let base = self
            .storage
            .join("ecosystem/releases")
            .join(PRODUCT)
            .join(VERSION);
        (
            std::fs::read(base.join("source-revision.json")).expect("read the version claim"),
            std::fs::read(base.join(PLATFORM).join("source-revision.json"))
                .expect("read the platform claim"),
        )
    }
}

fn document(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document, got {error}\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn a_claimed_coordinate_records_its_revision_and_repeats_without_changing_it() {
    let area = Area::new();

    let claimed = area.claim(FIRST_REVISION);
    assert!(
        claimed.status.success(),
        "the first claim failed: {}",
        String::from_utf8_lossy(&claimed.stderr)
    );
    let receipt = document(&claimed);
    assert_eq!(receipt["state"], "claimed");
    assert_eq!(
        receipt["uri"],
        format!("stado://releases/{PRODUCT}/{VERSION}/{PLATFORM}/source-revision.json")
    );

    let (version_record, platform_record) = area.records();
    let held: Value = serde_json::from_slice(&version_record).expect("the version claim is JSON");
    assert_eq!(held["schema_version"], CLAIM_SCHEMA_VERSION);
    assert_eq!(held["product"], PRODUCT);
    assert_eq!(held["version"], VERSION);
    assert_eq!(held["source_revision"], FIRST_REVISION);
    let coordinate: Value =
        serde_json::from_slice(&platform_record).expect("the platform claim is JSON");
    assert_eq!(coordinate["platform"], PLATFORM);
    assert_eq!(coordinate["source_revision"], FIRST_REVISION);

    // The same publisher arriving twice is the ordinary retry of one build.
    let repeated = area.claim(FIRST_REVISION);
    assert!(
        repeated.status.success(),
        "the same revision was refused its own coordinate: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(document(&repeated)["state"], "confirmed");
    assert_eq!(area.records(), (version_record, platform_record));
}

#[test]
fn a_second_source_revision_is_refused_and_the_recorded_claim_is_untouched() {
    let area = Area::new();
    assert!(area.claim(FIRST_REVISION).status.success());
    let before = area.records();

    let refused = area.claim(SECOND_REVISION);
    assert_eq!(refused.status.code(), Some(REFUSED_EXIT));
    let failure = document(&refused);
    assert_eq!(failure["status"], "error");
    assert_eq!(failure["error_code"], "refused");
    assert_eq!(failure["retryable"], false);
    assert_eq!(failure["failure_point"], "cli.release.claim-coordinate");
    assert_eq!(
        failure["summary"],
        "an explicit policy refused this command"
    );
    assert_eq!(
        failure["message"],
        format!(
            "{PRODUCT}/{VERSION} already attests source revision {FIRST_REVISION}, and this \
             publisher carries {SECOND_REVISION}. Release objects are immutable: publish a new \
             version"
        )
    );
    assert_eq!(
        area.records(),
        before,
        "a refused claim rewrote the recorded revision"
    );
}
