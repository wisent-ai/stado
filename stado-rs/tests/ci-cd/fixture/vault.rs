//! The vault this journey reads its signing identity from, started the way a
//! fleet builder would find it.
//!
//! The grant is minted in the provision hook rather than afterwards, because
//! the broker reads its vault once at start and a grant written later answered
//! with a refusal.

use super::super::*;

impl SkarbiecFixture {
    pub(crate) fn start_release(home: &Path, private_key: &Path) -> Self {
        use base64::Engine;

        let encoded =
            base64::engine::general_purpose::STANDARD.encode(fs::read(private_key).unwrap());
        let item = SkarbiecItem::new(
            "ci-release-signing",
            "key-pair",
            json!({
                "schema": "skarbiec.item.v2",
                "kind": "key-pair",
                "fields": {"private_key": encoded},
                "context": {"service": "stado-release"}
            }),
        );
        // The darwin signing step reads the Apple identity from
        // `desktop-signing-apple-development` - through the broker, and when
        // the broker cannot serve it, straight from the owner vault this
        // isolated home points at. The journey signs with the fleet's real
        // identity, read through the product's own secret command the way a
        // fleet builder reads it; without it every darwin journey ended at
        // `cannot read desktop-signing-apple-development#certificate`.
        let apple = SkarbiecItem::new(
            "desktop-signing-apple-development",
            "bundle",
            json!({
                "schema": "skarbiec.item.v2",
                "kind": "bundle",
                "fields": {
                    "certificate": fleet_secret("desktop-signing-apple-development", "certificate"),
                    "private_key": fleet_secret("desktop-signing-apple-development", "private_key"),
                },
                "context": {"service": "native-signing"}
            }),
        );
        // Inside the build job the signing step reads the identity through
        // the broker as consumer `stado-control-plane`, with the grant file
        // at `$HOME/.stado/control-plane-skarbiec-token` - the route a fleet
        // builder takes. The grant is minted before the broker serves, in the
        // fixture's provision hook, because the broker reads its vault once
        // at start: a grant written to the file afterwards answered 403.
        let token = home.join(".stado/control-plane-skarbiec-token");
        fs::create_dir_all(token.parent().unwrap()).unwrap();
        let token_for_hook = token.clone();
        let home_for_hook = home.to_path_buf();
        Self::start(
            home,
            &[item, apple],
            home.join("release-signing-grant"),
            Some((
                "stado-release-coordinator",
                "read:ci-release-signing#private_key",
            )),
            move |gnupg, vault| {
                let minted = Command::new(skarbiec_support::real_skarbiec_binary())
                    .env_clear()
                    .env("HOME", &home_for_hook)
                    .env("GNUPGHOME", gnupg)
                    .env("SKARBIEC_VAULT_FILE", vault)
                    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                    .args([
                        "grant",
                        "issue",
                        "stado-control-plane",
                        "--capabilities",
                        "read:desktop-signing-apple-development#certificate,\
                         read:desktop-signing-apple-development#private_key",
                    ])
                    .output()
                    .expect("the Skarbiec CLI runs");
                assert!(
                    minted.status.success(),
                    "real Skarbiec refused the control-plane signing grant: {}",
                    String::from_utf8_lossy(&minted.stderr)
                );
                let grant: Value = serde_json::from_slice(&minted.stdout).unwrap();
                fs::write(&token_for_hook, grant["token"].as_str().unwrap()).unwrap();
                fs::set_permissions(&token_for_hook, fs::Permissions::from_mode(0o600)).unwrap();
            },
        )
    }
}

/// One field of a fleet secret, read through the operator's own Skarbiec CLI:
/// the owner read, which is also the fallback the signing step itself uses
/// when the broker will not serve the item. The Stado profile's broker read
/// refuses `local-operator` for this item with 403, so `stado secrets get`
/// is not the route. A refusal blocks the journey and says so.
fn fleet_secret(item: &str, field: &str) -> String {
    let binary = skarbiec_support::real_skarbiec_binary();
    let read = Command::new(&binary)
        .args(["get", item, "--field", field])
        .output()
        .expect("the Skarbiec CLI runs");
    assert!(
        read.status.success(),
        "blocked: the fleet secret {item}#{field} could not be read with {}: {}",
        binary.display(),
        String::from_utf8_lossy(&read.stderr)
    );
    String::from_utf8(read.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}
