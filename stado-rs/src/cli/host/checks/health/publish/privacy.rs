//! What macOS lets the Stado executable read, measured by that executable.
//!
//! macOS decides per program whether it may open Documents, Desktop and
//! Downloads, and a decision once clicked stays until the operator changes it
//! in System Settings. No program can grant itself access and nothing else
//! reads the decision for it, so a denial clicked once left every background
//! product sync failing with `Operation not permitted` for weeks while the
//! failure read as a rejected credential. The host process therefore measures
//! its own access and every beacon carries it.
//!
//! macOS attributes an access to the app a command was started from, so the
//! beacon the host process publishes (the `--health-interval-seconds` role of
//! `com.wisent.stado`) is the background process's answer, while a
//! `collect-beacon` typed in a terminal measures that terminal's grant.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// The protected folders a Stado role reads, by beacon key and folder name.
const FOLDERS: [(&str, &str); 3] = [
    ("documents", "Documents"),
    ("desktop", "Desktop"),
    ("downloads", "Downloads"),
];

/// The System Settings pane that holds the Files and Folders decisions.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

/// The beacon's `privacy` block: the measuring program and, per folder,
/// `granted`, `denied` (the kernel's EPERM, macOS's privacy refusal),
/// `absent` or `unreadable` with the operating system's own error.
pub(super) fn measure() -> Value {
    let home = crate::config_file::expand_tilde("~");
    let mut folders = Map::new();
    for (key, name) in FOLDERS {
        let path = home.join(name);
        let entry = match std::fs::read_dir(&path) {
            Ok(_) => json!({"state": "granted", "path": path}),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                json!({"state": "absent", "path": path})
            }
            Err(error)
                if cfg!(target_os = "macos") && error.raw_os_error() == Some(nix::libc::EPERM) =>
            {
                json!({"state": "denied", "path": path, "detail": error.to_string()})
            }
            Err(error) => json!({"state": "unreadable", "path": path, "detail": error.to_string()}),
        };
        folders.insert(key.to_string(), entry);
    }
    match std::env::current_exe() {
        Ok(program) => json!({"program": program, "folders": folders}),
        Err(error) => json!({
            "program": Value::Null,
            "program_error": error.to_string(),
            "folders": folders,
        }),
    }
}

/// `stado host privacy TARGET [--json] [--open]` — what TARGET's Stado
/// process may read, from its latest beacon. Exits non-zero when a folder is
/// denied or the beacon carries no measurement; `--open` opens the Files and
/// Folders pane when TARGET is the machine running this command.
pub async fn privacy(target: &str, json: bool, open: bool) -> Result<(), CmdError> {
    let store = crate::cli::host::checks::health::beacon_store().await?;
    let report = crate::monitor::host_health::load_host_health(&store, target)
        .await
        .map_err(CmdError::from)?;
    let host = report
        .beacon
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or(target);
    let reported_at = report
        .beacon
        .get("reported_at")
        .and_then(Value::as_str)
        .unwrap_or("an unrecorded time");
    let Some(block) = report.beacon.get("privacy") else {
        return Err(CmdError::click(format!(
            "{host}'s latest beacon (reported {reported_at}) carries no privacy measurement: the \
             Stado publishing it predates the measurement; install a current Stado on {host}"
        ))
        .stating(FailureCode::NotFound));
    };
    let program = block
        .get("program")
        .and_then(Value::as_str)
        .unwrap_or("the Stado executable");
    let denied: Vec<&str> = FOLDERS
        .iter()
        .filter(|(key, _)| block["folders"][*key]["state"] == "denied")
        .map(|(_, name)| *name)
        .collect();
    if open {
        if !super::beacon_is_this_host(host) {
            return Err(CmdError::refused(format!(
                "--open opens System Settings on the machine running this command, and {host} is \
                 another machine; run `stado host privacy {host} --open` on {host}"
            )));
        }
        let status = std::process::Command::new("/usr/bin/open")
            .arg(SETTINGS_URL)
            .status()
            .map_err(|error| {
                CmdError::click(format!(
                    "/usr/bin/open {SETTINGS_URL} could not start: {error}"
                ))
            })?;
        if !status.success() {
            return Err(CmdError::click(format!(
                "/usr/bin/open {SETTINGS_URL} exited {status}"
            )));
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": host, "reported_at": reported_at, "privacy": block,
                "settings_url": SETTINGS_URL,
            }))?
        );
    } else {
        println!("host:        {host}");
        println!("measured_at: {reported_at}");
        println!("program:     {program}");
        for (key, name) in FOLDERS {
            let entry = &block["folders"][key];
            let state = entry["state"].as_str().unwrap_or("not measured");
            match entry["detail"].as_str() {
                Some(detail) => println!("{name:<12} {state} ({detail})"),
                None => println!("{name:<12} {state}"),
            }
        }
    }
    if denied.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "macOS denies {program} on {host}: {}; allow it in System Settings → Privacy & Security → \
         Files and Folders (or add it to Full Disk Access), which `stado host privacy {host} \
         --open` opens on {host}",
        denied.join(", ")
    ))
    .stating(FailureCode::Refused))
}
