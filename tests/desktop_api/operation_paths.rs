//! Every CLI path a Stado Desktop capability operation runs resolves to a
//! command of the built `stado`, and every option it passes is one that
//! command takes.
//!
//! The built Desktop app (`STADO_TEST_DESKTOP_BIN`, its executable) prints
//! every operation's group, id, path, fixed arguments and field options with
//! `--operation-paths`; each path is then asked for `--help` through the
//! built `stado` in one isolated deployment. Expected: every path answers its
//! help and that help names every option the operation passes. A path naming
//! a verb the CLI does not have is listed with clap's refusal, and an option
//! the command does not take is listed by name, so the operation that
//! drifted is named.
use crate::fixture::Service;
use serde_json::Value;
use std::path::Path;

#[tokio::test]
async fn every_desktop_operation_path_is_a_cli_command() {
    let mut service = Service::start();
    let desktop = service.input("STADO_TEST_DESKTOP_BIN");
    let listed = service.execute(Path::new(&desktop), &["--operation-paths"]);
    assert!(
        listed.status.success(),
        "the Desktop app lists its operation paths: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let operations: Vec<Value> = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(!operations.is_empty(), "the Desktop app lists no operation");
    let stado = Path::new(env!("CARGO_BIN_EXE_stado"));
    let mut drifted = Vec::new();
    for operation in &operations {
        let path: Vec<&str> = operation["path"]
            .as_array()
            .expect("every operation names its path")
            .iter()
            .map(|word| word.as_str().expect("a path word is text"))
            .collect();
        let mut args = path.clone();
        args.push("--help");
        let help = service.execute(stado, &args);
        if !help.status.success() {
            drifted.push(format!(
                "{} {} runs `stado {}`: {}",
                operation["group"],
                operation["id"],
                path.join(" "),
                String::from_utf8_lossy(&help.stderr).trim()
            ));
            continue;
        }
        // Every option a field passes, and every fixed argument that is an
        // option, is one the command's help names.
        let usage = String::from_utf8_lossy(&help.stdout);
        let passed = operation["options"]
            .as_array()
            .expect("every operation lists the options its fields pass")
            .iter()
            .chain(
                operation["fixed_arguments"]
                    .as_array()
                    .into_iter()
                    .flatten(),
            )
            .filter_map(Value::as_str)
            .filter(|option| option.starts_with("--"));
        for option in passed {
            let named = usage
                .split(|character: char| {
                    character.is_whitespace()
                        || character == ','
                        || character == '='
                        || character == '['
                        || character == ']'
                })
                .any(|word| word == option);
            if !named {
                drifted.push(format!(
                    "{} {} passes {option} to `stado {}`, whose help names no such option",
                    operation["group"],
                    operation["id"],
                    path.join(" ")
                ));
            }
        }
    }
    assert!(
        drifted.is_empty(),
        "Desktop operations the CLI does not have:\n{}",
        drifted.join("\n")
    );
    service.pass();
}
