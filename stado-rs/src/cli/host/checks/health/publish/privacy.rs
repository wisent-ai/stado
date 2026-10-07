//! What macOS lets the Stado executable read, measured by that executable,
//! judged against the grants the registry declares for the host.
//!
//! macOS decides per program whether it may open Documents, Desktop and
//! Downloads, and a decision once clicked stays until the operator changes it
//! in System Settings. No program can grant itself access and nothing else
//! reads the decision for it, so a denial clicked once left every background
//! product sync failing with `Operation not permitted` for weeks while the
//! failure read as a rejected credential. The host process therefore measures
//! its own access and every beacon carries it; `targets[].privacy_grants`
//! (see [`crate::targets::PrivacyGrant`]) says which program needs which
//! folder, so a denial is a defect only where a declaration asks for access.
//!
//! macOS attributes an access to the app a command was started from, so the
//! beacon the host process publishes (the `--health-interval-seconds` role of
//! `com.wisent.stado`) is the background process's answer, while a
//! `collect-beacon` typed in a terminal measures that terminal's grant.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::targets::{PrivacyGrant, PRIVACY_FOLDERS as FOLDERS};

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

/// One declared grant beside what the beacon measured for it.
struct Verdict<'a> {
    grant: &'a PrivacyGrant,
    path: PathBuf,
    /// The measured state, or `not measured` for a program other than the
    /// one that published the beacon: macOS answers only the program asking.
    state: String,
}

impl Verdict<'_> {
    /// A declared folder the program may not read. `absent` is no denial:
    /// there is nothing to read.
    fn refused(&self) -> bool {
        self.state == "denied" || self.state == "unreadable"
    }

    fn to_json(&self) -> Value {
        json!({
            "program": self.grant.program, "folder": self.grant.folder,
            "reason": self.grant.reason, "path": self.path, "state": self.state,
        })
    }
}

fn folder_name(key: &str) -> &str {
    FOLDERS
        .iter()
        .find(|(declared, _)| *declared == key)
        .map_or(key, |(_, name)| *name)
}

/// `stado host privacy TARGET [--json] [--open]` — TARGET's declared grants
/// beside what its Stado process may read, from its latest beacon. Exits
/// non-zero when a declared grant is denied or the beacon carries no
/// measurement; `--open` opens the Files and Folders pane when TARGET is the
/// machine running this command.
pub async fn privacy(target: &str, json: bool, open: bool) -> Result<(), CmdError> {
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(CmdError::from)?;
    if let Some(sentence) = notice {
        eprintln!("{sentence}");
    }
    let grants = crate::cli::resolved_host(&registry, target)?.privacy_grants();
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
    // The measured folders sit in the measuring user's home, which is what
    // `~/` in a declared program means on that host.
    let home = block["folders"]["documents"]["path"]
        .as_str()
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let verdicts: Vec<Verdict> = grants
        .iter()
        .map(|grant| {
            let path = grant.program_on(&home);
            let state = if path.as_path() == Path::new(program) {
                block["folders"][grant.folder.as_str()]["state"]
                    .as_str()
                    .unwrap_or("not measured")
                    .to_string()
            } else {
                "not measured".to_string()
            };
            Verdict { grant, path, state }
        })
        .collect();
    let refused: Vec<&Verdict> = verdicts
        .iter()
        .filter(|verdict| verdict.refused())
        .collect();
    if open {
        if !super::beacon_is_this_host(host) {
            return Err(CmdError::refused(format!(
                "--open opens System Settings on the machine running this command, and {host} is \
                 another machine; run `stado host privacy {host} --open` on {host}"
            )));
        }
        // A program that cannot start carries its operating-system kind; an
        // `open` that ran and failed is macOS refusing the settings pane.
        let status = std::process::Command::new("/usr/bin/open")
            .arg(SETTINGS_URL)
            .status()
            .map_err(|error| {
                CmdError::click(format!(
                    "/usr/bin/open {SETTINGS_URL} could not start: {error}"
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        if !status.success() {
            return Err(CmdError::click(format!(
                "/usr/bin/open {SETTINGS_URL} exited {status}: macOS did not open \
                 Privacy & Security → Files and Folders"
            ))
            .stating(FailureCode::Refused));
        }
    }
    if json {
        let grants: Vec<Value> = verdicts.iter().map(Verdict::to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": host, "reported_at": reported_at, "privacy": block,
                "grants": grants, "settings_url": SETTINGS_URL,
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
        if verdicts.is_empty() {
            println!("declared:    none (targets.{target}.privacy_grants)");
        }
        for verdict in &verdicts {
            println!(
                "declared:    {} {} {} — {}",
                verdict.path.display(),
                folder_name(&verdict.grant.folder),
                verdict.state,
                verdict.grant.reason
            );
        }
    }
    if refused.is_empty() {
        return Ok(());
    }
    let named: Vec<String> = refused
        .iter()
        .map(|verdict| {
            format!(
                "{} {} (declared: {})",
                verdict.path.display(),
                folder_name(&verdict.grant.folder),
                verdict.grant.reason
            )
        })
        .collect();
    Err(CmdError::click(format!(
        "macOS denies declared grants on {host}: {}; allow each in System Settings → Privacy & \
         Security → Files and Folders (or add the program to Full Disk Access), which `stado host \
         privacy {target} --open` opens on {host}",
        named.join("; ")
    ))
    .stating(FailureCode::Refused))
}
