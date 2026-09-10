//! An authority this host cannot reach, and the refusal it publishes.
use super::*;

/// This machine's kernel hostname, normalized the way the registry
/// validator demands ("must be normalized as '<lowercase>'").
fn hostname() -> String {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let out = Command::new("hostname").output().expect("hostname(1) runs");
    String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
}

fn spawn_stado(storage: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        // The resolver reads its ssh key, its control sockets and its
        // socket-reaper directory out of $HOME/.stado. Pointed at the
        // tempdir the ssh call is hermetic AND the reaper cannot reach the
        // live resolver's control sockets on the operator's machine — it
        // dropped one during the first hand probe of this very test.
        .env("HOME", storage)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("STADO_RESOLVER_SSH_KEY_FILE")
        .output()
}

fn stado(storage: &Path, args: &[&str]) -> Output {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    spawn_stado(storage, args).expect("stado binary runs")
}

#[test]
fn an_unreachable_authority_publishes_its_own_sentence_as_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    // This machine has to be a registry target for the resolver to know who
    // it is; the authority is a name RFC 2606 reserves so it can never
    // resolve and no packet leaves the machine.
    let document = json!({
        "schema_version": 2,
        "targets": [
            {
                "name": "silence-test-local",
                "kind": "local",
                "release_platform": "darwin-arm64",
                "hostnames": [hostname()]
            },
            {
                "name": "silence-test-authority",
                "kind": "local",
                "ssh": "stado@silence-test-authority.invalid",
                "release_platform": "darwin-arm64",
                "services": [
                    {"name": "brama", "kind": "launchd", "path": "/opt/stado/brama.plist"}
                ]
            }
        ],
        "coordinators": [],
        "service_directory": {
            "authority": {
                "target": "silence-test-authority",
                "command": "/opt/stado/bin/stado"
            },
            "generation": 7,
            "services": {
                "brama": {
                    "managed_service": "brama",
                    "active_host": "silence-test-authority",
                    "endpoints": {
                        "silence-test-authority": {"url": "http://127.0.0.1:8080"}
                    },
                    "consumers": {"lem": {"capabilities": ["model-routing"]}}
                }
            }
        }
    });
    std::fs::write(
        storage.join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let out = stado(
        storage,
        &["resolver", "resolve", "brama", "--consumer", "lem"],
    );
    assert!(
        !out.status.success(),
        "an unreachable authority resolved: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let printed = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        printed.contains("registry authority"),
        "the command did not reach the authority read: {printed}"
    );

    let names = blob_names(storage, "state/reader_refusals/silence-test-authority");
    assert_eq!(
        names.len(),
        1,
        "the failed read published no refusal (stderr: {printed})"
    );
    let record = on_disk(
        storage,
        &format!("state/reader_refusals/silence-test-authority/{}", names[0]),
    );
    assert_eq!(
        record["host"], "silence-test-authority",
        "the refusal is filed under the host it is evidence about, not the \
         machine that noticed"
    );
    assert_eq!(record["reader"], "cli");
    assert_eq!(record["reason"], "authority_unreachable");
    let detail = record["detail"].as_str().expect("detail is a string");
    assert!(
        printed.contains(detail),
        "the stored detail is not the sentence the command printed:\n  stored: {detail}\n  printed: {printed}"
    );
}
