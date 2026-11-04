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
//! A Postgres item without the REST API's fields is granted its connection
//! fields and reports the REST fields under `not_granted`; once the item
//! carries them, the same command grants them; an item without `pooler_url`
//! is refused naming the host and granting nothing. The vault's stored grant
//! is read back after each step, and every command is kept beside the run.
mod run;

use serde_json::{json, Value};

use run::Run;

const DATABASE: &str = "ledger";
const ITEM: &str = "ledger-database";
const CONSUMER: &str = "ledger-database-client";
const CONNECTION: [&str; 3] = ["pooler_url", "session_url", "ca_certificate"];
const REST: [&str; 3] = ["rest_url", "anon_key", "service_role_key"];

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

    let first = run.client();
    let answer: Value = serde_json::from_str(&Run::succeeded(&first)).expect("client answers JSON");
    assert_eq!(answer["fields"], json!(CONNECTION), "{answer}");
    let left: Vec<&str> = answer["not_granted"]
        .as_array()
        .expect("not_granted lists the rest")
        .iter()
        .map(|row| row["field"].as_str().expect("each row names its field"))
        .collect();
    assert_eq!(left, REST.to_vec(), "{answer}");
    assert!(
        answer["not_granted"][0]["why"]
            .as_str()
            .expect("a reason")
            .contains("stado database rest place"),
        "{answer}"
    );
    assert!(
        !answer["owner"]
            .as_str()
            .expect("the granting host")
            .is_empty(),
        "{answer}"
    );
    assert_eq!(
        run.granted_fields(),
        sorted(&CONNECTION),
        "the vault holds exactly the connection fields"
    );

    let mut placed = json!({ "pooler_url": "postgres://p", "session_url": "postgres://s", "ca_certificate": "c" });
    for field in REST {
        placed[field] = json!(format!("{field}-value"));
    }
    run.store_item(&placed);
    let second = run.client();
    let answer: Value =
        serde_json::from_str(&Run::succeeded(&second)).expect("client answers JSON");
    assert_eq!(answer["not_granted"], json!([]), "{answer}");
    let all: Vec<&str> = CONNECTION.iter().chain(REST.iter()).copied().collect();
    assert_eq!(
        run.granted_fields(),
        sorted(&all),
        "the REST fields are granted once the item holds them"
    );

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
