//! `stado capabilities resolve`, `call`, `attestation` and `select`, run as
//! the real binary in an isolated home: a family whose first alternative does
//! not answer is served by the next one and says so; a model request handed
//! to `call` reaches whichever alternative answers, so the caller changes
//! nothing when the alternatives or their order change; a request no
//! statement answers yet is kept and refused naming the commands that answer
//! it; the recorded statement answers that exact request, labelled as an
//! attestation; withdrawing it refuses the request again; `select` writes
//! only an order of the family's own alternatives.
//!
//! Brama is placed nowhere in the isolated registry, so the directory refuses
//! it, as it does on a machine where Brama is down.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const CONSUMER: &str = "oko";

struct Home {
    root: PathBuf,
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
        Self { root }
    }

    fn run_with(&self, args: &[&str], input: Option<&str>) -> Output {
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
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, None)
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

/// A request that makes the answer call one function, as Probierz's
/// evaluators send.
fn judged_request() -> Value {
    json!({
        "model": "any-vision-capable",
        "messages": [{"role": "user", "content": "Judge this talk."}],
        "tools": [{"type": "function", "function": {
            "name": "record_talk_evaluation",
            "parameters": {"type": "object"}
        }}],
        "tool_choice": {"type": "function", "function": {"name": "record_talk_evaluation"}}
    })
}

fn call(home: &Home, request: &Value) -> Output {
    home.run_with(
        &[
            "capabilities",
            "call",
            "inference",
            "--consumer",
            CONSUMER,
            "--json",
        ],
        Some(&request.to_string()),
    )
}

