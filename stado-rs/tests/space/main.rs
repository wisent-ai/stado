//! `space` capability tests against an isolated local registry and host.

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const STAGES: [&str; 9] = [
    "registry_cleanup",
    "build_scratch",
    "queue_workdirs",
    "foreign_home_trees",
    "delivered_trees",
    "rebuildable_caches",
    "chromium_clones",
    "local_apfs_snapshots",
    "runner_work_trees",
];

fn stado(storage: &Path, args: &[&str]) -> Output {
    let home = storage.join("home");
    fs::create_dir_all(&home).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(args)
        .env("HOME", &home)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    command.output().expect("stado binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn system_hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase()
}

fn release_platform() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "darwin-amd64"
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-amd64"
    }
}

fn write_registry(storage: &Path, cache_root: Option<&Path>, policy: bool) -> String {
    let disk_cleanup = policy.then(|| {
        let mut cleaners = serde_json::Map::new();
        if let Some(root) = cache_root {
            cleaners.insert(
                "build_caches".to_string(),
                serde_json::json!({
                    "min_age_seconds": 86_400,
                    "root": root.to_string_lossy(),
                }),
            );
        }
        serde_json::json!({
            "mode": "report",
            "check_interval_seconds": 3_600,
            "low_free_gb": 1,
            "target_free_gb": 2,
            "max_bytes_per_pass": 1_073_741_824_i64,
            "max_items_per_pass": 10_000,
            "max_scan_items": 100_000,
            "cleaners": cleaners,
        })
    });
    let mut target = serde_json::json!({
        "name": "space-fixture",
        "kind": "local",
        "release_platform": release_platform(),
        "hostnames": [system_hostname()],
    });
    if let Some(policy) = disk_cleanup {
        target["disk_cleanup"] = policy;
    }
    let document = serde_json::json!({
        "schema_version": 2,
        "targets": [target],
        "coordinators": [],
    });
    let text = serde_json::to_string_pretty(&document).unwrap();
    fs::write(storage.join("registry.json"), &text).unwrap();
    text
}

fn setup(cache: bool, policy: bool) -> (tempfile::TempDir, PathBuf, String) {
    let storage = tempfile::tempdir().unwrap();
    let cache_root = storage.path().join("home/build-cache");
    fs::create_dir_all(&cache_root).unwrap();
    let registry = write_registry(
        storage.path(),
        if cache { Some(&cache_root) } else { None },
        policy,
    );
    (storage, cache_root, registry)
}

fn report(storage: &Path) -> (Output, serde_json::Value) {
    let output = stado(storage, &["space", "report", "space-fixture", "--json"]);
    assert!(
        output.status.success(),
        "report failed: {}",
        stderr(&output)
    );
    let document = serde_json::from_slice(&output.stdout).expect("report is one JSON document");
    (output, document)
}

