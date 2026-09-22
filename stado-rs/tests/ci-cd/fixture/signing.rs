//! Real native SDK preparation and the pinned Apple issuer for Darwin journeys.

use super::*;

pub(crate) fn seed_native_signing_input(home: &Path, storage: &Path) {
    let selected = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["config", "show"]).output().expect("the real Stado configuration reader runs");
    assert!(selected.status.success(), "cannot read the selected release-store configuration: {}", String::from_utf8_lossy(&selected.stderr));
    fs::write(home.join("native-sdk-config.stdout.json"), &selected.stdout).unwrap();
    fs::write(home.join("native-sdk-config.stderr.log"), &selected.stderr).unwrap();
    let selected: serde_json::Value = serde_json::from_slice(&selected.stdout).unwrap();
    let config = PathBuf::from(selected["file"].as_str().expect("selected Stado configuration path is absent"));
    let prepared = Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", home)
        .env("STADO_CONFIG", &config)
        .env("WISENT_WORKSPACE", home.join("workspace"))
        .args(["product", "catalog", "--json"]).output().expect("the native SDK consumer runs");
    fs::write(home.join("native-sdk.stdout.json"), &prepared.stdout).unwrap();
    fs::write(home.join("native-sdk.stderr.log"), &prepared.stderr).unwrap();
    fs::write(home.join("native-sdk.exit.txt"), prepared.status.to_string()).unwrap();
    assert!(prepared.status.success(), "blocked: the qualified native SDK could not be prepared through Stado: {}", String::from_utf8_lossy(&prepared.stderr));

    let namespace = std::env::var("WC_STADO_STORAGE_NAMESPACE").ok().filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let declared: serde_json::Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
            declared["storage"]["stado"]["namespace"].as_str().filter(|value| !value.is_empty())
                .expect("the selected Stado configuration declares no signing-input namespace").to_owned()
        });
    let issuers = stado::deploy::native_signing::APPLE_ISSUER_CHAIN_SHA256;
    seed_pinned_input(home, storage, &namespace, &format!("apple-issuers-{issuers}.pem"), issuers);
}

fn seed_pinned_input(home: &Path, storage: &Path, namespace: &str, leaf: &str, sha: &str) {
    let fetched_copy = home.join(format!("native-signing-input-{leaf}"));
    let uri = format!("stado://{namespace}/artifacts/native-signing/{leaf}");
    let fetched = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["storage", "get", &uri]).arg(&fetched_copy)
        .output().expect("the Stado object reader runs");
    fs::write(home.join("native-issuer-fetch.stdout.log"), &fetched.stdout).unwrap();
    fs::write(home.join("native-issuer-fetch.stderr.log"), &fetched.stderr).unwrap();
    fs::write(home.join("native-issuer-fetch.exit.txt"), fetched.status.to_string()).unwrap();
    assert!(fetched.status.success(), "blocked: the pinned Apple issuer {uri} could not be read from the real store: {}", String::from_utf8_lossy(&fetched.stderr));
    let (_, digest) = stado::release_control::sha256_file(&fetched_copy).unwrap();
    assert_eq!(digest, sha, "the real store served another Apple issuer chain");
    run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("STADO_CONFIG", home.join(".stado/config.json"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("WC_STADO_STORAGE_NAMESPACE", "ci-release")
        .args([
            "storage", "put", "--content-type", "application/x-pem-file",
            &format!("stado://ci-release/artifacts/native-signing/{leaf}"),
        ]).arg(&fetched_copy));
}
