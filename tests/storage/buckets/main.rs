//! `stado storage bucket` against a real self-hosted Stado.
//!
//! The run names a fleet host whose own Stado serves an object API over its
//! disk (STADO_TEST_BUCKET_HOST) and the service unit that serves it
//! (STADO_TEST_BUCKET_UNIT), and uses the operator's configured Stado: its
//! registry reaches the host and its vault holds the bucket's key. The
//! journey owns one bucket of its own, named for this run, and removes it
//! at the end, so no existing bucket, declaration or vault item is touched.
//!
//! create declares the bucket on the host with a grant and mints its key,
//! and list and show read it back from the host's own configuration; a
//! second create is refused naming the host; update replaces the grant and
//! show reads the new one back; rotate replaces the key in the same item;
//! a delete whose --confirm does not repeat the name removes nothing; delete
//! removes declaration and key, after which list lacks the bucket and show,
//! update and rotate are refused naming what is missing. Every command, its
//! exit status and output are kept in report.json beside the run.
//!
//! `cargo test --test storage-buckets -- --nocapture`
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Run {
    root: PathBuf,
    report: Value,
    host: String,
    unit: String,
}

/// A value the run declares in the environment variable `name`.
fn declared(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!("{name} is empty; it must name what this journey runs against"),
        Err(error) => panic!("{name} must name what this journey runs against: {error}"),
    }
}

impl Run {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/storage-buckets")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        let mut git = Command::new("git");
        git.current_dir(&repository).args(["rev-parse", "HEAD"]);
        let revision = git.output().unwrap();
        Self {
            root,
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "commands": [],
                "outcome": "failed",
            }),
            host: declared("STADO_TEST_BUCKET_HOST"),
            unit: declared("STADO_TEST_BUCKET_UNIT"),
        }
    }

    fn bucket(&mut self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["storage", "bucket"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn save(&self) {
        let report = serde_json::to_vec_pretty(&self.report).unwrap();
        fs::write(self.root.join("report.json"), report).unwrap();
    }

    fn answered(&mut self, args: &[&str]) -> Value {
        let output = self.bucket(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Run `args`, which must be refused saying one of `causes`.
    fn refused(&mut self, args: &[&str], causes: &[&str]) {
        let output = self.bucket(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{args:?} was accepted: {stdout}");
        let said = causes.iter().any(|cause| stderr.contains(cause));
        assert!(
            said,
            "{args:?}: the refusal says none of {causes:?}: {stderr}"
        );
    }
}

fn names(listed: &Value) -> Vec<String> {
    let rows = listed["buckets"].as_array().unwrap();
    rows.iter()
        .map(|row| row["bucket"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_bucket_is_created_read_changed_rekeyed_and_removed_on_its_host() {
    let mut run = Run::start();
    let (host, unit) = (run.host.clone(), run.unit.clone());
    let name = format!("bucket-journey-{}", uuid::Uuid::new_v4().simple());
    let on = ["--host", host.as_str(), "--json"];
    let reload = ["--reload-service", unit.as_str()];

    let mut create = vec!["create", name.as_str(), "--grant", "inbox/=put,stat"];
    create.extend(reload);
    create.extend(on);
    let created = run.answered(&create);
    assert_eq!(created["bucket"], json!(name));
    assert_eq!(created["reloaded"], json!(unit), "{created}");
    let item = created["key"]["item"].as_str().unwrap().to_string();
    let mut list = vec!["list"];
    list.extend(on);
    let listed = run.answered(&list);
    assert!(
        names(&listed).contains(&name),
        "list does not read the bucket back from {host}"
    );
    let mut show = vec!["show", name.as_str()];
    show.extend(on);
    let shown = run.answered(&show);
    let same = shown["grant"] == created["grant"];
    assert!(same, "show reads another grant than create wrote: {shown}");
    run.refused(
        &create,
        &[format!("{host} already serves bucket {name}").as_str()],
    );

    let mut update = vec!["update", name.as_str()];
    update.extend(["--grant", "inbox/=get,list", "--grant", "outbox/=put"]);
    update.extend(reload);
    update.extend(on);
    let updated = run.answered(&update);
    let reread = run.answered(&show);
    let same = reread["grant"] == updated["grant"];
    assert!(same, "show does not read the new grant back: {reread}");
    assert!(
        reread["grant"] != shown["grant"],
        "update left the grant as it was: {reread}"
    );

    let rotated = run.answered(&["rotate", name.as_str(), "--json"]);
    assert_eq!(rotated["rotated"], json!(true));
    let kept = rotated["key"]["item"] == json!(item);
    assert!(kept, "rotate wrote the key into another item: {rotated}");

    let mut wrong = vec!["delete", name.as_str(), "--confirm", "another-bucket"];
    wrong.extend(on);
    run.refused(&wrong, &["nothing was removed"]);
    assert!(
        names(&run.answered(&list)).contains(&name),
        "a refused delete removed the bucket"
    );
    let mut delete = vec!["delete", name.as_str(), "--confirm", name.as_str()];
    delete.extend(reload);
    delete.extend(on);
    let deleted = run.answered(&delete);
    assert_eq!(deleted["removed"]["item"], json!(item), "{deleted}");
    // A host whose only bucket this was declares none now, which list
    // refuses by name; one that serves others lists them without it.
    let after = run.bucket(&list);
    if after.status.success() {
        let listed: Value = serde_json::from_slice(&after.stdout).unwrap();
        assert!(
            !names(&listed).contains(&name),
            "list still reads the deleted bucket from {host}"
        );
    } else {
        let stderr = String::from_utf8_lossy(&after.stderr);
        assert!(
            stderr.contains(&format!("{host} declares no readable buckets")),
            "{stderr}"
        );
    }
    let missing = format!("no bucket {name}");
    let empty = format!("{host} declares no readable buckets");
    run.refused(&show, &[missing.as_str(), empty.as_str()]);
    run.refused(&update, &[missing.as_str(), empty.as_str()]);
    run.refused(
        &["rotate", name.as_str(), "--json"],
        &["the vault holds no key"],
    );

    run.report["outcome"] = json!("passed");
    run.save();
    eprintln!("bucket journey evidence: {}", run.root.display());
}
