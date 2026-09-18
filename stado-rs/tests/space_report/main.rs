//! `stado space report`, driven as the real binary against an isolated
//! registry that names this machine, so the read executes locally.
//!
//! What this defends is one shape of failure. The report is two things at
//! once: three cheap fields — free space, the janitor's last outcome, memory —
//! and one attribution walk over the whole selected tree. The walk cost more
//! than the shared two-minute channel bound, so on 2026-09-02 and again on
//! 2026-09-09 the command died having computed nothing, on the very machine
//! whose disk was the question. The cheap fields cost under a second and were
//! lost with it.
//!
//! So the walk now has its own budget, and exceeding it is reported rather
//! than fatal. A budget of zero is that same branch without a race: it says
//! the walk was not attempted, so the cheap report an operator needs on a
//! host whose walk costs minutes is one environment value away, and the
//! branch is provable on a warm machine instead of only on a slow one.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

/// The isolated registry names one target, and the assertions read it back.
const TARGET: &str = "space-report-fixture";

/// Directories a Mac always has, so the fixture's `PATH` finds `df`, `tr` and
/// the shell the remote program runs under.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Schema versions the product's own writers use: `config_file` for the
/// configuration document and `targets::REGISTRY_SCHEMA_VERSION` for the
/// registry. Named here rather than spelled inside the fixture documents, so
/// a reader can see which contract each number belongs to.
const CONFIG_SCHEMA_VERSION: i64 = 1;
const REGISTRY_SCHEMA_VERSION: i64 = 2;

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    storage: PathBuf,
    config: PathBuf,
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture file");
    let mut permissions = fs::metadata(path).expect("stat fixture file").permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).expect("restrict fixture file");
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("read this machine's hostname");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl Fixture {
    fn new() -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/space-report");
        fs::create_dir_all(&evidence).unwrap();
        let root = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(evidence)
            .unwrap()
            .keep();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(root.join("revision.txt"), revision.stdout).unwrap();
        let home = root.join("home");
        let storage = root.join("storage");
        for directory in [&home, &storage, &root.join("tmp")] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        let config = root.join("config.json");
        write_private(
            &config,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "storage": {"backend": "local", "local": {"path": storage}},
            }))
            .expect("render fixture config"),
        );
        write_private(
            &storage.join("registry.json"),
            &serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA_VERSION,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": "darwin-arm64",
                    "hostnames": [hostname()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .expect("render fixture registry"),
        );
        Self {
            root,
            home,
            storage,
            config,
        }
    }

    /// The report with the attribution walk held to `budget_seconds`, where
    /// `0` means the walk is not attempted at all.
    fn report(&self, budget_seconds: &str, extra: &[&str]) -> Output {
        let mut args = vec!["space", "report", TARGET];
        args.extend_from_slice(extra);
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(&args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .env("STADO_INVENTORY_BUDGET_SECONDS", budget_seconds)
            .output()
            .expect("run stado space report");
        let name = if extra.contains(&"--json") {
            "json"
        } else {
            "text"
        };
        fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        fs::write(
            self.root.join(format!("{name}.exit")),
            format!("{:?}", output.status.code()),
        )
        .unwrap();
        output
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.home);
        let _ = fs::remove_dir_all(&self.storage);
    }
}

#[test]
fn a_report_without_the_walk_still_carries_free_space_memory_and_the_janitor() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &[]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_eq!(
        output.status.code(),
        Some(0),
        "a skipped walk is not a failed command; stderr: {stderr}"
    );
    assert!(
        text.contains("disk:") && text.contains("free:") && text.contains("GiB"),
        "the disk and free-space lines survive the missing walk: {text}"
    );
    assert!(
        text.contains("memory:"),
        "the memory reading survives the missing walk: {text}"
    );
    assert!(
        text.contains("janitor:"),
        "the janitor's own outcome survives the missing walk: {text}"
    );
    assert!(
        text.contains("inventory incomplete:"),
        "the missing inventory was hidden: {text}"
    );
    fixture.cleanup();
}

#[test]
fn the_report_names_the_walk_it_did_not_run() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &["--json"]);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let detail = document
        .get("inventory_incomplete")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("the report names the walk it did not run: {document}"));

    assert!(
        detail.contains("inventory not read:")
            && detail.contains("STADO_INVENTORY_BUDGET_SECONDS is 0"),
        "{detail}"
    );
    fixture.cleanup();
}

