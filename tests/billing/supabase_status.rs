//! `stado billing refresh` publishes a `supabase` section that says whether
//! the hosted project behind each declared database runs, and names every
//! reason it cannot say.
//!
//! One isolated deployment through the real `stado` binary: a local store, a
//! config whose `credentials.store` selects the owner-only file credential
//! store, `billing.providers` set to `supabase` alone, and no Skarbiec. With
//! no database declared the section is `ok` and says no database names a
//! Supabase project. A database declared with `stado database declare` whose
//! item (`stado credentials put`) names a project, on a store holding no
//! item for the `supabase-management` role, makes the section
//! `no_credentials` naming that role. A database declaration that does not
//! parse makes it `config_error` naming the declaration. Each refresh is
//! read back from the snapshot the local store holds, and the commands with
//! their exit statuses and outputs are kept in the deployment's
//! `report.json` under the checkout's `.build`, the run's evidence.
//!
//! The section's answer for a project Supabase reports is read live from the
//! management API, which only the operator's own token reaches; that run is
//! `stado billing refresh` on a host configured with that token.
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const DATABASE: &str = "ledger";
const ITEM: &str = "ledger-database";
const PROJECT: &str = "ledgerprojectreference";

struct Deployment {
    root: PathBuf,
    report: Value,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/billing-supabase-status")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join(".stado")).unwrap();
        fs::create_dir_all(root.join("store")).unwrap();
        let store = root.join(".stado/credentials.json");
        fs::write(
            root.join(".stado/config.json"),
            serde_json::to_vec_pretty(&json!({
                "credentials": { "store": format!("file://{}", store.display()) },
                "billing": { "providers": ["supabase"] },
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION")
                    .expect("STADO_SOURCE_REVISION: the committed candidate this run judges"),
                "commands": [],
                "outcome": "failed",
            }),
        }
    }

    fn command(&self, args: &[&str], environment: &[(&str, &str)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado/config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"));
        for (name, value) in environment {
            command.env(name, value);
        }
        command
    }

    fn keep(&mut self, args: &[&str], output: &Output) {
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    fn run(&mut self, args: &[&str], environment: &[(&str, &str)]) -> Output {
        let output = self
            .command(args, environment)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        self.keep(args, &output);
        output
    }

    fn run_with_stdin(&mut self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args, &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        self.keep(args, &output);
        output
    }

    /// One refresh, its printed snapshot's `supabase` section, and the same
    /// section as the local store now holds it.
    fn refresh(&mut self, environment: &[(&str, &str)]) -> Value {
        let output = self.run(&["billing", "refresh", "--json"], environment);
        assert!(output.status.success(), "{}", said(&output));
        let printed: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("refresh printed no JSON ({error}): {}", said(&output)));
        let stored: Value = serde_json::from_slice(
            &fs::read(self.root.join("store/billing_health/credits.json"))
                .expect("the refresh stores its snapshot in the local store"),
        )
        .expect("the stored snapshot is JSON");
        assert_eq!(
            printed["supabase"], stored["supabase"],
            "the stored snapshot is the printed one"
        );
        printed["supabase"].clone()
    }
}

fn said(output: &Output) -> String {
    format!(
        "exit {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_billing_snapshot_says_whether_a_declared_databases_supabase_project_runs() {
    let mut deployment = Deployment::start();

    // No database declared: nothing lives in Supabase, and the section says so.
    let none = deployment.refresh(&[]);
    assert_eq!(none["status"], "ok", "{none}");
    assert_eq!(
        none["detail"], "no declared database names a Supabase project",
        "{none}"
    );
    assert_eq!(none["databases"], json!([]), "{none}");

    // A declared database whose item names a project, and no management token.
    let declared = deployment.run(
        &[
            "database",
            "declare",
            DATABASE,
            "--engine",
            "postgres",
            "--scope",
            "read",
            "--consumer",
            "ledger-client",
        ],
        &[],
    );
    assert!(declared.status.success(), "{}", said(&declared));
    let item = json!({ "project_ref": PROJECT }).to_string();
    let stored = deployment.run_with_stdin(&["credentials", "put", ITEM], &item);
    assert!(stored.status.success(), "{}", said(&stored));
    let tokenless = deployment.refresh(&[]);
    assert_eq!(tokenless["status"], "no_credentials", "{tokenless}");
    let detail = tokenless["detail"]
        .as_str()
        .expect("the section says why it read nothing");
    assert!(detail.contains("supabase-management"), "{tokenless}");

    // A declaration that does not parse is the operator's configuration.
    let malformed = deployment.refresh(&[(
        "WC_DATABASE_API_DATABASES",
        r#"{"Not Canonical": {"item": "x"}}"#,
    )]);
    assert_eq!(malformed["status"], "config_error", "{malformed}");
    assert!(
        malformed["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("database_api.databases")),
        "{malformed}"
    );

    // The human rendering names the section's state and cause.
    let shown = deployment.run(&["billing", "show"], &[]);
    assert!(shown.status.success(), "{}", said(&shown));
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("Supabase: config_error"),
        "{}",
        said(&shown)
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
