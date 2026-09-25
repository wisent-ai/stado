//! The vault this journey reads its signing identity from, started the way a
//! fleet builder would find it.
//!
//! The grant is minted before the broker starts, since this fixture's broker
//! reads its vault at startup.

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
        // `cannot read desktop-signing-apple-development#certificate`. A
        // Linux build signs no Apple code and its builder holds no fleet
        // vault, so there the item is neither read nor granted: reading it
        // anyway refused every Linux journey with `vault not initialized`.
        let mut items = vec![item];
        let mut grants = String::from("read:ci-release-signing#private_key");
        if cfg!(target_os = "macos") {
            items.push(SkarbiecItem::new(
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
            ));
            grants.push_str(
                ",read:desktop-signing-apple-development#certificate,\
                 read:desktop-signing-apple-development#private_key",
            );
        }
        let token = home.join(".stado/stado-skarbiec-token");
        fs::create_dir_all(token.parent().unwrap()).unwrap();
        Self::start(home, &items, token, Some(("stado", &grants)), |_, _| {})
    }
}

/// Read a fleet secret through the operator's Skarbiec CLI. Failure blocks
/// the journey instead of supplying a fixture value.
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