/// The `disk:` line measures the fleet's volume; `volumes[]` names every
/// device-backed filesystem beside it, the fleet's among them, and
/// `block_devices.read` says whether the host could list its disks at all.
/// On 2026-09-18 a 16 TiB disk sat attached and unmounted on the Linux
/// builder while the report said the host had 29 GiB.
#[test]
fn the_report_lists_every_volume_and_says_whether_disks_were_listed() {
    let fixture = Fixture::new();
    let output = fixture.report("0", &["--json"]);
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let fleet_volume = document["usage"]["filesystem"]
        .as_str()
        .unwrap_or_else(|| panic!("the report names the fleet's volume: {document}"));
    let volumes = document["volumes"]
        .as_array()
        .unwrap_or_else(|| panic!("the report carries volumes[]: {document}"));
    assert!(
        volumes
            .iter()
            .any(|volume| volume["filesystem"].as_str() == Some(fleet_volume)),
        "the fleet's volume {fleet_volume} is among the volumes: {volumes:?}"
    );
    assert!(
        volumes.iter().all(|volume| {
            volume["filesystem"]
                .as_str()
                .is_some_and(|device| device.starts_with("/dev/"))
                && volume["mounted_on"]
                    .as_str()
                    .is_some_and(|point| point.starts_with('/'))
        }),
        "every volume is device-backed and mounted: {volumes:?}"
    );
    let listed = document["block_devices"]["read"]
        .as_bool()
        .unwrap_or_else(|| panic!("the report says whether disks were listed: {document}"));
    let has_lsblk = Path::new("/usr/bin/lsblk").exists();
    assert_eq!(
        listed, has_lsblk,
        "block_devices.read follows whether this host has lsblk: {document}"
    );
    fixture.cleanup();
}

