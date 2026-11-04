//! A program Stado signs carries the `com.wisent.<product>` name every
//! Wisent name follows, and one signed earlier under the retired
//! `ai.wisent.<product>.<product>` spelling moves to it instead of being
//! refused as an identity change. Any other change is still refused.
//!
//! The real `stado` this build made signs copies of itself, kept under the
//! checkout's build directory, with the signing identity the operator names
//! for product signing (`WISENT_CODESIGN_KEYCHAIN` and
//! `WISENT_CODESIGN_IDENTITY`); `codesign -dv` reads what each copy carries.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    command.output().expect("the program runs")
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The identifier the code at `path` carries, as `codesign` reads it.
fn carried(path: &Path) -> String {
    let read = run(Command::new("/usr/bin/codesign").arg("-dv").arg(path));
    said(&read)
        .lines()
        .find_map(|line| line.strip_prefix("Identifier="))
        .unwrap_or_else(|| panic!("codesign names no identifier of {}: {}", path.display(), said(&read)))
        .to_string()
}

fn sign(path: &Path, naming: &[&str], previous: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command.args(["product", "signing", "sign", "--json"]).args(naming);
    if let Some(previous) = previous {
        command.arg("--previous").arg(previous);
    }
    run(command.arg(path))
}

/// A journey's own directory, removed when the journey ends, passed or
/// failed: each holds copies of the real `stado`, hundreds of megabytes, and
/// a failed run used to leave them under the checkout's build directory.
struct Owned(PathBuf);

impl Owned {
    fn new(kind: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.build")
            .join(kind)
            .join(std::process::id().to_string());
        std::fs::create_dir_all(&path).expect("create the journey's directory");
        Self(path)
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("the journey's directory {} was not removed: {error}", self.0.display());
        }
    }
}

#[test]
fn a_signed_program_carries_the_com_wisent_name() {
    for variable in ["WISENT_CODESIGN_KEYCHAIN", "WISENT_CODESIGN_IDENTITY"] {
        assert!(
            std::env::var_os(variable).is_some(),
            "{variable} names the signing identity this journey signs with; it is not set"
        );
    }
    let owned = Owned::new("signing-convention");
    let root = owned.0.clone();

    // A program named after its product, never signed: com.wisent.<product>.
    let fresh = root.join("probe");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &fresh).expect("copy the real stado");
    let signed = sign(&fresh, &["--product", "probe"], None);
    assert!(signed.status.success(), "a fresh program was not signed: {}", said(&signed));
    assert_eq!(carried(&fresh), "com.wisent.probe");

    // Another program of the product: com.wisent.<product>.<name>.
    let helper = root.join("probe-helper");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &helper).expect("copy the real stado");
    let signed = sign(&helper, &["--product", "probe"], None);
    assert!(signed.status.success(), "a helper program was not signed: {}", said(&signed));
    assert_eq!(carried(&helper), "com.wisent.probe.probe-helper");

    // An installed program under the retired spelling moves to the convention.
    // Stado no longer signs that spelling, so the installed copy is signed by
    // codesign itself with the same identity, as an earlier Stado signed it.
    let installed = root.join("installed");
    codesigned_copy(&installed, "ai.wisent.probe.probe");
    let candidate = root.join("candidate");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &candidate).expect("copy the real stado");
    let moved = sign(&candidate, &["--product", "probe"], Some(&installed));
    assert!(moved.status.success(), "the retired spelling was refused: {}", said(&moved));
    assert_eq!(carried(&candidate), "com.wisent.probe");

    // A nested program that named no owner (`skarbiec`) moves to its
    // convention name, given explicitly.
    let bare = root.join("bare");
    codesigned_copy(&bare, "skarbiec");
    let owned = root.join("owned");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &owned).expect("copy the real stado");
    let moved = sign(&owned, &["--identifier", "com.wisent.skarbiec"], Some(&bare));
    assert!(moved.status.success(), "a bare identifier was not moved: {}", said(&moved));
    assert_eq!(carried(&owned), "com.wisent.skarbiec");

    // Any other change of identifier is still refused, naming both.
    let other = root.join("other");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &other).expect("copy the real stado");
    let refused = sign(&other, &["--identifier", "com.wisent.another"], Some(&installed));
    assert!(!refused.status.success(), "an identity change was signed: {}", said(&refused));
    assert!(
        said(&refused).contains("code identifier would change: \"ai.wisent.probe.probe\" -> com.wisent.another"),
        "the refusal does not name both identifiers: {}",
        said(&refused)
    );

    // An identifier outside the convention is refused before anything is
    // signed, naming the convention name of a retired one.
    let outside = root.join("outside");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &outside).expect("copy the real stado");
    let refused = sign(&outside, &["--identifier", "ai.wisent.probe.probe"], None);
    assert!(!refused.status.success(), "a retired identifier was signed: {}", said(&refused));
    assert!(
        said(&refused).contains("code identifier ai.wisent.probe.probe is not a com.wisent.<product> name")
            && said(&refused).contains("(ai.wisent.probe.probe is com.wisent.probe now)"),
        "the refusal does not name the convention: {}",
        said(&refused)
    );

    drop(owned);
}

