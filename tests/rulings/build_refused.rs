//! A product operation that compiles runs only while the operator's rulings
//! allow building, as Oko's one admission decision (`oko admit build`) answers;
//! Stado keeps no copy of the rule.
//!
//! Runs the built `stado product cargo metadata` on this checkout's own
//! manifest, with a ruling register of the run's own (OKO_RULING_REGISTER, in
//! the shape `oko rulings record` writes it) that the installed `oko` reads.
//! First the register holds the operator's withdrawal of every build: Stado
//! refuses before Cargo runs, quoting the ruling and its words, and saying
//! that the run named no purpose; the same run naming an open defect outside
//! the build-first category with `--purpose` is refused naming that defect,
//! so the purpose reached Oko; a purpose that names no open defect is no
//! decision at all, and Stado says Oko gave none, quoting why. Then a later
//! ruling consents again: the same operation runs to its end, with and
//! without a purpose. Nothing of the operator's is read or written but his
//! defect register, read for an open defect to name and whether it is open;
//! the ruling register is removed at the end.
//!
//! An `oko` that gives no verdict decided nothing: with a ruling register
//! that is not JSON the installed `oko admit` answers no decision, and the
//! same operation is refused saying the admission decision was never read,
//! not that a ruling refuses it.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

fn ruling(id: &str, subject: &str, said: &str, at: &str) -> Value {
    json!({
        "id": id, "subjects": [subject], "session": "stado-build-refused", "ordinal": said.len(),
        "said": said, "saidAt": at, "recordedIn": "stado-build-refused", "recordedAt": at,
    })
}

/// The built `stado product cargo metadata` on this checkout's manifest, the
/// run's ruling register in place of the operator's, naming `purposes` with
/// `--purpose` (given after the verb, as the operator types it).
fn stado(register: &Path, purposes: &[&str]) -> Output {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(["product", "cargo", "--manifest-path"])
        .arg(&manifest);
    for &purpose in purposes {
        command.args(["--purpose", purpose]);
    }
    command
        .arg("metadata")
        .env("OKO_RULING_REGISTER", register)
        .output()
        .expect("the built stado starts")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An open defect of the operator's register outside the build-first
/// category, read with the installed `oko`: a purpose no scoped consent of
/// his covers.
fn open_defect() -> String {
    let listed = Command::new("oko")
        .args(["defects", "list", "--open", "--json"])
        .output()
        .expect("the installed oko starts");
    assert!(
        listed.status.success(),
        "oko defects list failed: {}",
        text(&listed.stderr)
    );
    let defects: Value = serde_json::from_slice(&listed.stdout).expect("oko lists defects as JSON");
    defects
        .as_array()
        .expect("the listing is an array")
        .iter()
        .find(|defect| defect["category"].is_null())
        .and_then(|defect| defect["id"].as_str())
        .expect("an open defect outside build-first")
        .to_string()
}

#[test]
fn a_withdrawn_build_is_refused_until_a_later_ruling_lifts_it() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/journeys")
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
    let refused = stado(&register, &[]);
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
    assert!(
        said.contains("no open defect was named as the purpose of this run"),
        "a run that names no purpose is said to name none: {said}"
    );

    // A named purpose reaches Oko's decision: the refusal is now about the
    // defect the run names, which no consent of his covers.
    let purpose = open_defect();
    let named = stado(&register, &[purpose.as_str()]);
    let said = text(&named.stderr);
    assert!(
        !named.status.success(),
        "a withdrawn build ran for a purpose: {}",
        text(&named.stdout)
    );
    assert!(
        said.contains(&format!(
            "none of his consents covers the purpose {purpose}"
        )),
        "the refusal names the purpose the run gave: {said}"
    );

    // A purpose that names no open defect is no decision: Oko refuses the
    // question, and Stado says so with Oko's words, not as a ruling.
    let unknown = "not-a-defect-of-this-run";
    let asked = stado(&register, &[unknown]);
    let said = text(&asked.stderr);
    assert!(
        !asked.status.success(),
        "a build ran for an unknown purpose: {}",
        text(&asked.stdout)
    );
    assert!(
        said.contains("Oko gave no admission decision")
            && said.contains(&format!("--purpose {unknown} names no open defect")),
        "the refusal says no decision was given and why: {said}"
    );

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
    let allowed = stado(&register, &[]);
    assert!(
        allowed.status.success(),
        "a build the later ruling allows was refused: {}",
        text(&allowed.stderr)
    );
    let allowed = stado(&register, &[purpose.as_str()]);
    assert!(
        allowed.status.success(),
        "naming a purpose refused a build the operator's consent allows: {}",
        text(&allowed.stderr)
    );

    std::fs::remove_dir_all(&root).expect("the run directory is removed");
}

#[test]
fn an_oko_that_gives_no_verdict_is_not_read_as_a_ruling() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/journeys")
        .join(format!("rulings-no-verdict-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("the run directory is created");
    let broken = root.join("rulings.json");
    std::fs::write(&broken, "not a ruling register").expect("the broken register is written");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let refused = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "cargo", "--manifest-path"])
        .arg(&manifest)
        .arg("metadata")
        .env("OKO_RULING_REGISTER", &broken)
        .output()
        .expect("the built stado starts");
    let said = text(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a build ran with no admission decision: {}",
        text(&refused.stdout)
    );
    assert!(
        said.contains("Oko gave no admission decision") && said.contains("oko admit build"),
        "the refusal names the decision that was never read and why: {said}"
    );
    assert!(
        !said.contains("the operator's rulings do not allow it now"),
        "no ruling was read, so none is quoted: {said}"
    );
    assert!(said.contains("nothing was exported or built"), "{said}");
    std::fs::remove_dir_all(&root).expect("the run directory is removed");
}
