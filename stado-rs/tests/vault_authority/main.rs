//! `stado credentials vault` answers which vault this machine's credential
//! operations resolve to, and refuses when nothing does.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) against a
//! real temp `HOME` holding real vault files, with `STADO_CONFIG` pointing at a
//! path that does not exist so the operator's own configuration cannot leak in.
//! Nothing is stubbed: the command runs the same resolver every credential
//! write and every authoritative read in the product runs, and the assertions
//! read its exit status and the exact refusal sentences.
//!
//! # The incident
//!
//! On 2026-09-05 `lukasz-macbook` held two vaults claiming one owner —
//! `~/.local/share/skarbiec/skarbiec.vault.json` with 660 items and
//! `~/.stado/skarbiec.vault.json` with 626 — because the `skarbiec` CLI
//! defaults to the first and Stado used to name the second. Six `skarbiec
//! set-json` writes were simultaneously real, active on the host, and
//! invisible to `stado host reconcile-release-verifier`, which closed the
//! fleet's release publication boundary for every product: `stado doctor
//! --deployment-preflight` failed `object-auth` with seven publisher items
//! missing from the release verifier's grant, and the repair could not run
//! because the resolver refused an ambiguous machine.
//!
//! What is defended here: the refusal states both paths and the durable way to
//! answer it; a declaration resolves and is reported as `declared`; a
//! declaration naming a file the machine does not hold is refused rather than
//! silently discovered around; and a machine with one candidate needs no
//! declaration at all.

use std::path::Path;
use std::process::{Command, Output};

/// A vault file as `vault_identity` reads it: the plaintext envelope carries
/// the owner and the item map, and no value is ever decrypted to answer which
/// vault this is.
fn vault(path: &Path, owner: &str, items: usize) {
    std::fs::create_dir_all(path.parent().expect("vault has a parent")).unwrap();
    let items: serde_json::Map<String, serde_json::Value> = (0..items)
        .map(|index| (format!("item-{index}"), serde_json::json!({})))
        .collect();
    std::fs::write(
        path,
        serde_json::json!({"owner": owner, "items": items}).to_string(),
    )
    .unwrap();
}

fn run(home: &Path, declared: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(["credentials", "vault", "--json"])
        .current_dir(home)
        .env("HOME", home)
        // A config that does not exist: the resolver's answer must come from
        // this test's declaration, never from the machine running it.
        .env("STADO_CONFIG", home.join("no-such-config.json"))
        .env_remove("SKARBIEC_VAULT_FILE");
    if let Some(declared) = declared {
        command.env("SKARBIEC_VAULT_FILE", declared);
    }
    command.output().expect("stado credentials vault runs")
}

fn report(out: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "expected a JSON report, got {error}: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn two_vaults_under_one_owner_are_refused_and_the_refusal_says_how_to_answer_it() {
    let home = tempfile::tempdir().expect("temp home");
    vault(
        &home
            .path()
            .join(".local/share/skarbiec/skarbiec.vault.json"),
        "owner-one",
        660,
    );
    vault(
        &home.path().join(".stado/skarbiec.vault.json"),
        "owner-one",
        626,
    );
    let out = run(home.path(), None);
    assert!(
        !out.status.success(),
        "an unresolvable machine must exit non-zero so a script can gate on it"
    );
    let report = report(&out);
    assert_eq!(report["state"], "ambiguous");
    assert_eq!(report["path"], serde_json::Value::Null);
    assert_eq!(report["candidates"].as_array().map(Vec::len), Some(2));
    let refusal = report["refusal"].as_str().expect("a refusal sentence");
    for expected in [
        "vaults that all claim owner owner-one",
        "There is no single authoritative vault",
        "stado config set secrets.skarbiec.vault_file <path>",
        "stado host config-set <target> secrets.skarbiec.vault_file <path>",
        "Nothing is merged for you",
    ] {
        assert!(
            refusal.contains(expected),
            "missing {expected:?}: {refusal}"
        );
    }
    // Both rivals are named, with their counts, because "one of your vaults"
    // is not something an operator can act on.
    assert!(
        refusal.contains("(660 items)") && refusal.contains("(626 items)"),
        "{refusal}"
    );
}