/// `space volume mount` refuses a device word that is not one `/dev` leaf
/// and a mount point under a system tree before it reaches any host: the
/// refusal is the command's own sentence, and the exit is nonzero.
#[test]
fn volume_mount_refuses_a_device_path_and_a_system_mount_point_before_the_host() {
    let fixture = Fixture::new();
    for (device, mount_point, expected) in [
        ("../sda", "/mnt/data", "--device names one /dev leaf"),
        ("sdb1", "/etc/data", "is under a system tree"),
        ("sdb1", "mnt/data", "absolute directory path"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args([
                "space",
                "volume",
                "mount",
                TARGET,
                "--device",
                device,
                "--mount-point",
                mount_point,
            ])
            .env_clear()
            .env("HOME", &fixture.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", fixture.root.join("tmp"))
            .env("STADO_CONFIG", &fixture.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &fixture.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .output()
            .expect("run stado space volume mount");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        fs::write(
            fixture
                .root
                .join(format!("volume-mount-{}.stderr", device.replace('/', "_"))),
            &output.stderr,
        )
        .unwrap();
        assert_ne!(
            output.status.code(),
            Some(0),
            "{device} at {mount_point} was not refused: {stderr}"
        );
        assert!(
            stderr.contains(expected),
            "{device} at {mount_point}: the refusal lost its sentence {expected:?}: {stderr}"
        );
    }
    fixture.cleanup();
}

/// `space work-root` without `--path` reads the declaration; a target that
/// declares none is told where its agent works by default. With `--path`,
/// a relative path and a path under a system tree are refused with the
/// registry's own sentence before any host is reached, and the registry is
/// left as it was.
#[test]
fn work_root_reads_the_default_and_refuses_bad_paths_before_the_host() {
    let fixture = Fixture::new();
    let run = |extra: &[&str]| {
        let mut args = vec!["space", "work-root", TARGET];
        args.extend_from_slice(extra);
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(&args)
            .env_clear()
            .env("HOME", &fixture.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", fixture.root.join("tmp"))
            .env("STADO_CONFIG", &fixture.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &fixture.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .output()
            .expect("run stado space work-root")
    };
    let read = run(&["--json"]);
    fs::write(fixture.root.join("work-root-read.stdout"), &read.stdout).unwrap();
    let document: serde_json::Value =
        serde_json::from_slice(&read.stdout).expect("the read is one JSON document");
    assert_eq!(read.status.code(), Some(0));
    assert!(
        document["work_root"].is_null(),
        "an undeclared target reads as undeclared: {document}"
    );
    for (path, expected) in [
        ("mnt/data", "must be an absolute path"),
        ("/etc/stado-work", "is under a system tree"),
        ("/", "must name a directory below /"),
    ] {
        let refused = run(&["--path", path]);
        let stderr = String::from_utf8_lossy(&refused.stderr).into_owned();
        assert_ne!(
            refused.status.code(),
            Some(0),
            "--path {path} was not refused: {stderr}"
        );
        assert!(
            stderr.contains(expected),
            "--path {path}: the refusal lost its sentence {expected:?}: {stderr}"
        );
    }
    let registry = fs::read_to_string(fixture.storage.join("registry.json")).unwrap();
    assert!(
        !registry.contains("work_root"),
        "a refused declaration reached the registry: {registry}"
    );
    fixture.cleanup();
}

/// A bound that is not a whole number of seconds is refused before anything
/// is walked, and the refusal says what zero means.
#[test]
fn a_malformed_walk_bound_is_refused_with_its_own_sentence() {
    let fixture = Fixture::new();
    let output = fixture.report("soon", &["--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert_ne!(
        output.status.code(),
        Some(0),
        "a malformed bound was accepted: {stderr}"
    );
    assert!(
        stderr.contains("STADO_INVENTORY_BUDGET_SECONDS must be a whole number of seconds")
            && stderr.contains("0 reads the report without the attribution walk"),
        "the refusal did not name the bound and what zero means: {stderr}"
    );
    fixture.cleanup();
}

/// The build-cache verdict walks the whole undeclared root, and on 2026-09-17
/// that walk opened `~/Library/CloudStorage`, met one unreadable Google Drive
/// `.tmp`, and reported lukasz-macbook as `scan-failed`, exit 1, classified
/// as rejected credentials. The walk now prunes the janitor's own refused
/// roots before opening them, and a directory it cannot open elsewhere is one
/// `permission-denied` row while the tagged cache beside it is still found.
#[test]
fn an_unreadable_directory_is_one_row_and_a_refused_root_is_never_opened() {
    let fixture = Fixture::new();
    let cloud = fixture.home.join("Library/CloudStorage/drive/.tmp");
    let secret = fixture.home.join("secret");
    let cache = fixture.home.join("work/target");
    for directory in [&cloud, &secret, &cache] {
        fs::create_dir_all(directory).expect("create fixture directory");
    }
    fs::write(
        cache.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .expect("write cache tag");
    for closed in [&cloud, &secret] {
        let mut permissions = fs::metadata(closed)
            .expect("stat closed directory")
            .permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(closed, permissions).expect("close fixture directory");
    }

    let output = fixture.report("0", &["--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    for closed in [&cloud, &secret] {
        let mut permissions = fs::metadata(closed)
            .expect("stat closed directory")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(closed, permissions).expect("reopen fixture directory");
    }
    assert_eq!(
        output.status.code(),
        Some(0),
        "one unreadable directory failed the whole host; stderr: {stderr}"
    );
    let document: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the report is one JSON document");
    let caches = &document["build_caches"];
    assert!(
        caches["error"].is_null(),
        "the verdict carried an error instead of rows: {}",
        caches["error"]
    );
    let entries = caches["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("the verdict lists its rows: {document}"));
    let state_of = |path: &Path| {
        entries
            .iter()
            .find(|entry| entry["path"].as_str() == Some(path.to_str().unwrap()))
            .map(|entry| entry["verdict"].as_str().unwrap_or_default().to_string())
    };
    assert_eq!(
        state_of(&secret).as_deref(),
        Some("permission-denied"),
        "the unreadable directory outside the refused roots is its own row: {entries:?}"
    );
    assert!(
        state_of(&cache).is_some_and(|state| state != "scan-failed"),
        "the tagged cache beside the unreadable directory was still found: {entries:?}"
    );
    assert!(
        entries.iter().all(|entry| !entry["path"]
            .as_str()
            .unwrap_or_default()
            .contains("CloudStorage")),
        "the refused root was opened: {entries:?}"
    );
    assert!(
        !stderr.contains("credentials this command used were rejected"),
        "a file the host would not open was reported as rejected credentials: {stderr}"
    );
    fixture.cleanup();
}
