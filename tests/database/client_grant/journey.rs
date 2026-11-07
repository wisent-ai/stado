//! `stado database client NAME` grants the library client only the fields
//! the database's item holds in the owner vault, and says which it left out
//! and which vault host granted.
//!
//! The real `stado` and `skarbiec` run with an isolated `HOME` under the
//! checkout's build directory. The vault owner side runs on this machine as
//! it runs on any owner: `~/.stado/bin/stado` and `~/.stado/bin/skarbiec`
//! (the binaries under test) against `~/.config/stado/config.json`, which
//! declares the run's own vault. The registry store is a loopback listener
//! that resets every connection, so the last-known-good copy is read: the
//! shared directory template with this machine as its first target and
//! Skarbiec placed on it. The client's bearer is seeded with a real grant.
//! A fleet item `stado database migrate` wrote is granted its connection
//! fields and `provider`, and reports its REST and sign-in addresses and
//! keys under `not_granted`, each with the command that writes it; once the
//! item carries them (and `realtime_url`), the same command grants every one,
//! which is what the provider's client library reads to reach a database
//! moved off its vendor. A hosted item that records no provider is told its
//! `url` and keys with `adopt`; a provider Stado does not know is refused by
//! name; an item without `pooler_url` is refused naming the host and
//! granting nothing. The vault's stored grant is read back after each step,
//! and every command is kept beside the run.
mod run;

use serde_json::{json, Value};

use run::Run;

const DATABASE: &str = "ledger";
const ITEM: &str = "ledger-database";
const CONSUMER: &str = "ledger-database-client";
const CONNECTION: [&str; 3] = ["pooler_url", "session_url", "ca_certificate"];
const FLEET: [&str; 5] = ["rest_url", "auth_url", "realtime_url", "anon_key", "service_role_key"];

fn sorted(fields: &[&str]) -> Vec<String> {
    let mut owned: Vec<String> = fields.iter().map(|field| field.to_string()).collect();
    owned.sort();
    owned
}

#[test]
fn the_client_is_granted_the_fields_its_item_holds_and_told_the_rest() {
    let run = Run::new("held");
    Run::succeeded(&run.stado(&["config", "init"]));
    Run::succeeded(&run.skarbiec(&["init", "database-client-grant-journey"], None));
    let vault = run.root.join("vault.json");
    Run::succeeded(&run.stado(&[
        "config",
        "set",
        "secrets.skarbiec.vault_file",
        vault.to_str().expect("UTF-8 path"),
    ]));
    run.place_vault_here();
    let token = run
        .root
        .join(".stado")
        .join(format!("{CONSUMER}-skarbiec-token"));
    std::fs::write(&token, "client-grant-journey-bearer").expect("seed the client's bearer");
    run.store_item(&json!({ "pooler_url": "postgres://p", "session_url": "postgres://s", "ca_certificate": "c" }));
    let token_path = token.to_str().expect("UTF-8 path");
    let seeded = format!("read:{ITEM}#pooler_url");
    Run::succeeded(&run.skarbiec(
        &[
            "grant",
            "issue",
            CONSUMER,
            "--capabilities",
            &seeded,
            "--token-file",
            token_path,
        ],
        None,
    ));

    // A fleet item `stado database migrate` wrote, before its REST API and
    // sign-in are placed: the connection fields and `provider` are granted,
    // and each address and key is named with the command that writes it.
    let moved = json!({
        "provider": "fleet", "pooler_url": "postgres://p", "session_url": "postgres://s", "ca_certificate": "c",
    });
    run.store_item(&moved);
    let first = run.client();
    let answer: Value = serde_json::from_str(&Run::succeeded(&first)).expect("client answers JSON");
    let mut expected: Vec<&str> = CONNECTION.to_vec();
    expected.push("provider");
    assert_eq!(answer["fields"], json!(expected), "{answer}");
    let left: Vec<&str> = answer["not_granted"]
        .as_array()
        .expect("not_granted lists the rest")
        .iter()
        .map(|row| row["field"].as_str().expect("each row names its field"))
        .collect();
    assert_eq!(left, ["rest_url", "auth_url", "anon_key", "service_role_key"], "{answer}");
    let why = |field: &str| {
        answer["not_granted"]
            .as_array()
            .expect("not_granted lists the rest")
            .iter()
            .find(|row| row["field"] == field)
            .and_then(|row| row["why"].as_str())
            .unwrap_or_else(|| panic!("no reason for {field}: {answer}"))
            .to_string()
    };
    assert!(why("rest_url").contains(&format!("stado database rest place {DATABASE} ")), "{answer}");
    assert!(why("auth_url").contains(&format!("stado database auth place {DATABASE} ")), "{answer}");
    assert!(
        !answer["owner"]
            .as_str()
            .expect("the granting host")
            .is_empty(),
        "{answer}"
    );
    assert_eq!(
        run.granted_fields(),
        sorted(&expected),
        "the vault holds exactly the connection fields and the provider"
    );

    // Once its API and sign-in are placed, the same command grants every
    // address and key: what the provider's client library reads to reach a
    // database moved onto the fleet. realtime_url is granted when the item
    // holds it and never named missing, since the fleet serves no Realtime.
    let mut placed = moved.clone();
    for field in FLEET {
        placed[field] = json!(format!("{field}-value"));
    }
    run.store_item(&placed);
    let second = run.client();
    let answer: Value =
        serde_json::from_str(&Run::succeeded(&second)).expect("client answers JSON");
    assert_eq!(answer["not_granted"], json!([]), "{answer}");
    let all: Vec<&str> = expected.iter().chain(FLEET.iter()).copied().collect();
    assert_eq!(
        run.granted_fields(),
        sorted(&all),
        "a fleet item's addresses and keys are granted once it holds them"
    );

    // A hosted item that records no provider: its one url and keys, each
    // named with `adopt` when missing.
    run.store_item(&json!({ "pooler_url": "postgres://p", "session_url": "postgres://s", "ca_certificate": "c" }));
    let hosted = run.client();
    let answer: Value =
        serde_json::from_str(&Run::succeeded(&hosted)).expect("client answers JSON");
    let left: Vec<&str> = answer["not_granted"]
        .as_array()
        .expect("not_granted lists the rest")
        .iter()
        .map(|row| row["field"].as_str().expect("each row names its field"))
        .collect();
    assert_eq!(left, ["url", "anon_key", "service_role_key"], "{answer}");
    assert!(
        answer["not_granted"][0]["why"]
            .as_str()
            .expect("a reason")
            .contains(&format!("stado database adopt {DATABASE}")),
        "{answer}"
    );

    run.store_item(&json!({ "provider": "elsewhere", "pooler_url": "postgres://p" }));
    let unknown = run.client();
    let said = String::from_utf8_lossy(&unknown.stderr).into_owned();
    assert!(!unknown.status.success(), "a provider Stado does not know is refused: {said}");
    assert!(said.contains(&format!("{ITEM}.provider is \"elsewhere\"")), "{said}");

    run.store_item(&json!({ "session_url": "postgres://s" }));
    let refused = run.client();
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "an item without its pooler is refused: {said}"
    );
    assert!(
        said.contains(&format!("{ITEM} on ")) && said.contains("holds no pooler_url"),
        "{said}"
    );
    assert_eq!(
        run.granted_fields(),
        sorted(&all),
        "a refusal grants nothing more"
    );
}
