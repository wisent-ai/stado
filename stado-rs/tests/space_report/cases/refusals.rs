//! What the command refuses before it reaches the host at all.

use crate::fixture::{Fixture, TARGET};

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
