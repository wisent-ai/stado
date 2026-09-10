//! What the suite reads back: minted verifiers, versions, hostnames, ports,
//! digests and the shape of a refusal.
use super::*;

pub(crate) fn mint_verifier(
    home: &Path,
    storage: &Path,
    config: &Path,
    gnupg_home: &Path,
    vault_file: &Path,
    capabilities: &str,
    token_file_name: &str,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args([
            "host",
            "vault-token-mint",
            HOST,
            "stado-registry-api-verifier",
            "--capabilities",
            capabilities,
            "--audience",
            "skarbiec",
            "--token-file-name",
            token_file_name,
            "--json",
        ])
        .env_clear()
        .env("HOME", home)
        .env("PATH", PATH_ENV)
        .env("TMPDIR", home.join("tmp"))
        .env("STADO_CONFIG", config)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("WC_STADO_STORAGE_NAMESPACE", "service-convergence")
        .env("SKARBIEC_VAULT_FILE", vault_file)
        .env("GNUPGHOME", gnupg_home)
        .stdin(Stdio::null())
        .output()
        .expect("built Stado verifier provisioning command runs")
}

pub(crate) fn binary_version(binary: &Path, home: &Path) -> String {
    let output = Command::new(binary)
        .arg("--version")
        .env_clear()
        .env("HOME", home)
        .env("PATH", PATH_ENV)
        .output()
        .expect("real binary version probe runs");
    assert!(
        output.status.success(),
        "real binary version probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("real binary version is UTF-8")
        .split_whitespace()
        .find(|candidate| {
            let parts = candidate.split('.').collect::<Vec<_>>();
            parts.len() == 3 && parts.iter().all(|part| part.parse::<u64>().is_ok())
        })
        .expect("real binary reports an exact semantic version")
        .to_string()
}

pub(crate) fn next_patch_version(version: &str) -> String {
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u64>().expect("semantic version component"))
        .collect::<Vec<_>>();
    assert_eq!(
        parts.len(),
        3,
        "fixture version must be exact semantic version"
    );
    parts[2] += 1;
    format!("{}.{}.{}", parts[0], parts[1], parts[2])
}

pub(crate) fn stage_current(home: &Path, name: &str, version: &str, binary: &Path) {
    let coordinate = home
        .join(".stado/releases")
        .join(name)
        .join(version)
        .join(release_platform());
    fs::create_dir_all(&coordinate).expect("staged release coordinate");
    fs::copy(binary, coordinate.join(name)).expect("stage delivered binary bytes");
}

pub(crate) fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no convergence platform mapping for {os}-{arch}"),
    }
}

pub(crate) fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("hostname command runs");
    assert!(output.status.success(), "hostname command succeeds");
    String::from_utf8(output.stdout)
        .expect("hostname is UTF-8")
        .trim()
        .to_string()
}

pub(crate) fn unused_loopback_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved listener address")
        .port()
}

pub(crate) fn skarbiec_generated_bearer() -> String {
    let output = Command::new(real_skarbiec_binary())
        .args([
            "generate", "--length", "64", "--lower", "--upper", "--digits",
        ])
        .env_clear()
        .env("HOME", std::env::var_os("HOME").expect("HOME is set"))
        .env("PATH", PATH_ENV)
        .output()
        .expect("real Skarbiec generates registry API bearer");
    assert!(
        output.status.success(),
        "real Skarbiec bearer generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let generated: Value =
        serde_json::from_slice(&output.stdout).expect("Skarbiec generation response is JSON");
    let bearer = generated["password"]
        .as_str()
        .expect("Skarbiec generation response carries password")
        .to_string();
    assert_eq!(bearer.len(), 64, "Skarbiec generated the requested bearer");
    bearer
}

pub(crate) fn digest(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).expect("read fixture file")))
}

pub(crate) fn binary_rows(report: &Value) -> BTreeMap<&str, &Value> {
    report["binaries"]
        .as_array()
        .expect("report carries binaries")
        .iter()
        .map(|row| (row["binary"].as_str().expect("row names its binary"), row))
        .collect()
}

pub(crate) fn assert_error(answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "unexpected response: {}",
        answer.body
    );
    assert_eq!(answer.body["ok"], false, "error envelope: {}", answer.body);
    assert_eq!(
        answer.body["error"]["code"], code,
        "error envelope: {}",
        answer.body
    );
    assert!(
        answer.body["error"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "error envelope has no diagnosis: {}",
        answer.body
    );
}