#[test]
fn declared_stages_are_reported_and_are_the_only_reclaim_stage_vocabulary() {
    let (storage, _, original_registry) = setup(true, true);
    let (_, document) = report(storage.path());
    let reported: Vec<&str> = document["reclaim_stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stage| stage["name"].as_str().unwrap())
        .collect();
    assert_eq!(reported, STAGES);

    for stage in STAGES {
        let output = stado(
            storage.path(),
            &[
                "space",
                "reclaim",
                "missing-target",
                "--stage",
                stage,
                "--dry-run",
            ],
        );
        assert!(!output.status.success());
        assert!(
            stderr(&output).contains("target 'missing-target' is not in the canonical registry"),
            "declared stage {stage} did not reach target resolution: {}",
            stderr(&output)
        );
    }

    let unknown = stado(
        storage.path(),
        &[
            "space",
            "reclaim",
            "space-fixture",
            "--stage",
            "mystery",
            "--dry-run",
        ],
    );
    assert!(!unknown.status.success());
    assert!(stderr(&unknown).contains(
        "stage 'mystery' is not declared; add it to stado-rs/data/space.json reclaim_stages"
    ));
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn report_exposes_watermarks_inventory_janitor_and_no_cache_tags_verdict() {
    let (storage, _, original_registry) = setup(true, true);
    let (_, document) = report(storage.path());
    assert_eq!(document["target"], "space-fixture");
    assert!(document["usage"]["filesystem"].is_string());
    assert!(document["memory"]["free_kb"].is_string());
    assert_eq!(
        document["free_space"]["low_watermark_bytes"],
        1_073_741_824_i64
    );
    assert_eq!(
        document["free_space"]["target_watermark_bytes"],
        2_147_483_648_i64
    );
    assert!(document["inventory"].is_array());
    assert!(document.get("cleanup_state").is_some());
    assert!(document.get("cleanup_lock").is_some());
    assert_eq!(
        document["build_caches"]["entries"][0]["verdict"],
        "no-cache-tags"
    );
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn build_cache_report_preserves_root_protected_and_scan_failed_verdicts() {
    let (storage, cache_root, original_registry) = setup(true, true);
    fs::write(
        cache_root.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    let (_, protected) = report(storage.path());
    assert_eq!(
        protected["build_caches"]["entries"][0]["verdict"],
        "root-protected"
    );

    fs::remove_file(cache_root.join("CACHEDIR.TAG")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cache_root, fs::Permissions::from_mode(0o000)).unwrap();
        let failed = stado(
            storage.path(),
            &["space", "report", "space-fixture", "--json"],
        );
        fs::set_permissions(&cache_root, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!failed.status.success());
        assert!(
            stdout(&failed).contains("\"verdict\":\"scan-failed\"")
                || stdout(&failed).contains("\"verdict\": \"scan-failed\""),
            "scan failure verdict missing: {}",
            stdout(&failed)
        );
    }
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn reclaim_dry_run_preserves_candidate_registry_and_audit_state() {
    let (storage, _, original_registry) = setup(true, true);
    let candidate = storage.path().join("home/.stado/build-work/old-build");
    fs::create_dir_all(&candidate).unwrap();
    let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    File::open(&candidate)
        .unwrap()
        .set_times(FileTimes::new().set_accessed(old).set_modified(old))
        .unwrap();

    let output = stado(
        storage.path(),
        &[
            "space",
            "reclaim",
            "space-fixture",
            "--stage",
            "build_scratch",
            "--dry-run",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "reclaim failed: {}",
        stderr(&output)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["mode"], "dry_run");
    assert_eq!(
        document["selected_stages"],
        serde_json::json!(["build_scratch"])
    );
    assert!(candidate.exists(), "dry-run removed an eligible candidate");
    assert!(!storage
        .path()
        .join("home/.stado/audit/host-reclaim.jsonl")
        .exists());
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn reclaim_apply_deletes_candidate_and_audits_the_reason_on_target() {
    let (storage, _, original_registry) = setup(true, true);
    let candidate = storage.path().join("home/.stado/build-work/old-build");
    fs::create_dir_all(&candidate).unwrap();
    let old = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    File::open(&candidate)
        .unwrap()
        .set_times(FileTimes::new().set_accessed(old).set_modified(old))
        .unwrap();

    let output = stado(
        storage.path(),
        &[
            "space",
            "reclaim",
            "space-fixture",
            "--stage",
            "build_scratch",
            "--apply",
            "--reason",
            "integration-test cleanup",
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "reclaim failed: {}",
        stderr(&output)
    );
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["mode"], "apply");
    assert!(!candidate.exists(), "apply retained an eligible candidate");
    let audit_path = storage.path().join("home/.stado/audit/host-reclaim.jsonl");
    let audit = fs::read_to_string(&audit_path).expect("applied run writes target audit");
    let record: serde_json::Value =
        serde_json::from_str(audit.trim()).expect("audit is one JSON-lines record");
    assert_eq!(record["reason"], "integration-test cleanup");
    assert_eq!(record["command"], "stado space reclaim");
    assert_eq!(record["host"], "space-fixture");
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn reclaim_refusals_name_the_missing_reason_stage_and_declaration() {
    let (storage, _, original_registry) = setup(true, true);
    let no_reason = stado(
        storage.path(),
        &["space", "reclaim", "space-fixture", "--apply"],
    );
    assert!(!no_reason.status.success());
    assert!(stderr(&no_reason).contains(
        "space reclaim --apply removes files and needs --reason <text>; the reason is appended to the target's own audit log beside the state it changed. Run without --apply to preview the declared stages"
    ));

    write_registry(storage.path(), None, false);
    let no_eligible = stado(
        storage.path(),
        &[
            "space",
            "reclaim",
            "space-fixture",
            "--stage",
            "local_apfs_snapshots",
            "--dry-run",
        ],
    );
    assert!(!no_eligible.status.success());
    assert!(stderr(&no_eligible).contains(
        "space-fixture declares no eligible space reclamation stage; add it to stado-rs/data/space.json reclaim_stages"
    ));

    fs::write(storage.path().join("registry.json"), &original_registry).unwrap();
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        original_registry
    );
}

#[test]
fn report_refuses_missing_cleanup_and_build_cache_declarations() {
    let (storage, _, missing_policy_registry) = setup(false, false);
    let missing_policy = stado(
        storage.path(),
        &["space", "report", "space-fixture", "--json"],
    );
    assert!(!missing_policy.status.success());
    assert!(stderr(&missing_policy).contains(
        "space-fixture declares no disk cleanup policy; add it to registry targets[].disk_cleanup"
    ));
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        missing_policy_registry
    );

    let missing_cleaner_registry = write_registry(storage.path(), None, true);
    let missing_cleaner = stado(
        storage.path(),
        &["space", "report", "space-fixture", "--json"],
    );
    assert!(!missing_cleaner.status.success());
    assert!(stderr(&missing_cleaner).contains(
        "space-fixture declares no build cache cleaner; add it to registry targets[].disk_cleanup.cleaners.build_caches"
    ));
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        missing_cleaner_registry
    );

    let invalid_root_registry =
        write_registry(storage.path(), Some(Path::new("relative/cache")), true);
    let invalid_root = stado(
        storage.path(),
        &["space", "report", "space-fixture", "--json"],
    );
    assert!(!invalid_root.status.success());
    assert!(stderr(&invalid_root).contains(
        "space-fixture declares build cache root \"relative/cache\" outside an absolute or home-relative path; fix registry targets[].disk_cleanup.cleaners.build_caches.root"
    ));
    assert_eq!(
        fs::read_to_string(storage.path().join("registry.json")).unwrap(),
        invalid_root_registry
    );
}

#[test]
fn host_help_no_longer_exposes_replaced_space_verbs() {
    let storage = tempfile::tempdir().unwrap();
    let output = stado(storage.path(), &["host", "--help"]);
    assert!(
        output.status.success(),
        "host help failed: {}",
        stderr(&output)
    );
    let help = stdout(&output);
    for removed in [
        "disk",
        "disk-cleanup",
        "cleanup",
        "reclaim",
        "build-caches",
        "object-relocate",
        "remove-file",
        "retire-file",
    ] {
        assert!(
            !help
                .lines()
                .any(|line| line.trim_start().starts_with(removed)),
            "host help still exposes {removed}: {help}"
        );
    }
}
