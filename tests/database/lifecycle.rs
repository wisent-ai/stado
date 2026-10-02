//! The database plane through the real `stado` binary, against an isolated
//! configuration file under Cargo's target directory: a database of any
//! engine is declared, listed with its engine, removed and confirmed gone,
//! and `create --provider external` refuses a URL that disagrees with
//! `--engine` or names a sqlite file, before anything reaches a vault.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

struct Isolated {
    directory: PathBuf,
    config: PathBuf,
}

impl Isolated {
    fn new(case: &str) -> Self {
        let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("database")
            .join(case);
        if directory.exists() {
            std::fs::remove_dir_all(&directory).expect("clear the previous run of this case");
        }
        std::fs::create_dir_all(&directory).expect("create the isolated directory");
        let config = directory.join("stado.config.json");
        std::fs::write(&config, "{}\n").expect("write the empty configuration");
        Self { directory, config }
    }

    fn run(&self, arguments: &[&str], input: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(arguments)
            .env("STADO_CONFIG", &self.config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the stado binary");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        if let Some(input) = input {
            stdin
                .write_all(input.as_bytes())
                .expect("write standard input");
        }
        drop(stdin);
        child.wait_with_output().expect("collect the stado output")
    }

    fn succeed(&self, arguments: &[&str]) -> Value {
        let output = self.run(arguments, None);
        assert!(
            output.status.success(),
            "stado {arguments:?} failed with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("stado printed JSON")
    }

    fn refuse(&self, arguments: &[&str], input: Option<&str>, reason: &str) {
        let before = std::fs::read_to_string(&self.config).expect("read the configuration");
        let output = self.run(arguments, input);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stado {arguments:?} was accepted");
        assert!(
            stderr.contains(reason),
            "stado {arguments:?} refused without {reason:?}: {stderr}"
        );
        let after = std::fs::read_to_string(&self.config).expect("read the configuration");
        assert_eq!(before, after, "a refused command changed the configuration");
    }

    /// The engine `database list` reports for `name`, or None when absent.
    fn listed_engine(&self, name: &str) -> Option<String> {
        let listed = self.succeed(&["database", "list", "--json"]);
        listed
            .as_array()
            .expect("database list prints an array")
            .iter()
            .find(|entry| entry["database"] == name)
            .map(|entry| {
                entry["engine"]
                    .as_str()
                    .expect("database engine")
                    .to_string()
            })
    }

    /// Declare `name` as `engine`, see it listed with that engine, remove it
    /// and see it gone.
    fn lifecycle(&self, name: &str, engine: &str) {
        let declared = self.succeed(&[
            "database",
            "declare",
            name,
            "--engine",
            engine,
            "--consumer",
            "probe",
            "--json",
        ]);
        assert_eq!(declared["engine"], engine);
        assert_eq!(self.listed_engine(name).as_deref(), Some(engine));
        self.succeed(&["database", "remove", name, "--json"]);
        assert_eq!(self.listed_engine(name), None);
    }

    fn create_external(&self, engine: Option<&str>, url: Option<&str>, reason: &str) {
        let authority = self.directory.join("authority.pem");
        let authority = authority.to_str().expect("UTF-8 path");
        let mut arguments = vec![
            "database",
            "create",
            "ledger",
            "--consumer",
            "probe",
            "--provider",
            "external",
            "--ca-certificate",
            authority,
        ];
        if let Some(engine) = engine {
            arguments.push("--engine");
            arguments.push(engine);
        }
        self.refuse(&arguments, url, reason);
    }
}

impl Drop for Isolated {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn a_mysql_database_is_declared_listed_and_removed() {
    Isolated::new("mysql").lifecycle("ledger", "mysql");
}

#[test]
fn a_mongodb_database_is_declared_listed_and_removed() {
    Isolated::new("mongodb").lifecycle("events", "mongodb");
}

#[test]
fn a_declaration_whose_engine_is_not_an_engine_name_is_refused() {
    let isolated = Isolated::new("engine-name");
    isolated.refuse(
        &[
            "database",
            "declare",
            "ledger",
            "--engine",
            "My SQL",
            "--consumer",
            "probe",
        ],
        None,
        "My SQL",
    );
}

#[test]
fn an_external_url_that_disagrees_with_the_engine_is_refused() {
    Isolated::new("external-disagree").create_external(
        Some("postgres"),
        Some("mysql://user:secret@db.example.com:3306/ledger"),
        "but --engine is postgres",
    );
}

#[test]
fn an_external_sqlite_url_is_refused() {
    Isolated::new("external-sqlite").create_external(
        None,
        Some("sqlite://files.example.com/ledger.db"),
        "a sqlite file is created with --provider fleet",
    );
}

#[test]
fn an_external_create_without_a_url_is_refused() {
    Isolated::new("external-empty").create_external(None, None, "standard input was empty");
}
