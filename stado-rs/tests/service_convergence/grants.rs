//! The grants this API is reached with: minting one, minting it again, and
//! the two target-local refusals a bad grant file earns.
use super::*;

/// Runs the mint, reuse, symlink and empty-file steps against the started
/// fixture, and leaves the protected file, the binaries and the vault as it
/// found them.
pub(crate) fn grant_refusals(fixture: &DashboardFixture, protected_baseline: &str) {
    let verifier_token = fixture
        .vault
        .as_ref()
        .expect("authenticated fixture has a real Skarbiec vault")
        .token
        .clone();
    let verifier_token_json = verifier_token
        .to_str()
        .expect("isolated verifier token path is UTF-8");
    let first_mint = fixture
        .verifier_mint
        .as_ref()
        .expect("fixture retains the first built-Stado mint receipt");
    assert_eq!(first_mint["target"], HOST);
    assert_eq!(first_mint["status"], "token_minted");
    assert_eq!(first_mint["skarbiec"]["token_file"], verifier_token_json);
    assert!(
        first_mint["skarbiec"].get("token").is_none(),
        "file-backed mint included a token field in JSON"
    );
    let first_bearer = fs::read(&verifier_token).expect("read persisted verifier bearer");
    let first_bearer_text = std::str::from_utf8(&first_bearer)
        .expect("persisted verifier bearer is UTF-8")
        .trim();
    assert!(
        !first_bearer_text.is_empty(),
        "persisted verifier bearer is empty"
    );
    assert!(
        !first_mint.to_string().contains(first_bearer_text),
        "file-backed mint included bearer bytes in JSON"
    );
    assert_eq!(
        fs::metadata(&verifier_token)
            .expect("persisted verifier bearer metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "persisted verifier bearer is not owner-only"
    );

    let repeated = fixture.mint_verifier("registry-api-verifier-grant");
    assert!(
        repeated.status.success(),
        "built Stado failed to reuse the verifier bearer: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    let repeated_receipt: Value = serde_json::from_slice(&repeated.stdout)
        .expect("repeated verifier bearer receipt from built Stado is JSON");
    assert_eq!(repeated_receipt["target"], HOST);
    assert_eq!(repeated_receipt["status"], "token_minted");
    assert_eq!(
        repeated_receipt["skarbiec"]["token_file"],
        verifier_token_json
    );
    assert!(
        repeated_receipt["skarbiec"].get("token").is_none(),
        "repeated file-backed mint included a token field in JSON"
    );
    assert!(
        !String::from_utf8_lossy(&repeated.stdout).contains(first_bearer_text),
        "repeated file-backed mint included bearer bytes in JSON"
    );
    assert!(
        fs::read(&verifier_token).expect("re-read persisted verifier bearer") == first_bearer,
        "repeated provisioning lost or rotated the persisted verifier bearer"
    );
    assert_eq!(
        fs::metadata(&verifier_token)
            .expect("reused verifier bearer metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "reused verifier bearer is not owner-only"
    );

    let vault_after_reuse = digest(
        fixture
            .vault
            .as_ref()
            .expect("authenticated fixture has a real Skarbiec vault")
            .vault_file(),
    );
    let symlink_token = fixture.home.join(".stado/refuse-symlink-grant");
    std::os::unix::fs::symlink(&fixture.protected, &symlink_token)
        .expect("create isolated token-file symlink");
    let symlink_refusal = fixture.mint_verifier("refuse-symlink-grant");
    assert!(
        !symlink_refusal.status.success(),
        "built Stado accepted a token-file symlink"
    );
    let symlink_diagnosis = String::from_utf8_lossy(&symlink_refusal.stderr);
    assert!(
        symlink_diagnosis.contains("token file must not be a symlink"),
        "unexpected symlink refusal: {symlink_diagnosis}"
    );
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(
        digest(
            fixture
                .vault
                .as_ref()
                .expect("authenticated fixture has a real Skarbiec vault")
                .vault_file()
        ),
        vault_after_reuse,
        "symlink refusal changed the real vault"
    );
    println!(
        "captured target-local symlink refusal: {}",
        symlink_diagnosis.trim()
    );

    let empty_token = fixture.home.join(".stado/refuse-empty-grant");
    fs::write(&empty_token, b"").expect("create isolated empty token file");
    fs::set_permissions(&empty_token, fs::Permissions::from_mode(0o600))
        .expect("protect isolated empty token file");
    let empty_refusal = fixture.mint_verifier("refuse-empty-grant");
    assert!(
        !empty_refusal.status.success(),
        "built Stado accepted an empty token file"
    );
    let empty_diagnosis = String::from_utf8_lossy(&empty_refusal.stderr);
    assert!(
        empty_diagnosis.contains("token file must be a nonempty regular file"),
        "unexpected empty-file refusal: {empty_diagnosis}"
    );
    assert_eq!(
        fs::metadata(&empty_token)
            .expect("empty refusal file remains")
            .len(),
        0,
        "empty-file refusal replaced the protected path"
    );
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(
        digest(
            fixture
                .vault
                .as_ref()
                .expect("authenticated fixture has a real Skarbiec vault")
                .vault_file()
        ),
        vault_after_reuse,
        "empty-file refusal changed the real vault"
    );
    println!(
        "captured target-local empty-file refusal: {}",
        empty_diagnosis.trim()
    );
}
