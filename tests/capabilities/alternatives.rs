//! `stado capabilities resolve` and `select`, run as the real binary in an
//! isolated home: a family whose first alternative does not answer is served
//! by the next one and says so; a family none of whose alternatives answers
//! is refused naming each one's product, route and cause; the answer is kept
//! and read back in `stado capabilities`; a reported refusal (`--failed`)
//! proves that alternative again; `select` writes only an order of the
//! family's own alternatives.
//!
//! The model endpoint is a real HTTP listener this test serves on loopback,
//! answering every request 401 as a gateway that refuses an unauthenticated
//! caller does: any HTTP answer is an answer. Brama is placed nowhere in the
//! isolated registry, so the directory refuses it.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const CONSUMER: &str = "oko";

struct Home {
    root: PathBuf,
    endpoint: Option<String>,
}

impl Home {
    fn new() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/capabilities-alternatives")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            endpoint: None,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(endpoint) = &self.endpoint {
            command
                .env("STADO_INFERENCE_ENDPOINT_URL", endpoint)
                .env("STADO_INFERENCE_ENDPOINT_MODEL", "example-model");
        }
        let output = command.output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

/// A loopback HTTP listener answering every request it accepts with 401.
fn refusing_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0_u8; 4096];
            let _read = stream.read(&mut request);
            let _written = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
        }
    });
    format!("http://{address}")
}

fn envelope(output: &Output) -> Value {
    let text = String::from_utf8_lossy(&output.stdout);
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        text
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .chain(serde_json::from_str::<Value>(&text).ok())
        .find(|value| value.get("details").is_some())
        .unwrap_or_else(|| panic!("no JSON refusal envelope in {text}"))
}

#[test]
fn a_down_first_alternative_is_named_and_the_next_one_serves() {
    let mut home = Home::new();
    let initialised = home.run(&["config", "init"]);
    assert!(
        initialised.status.success(),
        "{}",
        String::from_utf8_lossy(&initialised.stderr)
    );

    let listed = home.json(&["capabilities", "inference", "--json"]);
    assert_eq!(listed["alternatives"][0]["family"], "inference");
    assert_eq!(
        listed["alternatives"][0]["order"],
        serde_json::json!(["brama", "model-endpoint"])
    );

    // Nothing answers: Brama is placed nowhere and no endpoint is declared.
    let refused = home.run(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert!(
        !refused.status.success(),
        "a family nothing answers was resolved"
    );
    let details = &envelope(&refused)["details"];
    assert_eq!(details["family"], "inference");
    let unreachable = details["unreachable"].as_array().unwrap();
    let products: Vec<&str> = unreachable
        .iter()
        .map(|item| item["product"].as_str().unwrap())
        .collect();
    assert_eq!(products, ["brama", "model-endpoint"]);
    assert!(unreachable
        .iter()
        .all(|item| !item["cause"].as_str().unwrap().trim().is_empty()));
    assert!(unreachable[1]["cause"]
        .as_str()
        .unwrap()
        .contains("capabilities.inference.endpoint.url"));

    // The declared endpoint answers, so it serves, as a fallback.
    home.endpoint = Some(refusing_endpoint());
    let served = home.json(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert_eq!(served["provider"], "model-endpoint");
    assert_eq!(served["fallback"], true);
    assert_eq!(served["route"], home.endpoint.clone().unwrap());
    assert_eq!(served["model"], "example-model");
    assert_eq!(served["unreachable"][0]["product"], "brama");

    // The answer is kept: `stado capabilities` says who served the consumer.
    let listed = home.json(&["capabilities", "inference", "--json"]);
    let kept = &listed["alternatives"][0];
    assert_eq!(kept["served"][CONSUMER]["provider"], "model-endpoint");
    assert_eq!(kept["served"][CONSUMER]["fallback"], true);
    // A directory route's proof speaks for the consumer it was proven for.
    assert_eq!(kept["proven"]["brama for oko"]["answered"], false);
    assert_eq!(kept["proven"]["model-endpoint"]["answered"], true);

    // A caller refused by Brama says so, and Brama is proven again now.
    let reported = home.json(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--failed",
        "brama",
        "--json",
    ]);
    assert_eq!(reported["provider"], "model-endpoint");
    let listed = home.json(&["capabilities", "inference", "--json"]);
    assert_eq!(
        listed["alternatives"][0]["proven"]["brama for oko"]["reported_by"],
        CONSUMER
    );

    // An alternative the family does not have is refused before anything runs.
    let unknown = home.run(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--failed",
        "nobody",
    ]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("has no alternative nobody"));

    // The selected order is written, and only an order of its own alternatives.
    let refused = home.run(&[
        "capabilities",
        "select",
        "inference",
        "--order",
        "brama,brama",
    ]);
    assert!(
        !refused.status.success(),
        "an order naming brama twice was written"
    );
    let selected = home.run(&[
        "capabilities",
        "select",
        "inference",
        "--order",
        "model-endpoint,brama",
    ]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let listed = home.json(&["capabilities", "inference", "--json"]);
    assert_eq!(
        listed["alternatives"][0]["order"],
        serde_json::json!(["model-endpoint", "brama"])
    );
    let first = home.json(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert_eq!(first["provider"], "model-endpoint");
    assert_eq!(first["fallback"], false);
}
