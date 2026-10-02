//! Real CLI, terminal stdin and native HTTP operations on an isolated local registry.
use super::configuration::confined_storage;
use super::fixture::Service;
use super::{confirmed, payload};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{self, IsTerminal};
use std::os::fd::FromRawFd;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::process::Stdio;

#[cfg_attr(target_os = "linux", link(name = "util"))]
unsafe extern "C" {
    fn openpty(
        master: *mut c_int,
        slave: *mut c_int,
        name: *mut c_char,
        settings: *const c_void,
        size: *const c_void,
    ) -> c_int;
}

fn terminal_input() -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    // Both output pointers are valid. Null optional arguments request the OS defaults.
    let result = unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(result, 0, "openpty: {}", io::Error::last_os_error());
    // A successful openpty transfers two distinct, live descriptors to this caller.
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

fn registry(service: &mut Service) -> Value {
    serde_json::from_str(&service.cli(&["registry", "pull", "--with-generation"])).unwrap()
}

fn unchanged(service: &mut Service, before: &Value) {
    let after = registry(service);
    service.observe("last_registry_read", after.clone());
    assert_eq!(
        &after, before,
        "a refused or read-only operation changed the registry"
    );
}

fn missing_terminal_source(service: &mut Service, args: &[&str], before: &Value) {
    let (_master, slave) = terminal_input();
    let terminal = slave.is_terminal();
    service.observe(&format!("{}_stdin", args[1]), json!({"terminal": terminal}));
    assert!(
        terminal,
        "the missing-source regression requires actual terminal stdin"
    );
    let output = service.execute_with_stdin(
        Path::new(env!("CARGO_BIN_EXE_stado")),
        args,
        Stdio::from(slave),
    );
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    unchanged(service, before);
}

#[tokio::test]
async fn explicit_registry_sources_preserve_refusals_and_saved_state() {
    let mut service = Service::start();
    let configuration: Value =
        serde_json::from_str(&service.cli(&["config", "show", "--json"])).unwrap();
    assert_eq!(configuration["file"], service.config.to_str().unwrap());
    assert_eq!(configuration["resolved"]["wc_storage_backend"], "local");
    assert_eq!(
        configuration["resolved"]["wc_backup_storage_backend"],
        "local"
    );
    confined_storage(
        &service.home,
        &configuration["resolved"]["wc_local_storage_path"],
    );
    confined_storage(
        &service.home,
        &configuration["resolved"]["wc_backup_local_storage_path"],
    );
    service.observe("isolated_storage", configuration);
    let before = registry(&mut service);
    assert_eq!(before["document"]["targets"].as_array().unwrap().len(), 1);
    service.observe("initial_registry", before.clone());

    missing_terminal_source(&mut service, &["registry", "validate"], &before);
    missing_terminal_source(
        &mut service,
        &[
            "registry",
            "push",
            "--force",
            "--allow-empty-fleet",
            "--json",
        ],
        &before,
    );
    let refusal = service
        .call(json!({"args": ["registry", "validate"]}), 200)
        .await;
    assert_eq!(refusal["exit_code"], 2, "{refusal}");
    unchanged(&mut service, &before);
    let refusal = service
        .call(
            confirmed(json!({
                "args": ["registry", "push", "--force", "--allow-empty-fleet", "--json"],
                "stdin": serde_json::to_string(&before["document"]).unwrap(),
            })),
            200,
        )
        .await;
    assert_eq!(refusal["exit_code"], 2, "{refusal}");
    unchanged(&mut service, &before);

    let mut document = before["document"].clone();
    document["targets"][0]["notes"] = json!("selected registry file");
    let source = service.root.join("selected-registry.json");
    let bytes = serde_json::to_vec_pretty(&document).unwrap();
    fs::write(&source, &bytes).unwrap();
    service.cli(&["registry", "validate", source.to_str().unwrap()]);
    unchanged(&mut service, &before);
    let receipt: Value = serde_json::from_str(&service.cli(&[
        "registry",
        "push",
        source.to_str().unwrap(),
        "--if-generation",
        before["generation"].as_str().unwrap(),
        "--json",
    ]))
    .unwrap();
    assert_eq!(receipt["state"], "pushed", "{receipt}");
    let saved = registry(&mut service);
    assert_eq!(saved["document"], document);
    assert_eq!(saved["generation"], receipt["generation"]);
    assert_eq!(
        fs::read(&source).unwrap(),
        bytes,
        "push rewrote its input file"
    );
    service.observe("file_publication", saved.clone());

    document["targets"][0]["notes"] = json!("explicit standard input");
    let body = serde_json::to_string(&document).unwrap();
    let validation = service
        .call(
            json!({
                "args": ["registry", "validate", "$INPUT"], "input": body,
            }),
            200,
        )
        .await;
    assert_eq!(validation["exit_code"], 0, "{validation}");
    unchanged(&mut service, &saved);
    let request = json!({
        "args": ["registry", "push", "-", "--if-generation", saved["generation"], "--json"],
        "stdin": body,
    });
    service.call(request.clone(), 403).await;
    unchanged(&mut service, &saved);
    let receipt = payload(service.call(confirmed(request), 200).await);
    assert_eq!(receipt["state"], "pushed", "{receipt}");
    let current = registry(&mut service);
    assert_eq!(current["document"], document);
    assert_eq!(current["generation"], receipt["generation"]);
    service.observe("stdin_publication", current.clone());

    let refusal = service.call(confirmed(json!({
        "args": ["registry", "push", "-", "--if-generation", before["generation"], "--json"],
        "stdin": serde_json::to_string(&before["document"]).unwrap(),
    })), 200).await;
    assert_eq!(refusal["exit_code"], 75, "{refusal}");
    assert_eq!(refusal["ok"], false, "{refusal}");
    let stale: Value = serde_json::from_str(refusal["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(stale["state"], "conflict", "{stale}");
    assert_eq!(stale["expected_generation"], before["generation"]);
    assert_eq!(stale["actual_generation"], current["generation"]);
    unchanged(&mut service, &current);

    fs::write(&source, "{").unwrap();
    let invalid = service.execute(
        Path::new(env!("CARGO_BIN_EXE_stado")),
        &["registry", "push", source.to_str().unwrap(), "--json"],
    );
    assert_eq!(invalid.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains(source.to_str().unwrap()));
    assert_eq!(fs::read(&source).unwrap(), b"{");
    unchanged(&mut service, &current);

    let empty_document = json!({"schema_version": 2, "coordinators": [], "targets": []});
    let empty_source = service.root.join("empty-registry.json");
    fs::write(&empty_source, serde_json::to_vec(&empty_document).unwrap()).unwrap();
    service.cli(&["registry", "validate", empty_source.to_str().unwrap()]);
    unchanged(&mut service, &current);
    let empty = service
        .call(
            confirmed(json!({
                "args": ["registry", "push", "-", "--force", "--json"],
                "stdin": serde_json::to_string(&empty_document).unwrap(),
            })),
            200,
        )
        .await;
    assert_eq!(empty["exit_code"], 1, "{empty}");
    unchanged(&mut service, &current);
    service.observe("final_registry", current);
    service.pass();
}