#[test]
fn a_down_first_alternative_is_named_and_the_next_one_serves() {
    let home = Home::new();
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
        json!(["brama", "attestation"])
    );

    // Brama is placed nowhere, so the attestation alternative serves, as a
    // fallback, and Brama is named with its cause.
    let served = home.json(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert_eq!(served["provider"], "attestation");
    assert_eq!(served["kind"], "attestation");
    assert_eq!(served["fallback"], true);
    assert_eq!(served["unreachable"][0]["product"], "brama");
    assert!(!served["unreachable"][0]["cause"]
        .as_str()
        .unwrap()
        .trim()
        .is_empty());

    // The answer is kept: `stado capabilities` says who served the consumer.
    let listed = home.json(&["capabilities", "inference", "--json"]);
    let kept = &listed["alternatives"][0];
    assert_eq!(kept["served"][CONSUMER]["provider"], "attestation");
    assert_eq!(kept["served"][CONSUMER]["fallback"], true);
    assert_eq!(kept["proven"]["brama for oko"]["answered"], false);
    assert_eq!(kept["proven"]["attestation for oko"]["answered"], true);

    // A request no statement answers is kept and refused, naming every
    // alternative it was asked of and the commands that answer it.
    let request = judged_request();
    let refused = call(&home, &request);
    assert!(
        !refused.status.success(),
        "a request nothing answers was answered"
    );
    let details = &envelope(&refused)["details"];
    let products: Vec<&str> = details["unreachable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["product"].as_str().unwrap())
        .collect();
    assert_eq!(products, ["brama", "attestation"]);
    let cause = details["unreachable"][1]["cause"].as_str().unwrap();
    assert!(
        cause.contains("stado capabilities attestation record inference --consumer oko"),
        "{cause}"
    );

    // The kept request is read back as it was asked.
    let waiting = home.json(&[
        "capabilities",
        "attestation",
        "list",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert_eq!(waiting.as_array().unwrap().len(), 1);
    assert_eq!(waiting[0]["attested"], false);
    let digest = waiting[0]["request"].as_str().unwrap().to_owned();
    let shown = home.json(&[
        "capabilities",
        "attestation",
        "show",
        "inference",
        "--consumer",
        CONSUMER,
        &digest[..12],
    ]);
    assert_eq!(shown["asked"], request);
    assert_eq!(shown["answer"], Value::Null);

    // An answer that is not the forced function's arguments is refused, and
    // so is a statement that does not say what was checked.
    let not_arguments = home.run_with(
        &[
            "capabilities",
            "attestation",
            "record",
            "inference",
            "--consumer",
            CONSUMER,
            &digest,
            "--checked",
            "read every slide against the application",
        ],
        Some("it passes"),
    );
    assert!(!not_arguments.status.success());
    assert!(String::from_utf8_lossy(&not_arguments.stderr).contains("record_talk_evaluation"));
    let unchecked = home.run_with(
        &[
            "capabilities",
            "attestation",
            "record",
            "inference",
            "--consumer",
            CONSUMER,
            &digest,
            "--checked",
            " ",
        ],
        Some(r#"{"summary":"met"}"#),
    );
    assert!(!unchecked.status.success());

    let recorded = home.run_with(
        &[
            "capabilities",
            "attestation",
            "record",
            "inference",
            "--consumer",
            CONSUMER,
            &digest,
            "--checked",
            "read every slide against the application",
        ],
        Some(r#"{"summary":"every criterion met"}"#),
    );
    assert!(
        recorded.status.success(),
        "{}",
        String::from_utf8_lossy(&recorded.stderr)
    );

    // The same request, its keys in another order, is answered by the
    // statement, labelled as an attestation and not as a model.
    let reordered: Value = serde_json::from_str(
        &serde_json::to_string(&json!({
            "tool_choice": request["tool_choice"],
            "tools": request["tools"],
            "messages": request["messages"],
            "model": request["model"],
        }))
        .unwrap(),
    )
    .unwrap();
    let answered = call(&home, &reordered);
    assert!(
        answered.status.success(),
        "{}",
        String::from_utf8_lossy(&answered.stderr)
    );
    let answered: Value = serde_json::from_slice(&answered.stdout).unwrap();
    assert_eq!(answered["resolution"]["provider"], "attestation");
    let answer = &answered["answer"];
    assert_eq!(answer["model"], "attestation");
    // A caller that accounts for every answer reads that a statement spent
    // no model tokens.
    assert_eq!(
        answer["usage"],
        json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0})
    );
    assert_eq!(
        answer["attestation"]["checked"],
        "read every slide against the application"
    );
    let called = &answer["choices"][0]["message"]["tool_calls"][0]["function"];
    assert_eq!(called["name"], "record_talk_evaluation");
    assert_eq!(
        serde_json::from_str::<Value>(called["arguments"].as_str().unwrap()).unwrap(),
        json!({"summary": "every criterion met"})
    );

    // A second statement is refused while one stands; withdrawn, the request
    // is refused again and still kept.
    let twice = home.run_with(
        &[
            "capabilities",
            "attestation",
            "record",
            "inference",
            "--consumer",
            CONSUMER,
            &digest,
            "--checked",
            "again",
        ],
        Some(r#"{"summary":"x"}"#),
    );
    assert!(!twice.status.success());
    let revoked = home.run(&[
        "capabilities",
        "attestation",
        "revoke",
        "inference",
        "--consumer",
        CONSUMER,
        &digest,
    ]);
    assert!(
        revoked.status.success(),
        "{}",
        String::from_utf8_lossy(&revoked.stderr)
    );
    assert!(!call(&home, &request).status.success());

    // What is not a request is refused before any alternative is asked.
    let malformed = home.run_with(
        &["capabilities", "call", "inference", "--consumer", CONSUMER],
        Some("not json"),
    );
    assert!(!malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stderr).contains("chat-completions request"));
    let secrets = home.run_with(
        &["capabilities", "call", "secrets", "--consumer", CONSUMER],
        Some(&request.to_string()),
    );
    assert!(!secrets.status.success());
    assert!(String::from_utf8_lossy(&secrets.stderr).contains("resolved, not called"));

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

    // The selected order is written, and only an order of its own
    // alternatives; with attestation first, it serves first.
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
        "attestation,brama",
    ]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let listed = home.json(&["capabilities", "inference", "--json"]);
    assert_eq!(
        listed["alternatives"][0]["order"],
        json!(["attestation", "brama"])
    );
    let first = home.json(&[
        "capabilities",
        "resolve",
        "inference",
        "--consumer",
        CONSUMER,
        "--json",
    ]);
    assert_eq!(first["provider"], "attestation");
    assert_eq!(first["fallback"], false);
}