/// A copy of the real `stado` at `path`, signed by codesign with `identifier`
/// and the identity the journey names, as a build before the convention
/// left it installed.
fn codesigned_copy(path: &Path, identifier: &str) {
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), path).expect("copy the real stado");
    let keychain = std::env::var("WISENT_CODESIGN_KEYCHAIN").expect("the signing keychain is named");
    let identity = std::env::var("WISENT_CODESIGN_IDENTITY").expect("the signing identity is named");
    // codesign finds an identity only in a keychain on the user's search
    // list. Stado unlocks a keychain under ~/.stado/signing with its empty
    // password and puts it first for each signature, then takes it off; this
    // does the same, and puts the list back as it was.
    let unlocked = run(Command::new("/usr/bin/security").args(["unlock-keychain", "-p", "", &keychain]));
    assert!(unlocked.status.success(), "the signing keychain {keychain} could not be unlocked: {}", said(&unlocked));
    let listed = run(Command::new("/usr/bin/security").args(["list-keychains", "-d", "user"]));
    let before: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(|line| line.trim().trim_matches('"').to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let mut first = vec!["list-keychains".to_string(), "-d".into(), "user".into(), "-s".into(), keychain.clone()];
    first.extend(before.iter().filter(|listed| **listed != keychain).cloned());
    let put = run(Command::new("/usr/bin/security").args(&first));
    assert!(put.status.success(), "the signing keychain could not be listed: {}", said(&put));
    let signed = run(Command::new("/usr/bin/codesign")
        .args(["--force", "--sign", &identity, "--keychain", &keychain, "--identifier", identifier])
        .arg(path));
    let mut restore = vec!["list-keychains".to_string(), "-d".into(), "user".into(), "-s".into()];
    restore.extend(before);
    let restored = run(Command::new("/usr/bin/security").args(&restore));
    assert!(restored.status.success(), "the keychain search list was not put back: {}", said(&restored));
    assert!(signed.status.success(), "codesign could not sign {} as {identifier}: {}", path.display(), said(&signed));
}

#[test]
fn the_convention_verb_lists_and_renames_retired_identifiers() {
    let owned = Owned::new("signing-convention-verb");
    let root = owned.0.clone();
    let git = |args: &[&str]| {
        let done = run(Command::new("git").arg("-C").arg(&root).args(args));
        assert!(done.status.success(), "git {args:?}: {}", said(&done));
    };
    git(&["init", "--quiet"]);
    std::fs::write(
        root.join("Info.plist"),
        "<string>ai.wisent.probe</string>\n<string>ai.wisent.identity.keychain-helper</string>\n",
    )
    .unwrap();
    std::fs::write(root.join("untracked.txt"), "ai.wisent.probe\n").unwrap();
    git(&["add", "Info.plist"]);

    let read = run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "signing", "convention", "--json"])
        .arg(&root));
    assert_eq!(read.status.code(), Some(1), "a read that finds retired identifiers exits 1: {}", said(&read));
    let listed = String::from_utf8_lossy(&read.stdout);
    assert!(
        listed.contains("\"convention\": \"com.wisent.probe.desktop\"")
            && !listed.contains("keychain-helper")
            && !listed.contains("untracked.txt"),
        "the read does not list the tracked file's identifiers alone: {listed}"
    );
    assert!(
        std::fs::read_to_string(root.join("Info.plist")).unwrap().contains("ai.wisent.probe"),
        "a read changed the file"
    );

    let wrote = run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "signing", "convention", "--write", "--json"])
        .arg(&root));
    assert!(wrote.status.success(), "--write failed: {}", said(&wrote));
    assert_eq!(
        std::fs::read_to_string(root.join("Info.plist")).unwrap(),
        "<string>com.wisent.probe.desktop</string>\n<string>ai.wisent.identity.keychain-helper</string>\n",
        "the shared Keychain helper's identifier was renamed; every signed-in session's item trusts it by name"
    );
    assert_eq!(std::fs::read_to_string(root.join("untracked.txt")).unwrap(), "ai.wisent.probe\n");

    drop(owned);
}

