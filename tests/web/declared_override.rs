//! `stado web list` reads the web product table from `WC_WEB_API_PRODUCTS`
//! when the config file declares none: a product the override declares is
//! listed, an override that is not JSON is refused naming the variable, and
//! nothing in either place lists nothing.
//!
//! The real stado runs with an isolated `HOME` under the checkout's build
//! directory and a `STADO_CONFIG` path that holds no file, so the override is
//! the only place the table can come from.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PRODUCT: &str = "marker";
const DECLARATION: &str =
    r#"{"marker": {"hostname": "marker.example.com", "redirect_to": "https://example.com"}}"#;
const NOTHING_DECLARED: &str = "no web products are declared";
const NOT_JSON: &str = "WC_WEB_API_PRODUCTS must be valid JSON";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.build/journeys/web-declared-override");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("tmp")).expect("create the run directory");
        Self { root }
    }

    /// `stado web list` with nothing of this account's configuration, and
    /// the override set to `products` when it is given.
    fn list(&self, products: Option<&str>, json: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(["web", "list"])
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set for the test"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if json {
            command.arg("--json");
        }
        if let Some(products) = products {
            command.env("WC_WEB_API_PRODUCTS", products);
        }
        command.output().expect("run the real stado")
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove the run directory");
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_product_the_override_declares_is_listed() {
    let run = Run::new("declared");
    let listed = run.list(Some(DECLARATION), true);
    assert!(listed.status.success(), "stado web list refused: {}", said(&listed));
    let rows: Vec<serde_json::Value> =
        serde_json::from_slice(&listed.stdout).expect("web list --json answers a JSON array");
    assert!(
        rows.iter().any(|row| row["product"] == PRODUCT && row["kind"] == "redirect"),
        "the override's product is listed: {}",
        said(&listed)
    );
}

#[test]
fn an_override_that_is_not_json_is_refused_not_read_as_nothing_declared() {
    let run = Run::new("not-json");
    let refused = run.list(Some("{not json"), false);
    assert!(!refused.status.success(), "a malformed override must be refused: {}", said(&refused));
    let answer = said(&refused);
    assert!(answer.contains(NOT_JSON), "the refusal names the variable: {answer}");
    assert!(!answer.contains(NOTHING_DECLARED), "a refused override is not an empty plane: {answer}");
}

#[test]
fn nothing_declared_anywhere_lists_nothing() {
    let run = Run::new("undeclared");
    let listed = run.list(None, false);
    assert!(listed.status.success(), "stado web list refused: {}", said(&listed));
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains(NOTHING_DECLARED),
        "an undeclared table lists nothing: {}",
        said(&listed)
    );
}
