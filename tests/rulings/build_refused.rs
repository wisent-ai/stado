//! A product operation that compiles runs only while the operator's rulings
//! allow building, as Oko's one admission decision (`oko admit build`) answers;
//! Stado keeps no copy of the rule.
//!
//! Runs the built `stado product cargo metadata` on this checkout's own
//! manifest, with a ruling register of the run's own (OKO_RULING_REGISTER, in
//! the shape `oko rulings record` writes it) that the installed `oko` reads.
//! First the register holds the operator's withdrawal of every build: Stado
//! refuses before Cargo runs, quoting the ruling and its words. Then a later
//! ruling consents again: the same operation runs to its end. Nothing of the
//! operator's is read or written; the register is removed at the end.
//!
//! An `oko` that gives no verdict decided nothing: with no `oko` on the
//! operation's PATH the same operation is refused saying the admission
//! decision was never read, not that a ruling refuses it.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn ruling(id: &str, subject: &str, said: &str, at: &str) -> Value {
    json!({
        "id": id, "subjects": [subject], "session": "stado-build-refused", "ordinal": said.len(),
        "said": said, "saidAt": at, "recordedIn": "stado-build-refused", "recordedAt": at,
    })
}

fn stado(register: &Path) -> Output {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "cargo", "--manifest-path"])
        .arg(&manifest)
        .arg("metadata")
        .env("OKO_RULING_REGISTER", register)
        .output()
        .expect("the built stado starts")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_withdrawn_build_is_refused_until_a_later_ruling_lifts_it() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rulings-build-refused-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("the run directory is created");
    let register = root.join("rulings.json");
    let withdrawal = ruling(
        "ruling-withdrawal",
        "no-build",
        "zakaz budowania",
        "2026-10-02T00:00:00Z",
    );

    std::fs::write(
        &register,
        json!({ "rulings": [withdrawal.clone()] }).to_string(),
    )
    .expect("the register is written");
    let refused = stado(&register);
    let said = text(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a withdrawn build ran: {}",
        text(&refused.stdout)
    );
    assert!(
        said.contains("the operator's rulings do not allow it now"),
        "{said}"
    );
    assert!(
        said.contains("ruling-withdrawal") && said.contains("zakaz budowania"),
        "the refusal quotes him: {said}"
    );
    assert!(said.contains("nothing was exported or built"), "{said}");

    let lifted = ruling(
        "ruling-lifted",
        "build",
        "mozesz budowac",
        "2026-10-03T00:00:00Z",
    );
    std::fs::write(
        &register,
        json!({ "rulings": [withdrawal, lifted] }).to_string(),
    )
    .expect("the register is written");
    let allowed = stado(&register);
    assert!(
        allowed.status.success(),
        "a build the later ruling allows was refused: {}",
        text(&allowed.stderr)
    );

    std::fs::remove_dir_all(&root).expect("the run directory is removed");
}

#[test]
fn an_oko_that_gives_no_verdict_is_not_read_as_a_ruling() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rulings-no-verdict-{}", std::process::id()));
    let empty = root.join("path-without-oko");
    std::fs::create_dir_all(&empty).expect("the run directory is created");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let refused = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "cargo", "--manifest-path"])
        .arg(&manifest)
        .arg("metadata")
        .env("PATH", &empty)
        .output()
        .expect("the built stado starts");
    let said = text(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a build ran with no admission decision: {}",
        text(&refused.stdout)
    );
    assert!(
        said.contains("Oko gave no admission decision")
            && said.contains("oko admit build")
            && said.contains("did not run"),
        "the refusal names the decision that was never read and why: {said}"
    );
    assert!(
        !said.contains("the operator's rulings do not allow it now"),
        "no ruling was read, so none is quoted: {said}"
    );
    assert!(said.contains("nothing was exported or built"), "{said}");
    std::fs::remove_dir_all(&root).expect("the run directory is removed");
}