/// An application renamed to `com.wisent.probe.desktop` finds what it kept
/// as `ai.wisent.probe` before it next runs: `stado product restart
/// --if-running` (the step every desktop installation runs after placing a
/// bundle) carries its preferences, with their own retired keys renamed, and
/// its support directory, never over a place the new name already has. The
/// bundle is never running here, so nothing is launched.
#[test]
fn a_renamed_application_keeps_what_it_kept_under_its_retired_name() {
    let owned = Owned::new("signing-convention-carry");
    let home = owned.0.clone();
    let app = home.join("Applications/Probe.app");
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    let bundle_id = format!("com.wisent.probe{}.desktop", std::process::id());
    let retired = bundle_id.replace("com.wisent.", "ai.wisent.").replace(".desktop", "");
    std::fs::write(
        app.join("Contents/Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{bundle_id}</string></dict></plist>\n"
        ),
    )
    .unwrap();
    let preferences = home.join("Library/Preferences");
    std::fs::create_dir_all(&preferences).unwrap();
    let mut kept = plist::Dictionary::new();
    kept.insert(format!("{retired}.onboarding.done"), plist::Value::Boolean(true));
    kept.insert("workspace".into(), plist::Value::String("/w".into()));
    plist::Value::Dictionary(kept)
        .to_file_binary(preferences.join(format!("{retired}.plist")))
        .unwrap();
    let support = home.join("Library/Application Support");
    std::fs::create_dir_all(support.join(&retired)).unwrap();
    std::fs::write(support.join(&retired).join("state.json"), "{\"kept\":true}").unwrap();
    // The new name already has its caches: they are never overwritten.
    let caches = home.join("Library/Caches");
    std::fs::create_dir_all(caches.join(&retired)).unwrap();
    std::fs::write(caches.join(&retired).join("old"), "old").unwrap();
    std::fs::create_dir_all(caches.join(&bundle_id)).unwrap();
    std::fs::write(caches.join(&bundle_id).join("new"), "new").unwrap();

    let restart = || {
        run(Command::new(env!("CARGO_BIN_EXE_stado"))
            .env("HOME", &home)
            .args(["product", "restart", "--if-running", "--json"])
            .arg(&app))
    };
    let first = restart();
    assert!(first.status.success(), "the restart step failed: {}", said(&first));
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).expect("the report is JSON");
    assert_eq!(report["state"], "not-running", "{report}");
    // Each retired spelling is reported (`ai.wisent.<p>.desktop` and
    // `ai.wisent.<p>`); the journey kept its data under the second.
    let state = |place: &str| {
        report["carried"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                let named = row["from"]
                    .as_str()
                    .and_then(|from| Path::new(from).file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let name = named.trim_end_matches(".plist").trim_end_matches(".savedState");
                row["to"].as_str().is_some_and(|to| to.contains(place)) && name == retired
            })
            .map(|row| row["state"].as_str().unwrap().to_string())
    };
    assert_eq!(state("Preferences").as_deref(), Some("carried"), "{report}");
    assert_eq!(state("Application Support").as_deref(), Some("carried"), "{report}");
    assert_eq!(state("Caches").as_deref(), Some("kept"), "{report}");
    assert_eq!(state("HTTPStorages").as_deref(), Some("absent"), "{report}");

    let carried = plist::Value::from_file(preferences.join(format!("{bundle_id}.plist"))).unwrap();
    let carried = carried.as_dictionary().unwrap();
    let renamed_key = format!("{}.onboarding.done", bundle_id.trim_end_matches(".desktop"));
    assert!(
        carried.get(&renamed_key).is_some() || carried.keys().any(|key| key.starts_with("com.wisent.") && key.ends_with(".onboarding.done")),
        "the retired key was not renamed: {carried:?}"
    );
    assert!(!carried.keys().any(|key| key.starts_with("ai.wisent.")), "a retired key stayed: {carried:?}");
    assert_eq!(carried.get("workspace").and_then(|v| v.as_string()), Some("/w"));
    assert_eq!(
        std::fs::read_to_string(support.join(&bundle_id).join("state.json")).unwrap(),
        "{\"kept\":true}"
    );
    assert!(!caches.join(&bundle_id).join("old").exists(), "a place the new name had was overwritten");
    assert!(preferences.join(format!("{retired}.plist")).exists(), "the old place was not left for a rollback");

    // Once carried, a second install changes nothing.
    std::fs::write(support.join(&bundle_id).join("state.json"), "{\"changed\":true}").unwrap();
    let second = restart();
    assert!(second.status.success(), "the second restart step failed: {}", said(&second));
    assert_eq!(
        std::fs::read_to_string(support.join(&bundle_id).join("state.json")).unwrap(),
        "{\"changed\":true}",
        "a second install overwrote what the renamed app wrote"
    );

    drop(owned);
}
