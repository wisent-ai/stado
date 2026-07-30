//! First-run CLI contract for a person starting with an empty home directory.

use std::process::Command;

const EXPECTED_ONBOARDING: &str = "\
Stado — one queue for every machine.

Stado needs three things:
- state storage for the queue and results,
- at least one compute provider,
- a running worker that can claim jobs.

Fastest path: local mode. `stado config init` creates:
- provider: local
- queue storage: ~/.stado/local-storage
- backup storage: ~/.stado/local-backup

No cloud account or credentials are required for local mode.
The worker host must already have the shell, runtime, and GPU driver required by the workload.

1. Create the local configuration:
   stado config init

2. Check the installation:
   stado config validate
   stado doctor --fix-hints

3. Start the local control plane:
   stado local-control-plane

Open http://127.0.0.1:8765

Submit your first job:
   stado submit \"printf 'hello from Stado\\n'\"

Already configured? Run:
   stado overview

More commands:
   stado --help
";

#[test]
fn fresh_user_sees_safe_local_path_without_creating_state() {
    let home = tempfile::tempdir().expect("fresh user home is created");
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("STADO_CONFIG")
        .env_remove("WC_STORAGE_BACKEND")
        .env_remove("WC_LOCAL_STORAGE_PATH")
        .env_remove("WC_PROVIDERS")
        .output()
        .expect("stado starts without arguments");

    assert!(
        output.status.success(),
        "first run must be guidance, not a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED_ONBOARDING);
    assert!(
        !home.path().join(".stado").exists(),
        "showing onboarding must not create configuration or mutable state"
    );
}

#[test]
fn config_init_seeds_a_safe_local_registry_for_the_first_worker() {
    let home = tempfile::tempdir().expect("fresh user home is created");
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env_remove("STADO_CONFIG")
        .env_remove("WC_STORAGE_BACKEND")
        .env_remove("WC_LOCAL_STORAGE_PATH")
        .env_remove("WC_PROVIDERS")
        .args(["config", "init"])
        .output()
        .expect("config init runs");

    assert!(
        output.status.success(),
        "config init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let registry_path = home
        .path()
        .join(".stado")
        .join("local-storage")
        .join(stado::targets::REGISTRY_BLOB);
    let registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&registry_path).expect("local registry was initialized"),
    )
    .expect("local registry is JSON");
    stado::targets::validate_registry(&registry).expect("local registry satisfies its schema");
    let targets = registry["targets"]
        .as_array()
        .expect("local registry targets are an array");
    assert_eq!(
        targets.len(),
        usize::try_from(stado::providers::local::disk_cleanup::STATE_VERSION)
            .expect("state version fits usize")
    );
    let hostname =
        stado::targets::normalize_hostname(&stado::providers::vast::system_hostname());
    let target = &targets[usize::default()];
    let name_matches = target["name"].as_str() == Some(hostname.as_str());
    let hostname_matches = target["hostnames"]
        .as_array()
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(hostname.as_str())));
    assert!(name_matches || hostname_matches);
    assert_eq!(
        targets[usize::default()]["disk_cleanup"]["mode"],
        "off"
    );
}