#[test]
fn a_declared_vault_is_the_answer_for_a_machine_that_holds_two() {
    let home = tempfile::tempdir().expect("temp home");
    vault(
        &home
            .path()
            .join(".local/share/skarbiec/skarbiec.vault.json"),
        "owner-one",
        660,
    );
    let declared = home.path().join(".stado/skarbiec.vault.json");
    vault(&declared, "owner-one", 626);
    let out = run(home.path(), Some(declared.to_str().expect("utf-8 path")));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report["state"], "declared");
    assert_eq!(report["path"], declared.to_string_lossy().as_ref());
    assert_eq!(report["refusal"], serde_json::Value::Null);
}

#[test]
fn a_declaration_naming_a_file_this_machine_does_not_hold_is_refused() {
    let home = tempfile::tempdir().expect("temp home");
    vault(
        &home.path().join(".stado/skarbiec.vault.json"),
        "owner-one",
        626,
    );
    let absent = home.path().join(".stado/somewhere-else.vault.json");
    let out = run(home.path(), Some(absent.to_str().expect("utf-8 path")));
    assert!(
        !out.status.success(),
        "a declaration that resolves to nothing must not fall back to discovery"
    );
    let report = report(&out);
    let refusal = report["refusal"].as_str().expect("a refusal sentence");
    assert!(
        refusal.contains("the declared owner vault") && refusal.contains("is not a file"),
        "{refusal}"
    );
    assert!(
        refusal.contains("secrets.skarbiec.vault_file"),
        "the refusal names the key that is wrong: {refusal}"
    );
}

#[test]
fn one_candidate_needs_no_declaration() {
    let home = tempfile::tempdir().expect("temp home");
    let only = home.path().join(".stado/skarbiec.vault.json");
    vault(&only, "owner-one", 12);
    let out = run(home.path(), None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report["state"], "discovered");
    assert_eq!(report["path"], only.to_string_lossy().as_ref());
    assert_eq!(report["declared"], serde_json::Value::Null);
}

#[test]
fn two_owners_are_two_machines_worth_of_items_not_an_ambiguous_one() {
    // Discovery is not confused by a second vault under a different owner:
    // that is another machine's store sitting in this home, and the first
    // candidate in Skarbiec's own search order is still this machine's.
    let home = tempfile::tempdir().expect("temp home");
    let first = home
        .path()
        .join(".local/share/skarbiec/skarbiec.vault.json");
    vault(&first, "owner-one", 40);
    vault(
        &home.path().join(".stado/skarbiec.vault.json"),
        "owner-two",
        7,
    );
    let out = run(home.path(), None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = report(&out);
    assert_eq!(report["state"], "discovered");
    assert_eq!(report["path"], first.to_string_lossy().as_ref());
}

#[test]
fn a_machine_holding_no_candidate_says_so_rather_than_creating_one() {
    let home = tempfile::tempdir().expect("temp home");
    // A vault file discovery never searches: holding it is not holding a
    // credential store this machine can write through.
    vault(
        &home.path().join(".stado/weles-skarbiec.vault.json"),
        "owner-one",
        62,
    );
    let out = run(home.path(), None);
    assert!(!out.status.success());
    let report = report(&out);
    assert_eq!(report["state"], "none");
    assert_eq!(report["candidates"].as_array().map(Vec::len), Some(0));
    let refusal = report["refusal"].as_str().expect("a refusal sentence");
    assert!(refusal.contains("no owner vault in"), "{refusal}");
    assert!(
        !home.path().join(".stado/skarbiec.vault.json").exists(),
        "resolution must never create a vault"
    );
}
