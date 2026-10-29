//! What macOS lets the Stado executable read, measured by that executable,
//! judged against the grants the registry declares for the host, and the
//! privacy requests macOS recorded against it.
//!
//! macOS decides per program whether it may open Documents, Desktop and
//! Downloads, and a decision once clicked stays until the operator changes it
//! in System Settings. No program can grant itself access and nothing else
//! reads the decision for it, so a denial clicked once left every background
//! product sync failing with `Operation not permitted` for weeks while the
//! failure read as a rejected credential. `targets[].privacy_grants` (see
//! [`crate::targets::PrivacyGrant`]) says which program needs which folder,
//! and the host process measures exactly the folders declared for it, so
//! every beacon carries the reality check of each declaration. A folder no
//! grant declares for it is never opened: opening it is the question macOS
//! asks the person at the keyboard, and an undeclared question is the defect.
//!
//! macOS attributes an access to the app a command was started from, so the
//! beacon the host process publishes (the `--health-interval-seconds` role of
//! `com.wisent.stado`) is the background process's answer, while a
//! `beacon collect` typed in a terminal measures that terminal's grant.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::targets::{ComputeTarget, PrivacyGrant, PRIVACY_FOLDERS as FOLDERS};

/// The System Settings pane that holds the Files and Folders decisions.
const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

/// What the measuring program found when it opened one protected folder, or
/// what macOS last decided about Local Network for a program. The beacon
/// carries a folder's as `state`; readers parse it into this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum FolderAccess {
    Granted,
    /// The folder does not exist; there is nothing to read and nothing denied.
    Absent,
    /// The kernel's EPERM, macOS's privacy refusal.
    Denied,
    /// Any other failed read, with the operating system's error.
    Unreadable,
    /// macOS put the Local Network question to the person at the keyboard
    /// and logged no decision after it.
    Pending,
    /// The grant is for a program other than the one that measured: macOS
    /// answers only the program asking.
    #[serde(rename = "not measured")]
    NotMeasured,
}

impl FolderAccess {
    /// The declared folder could not be read.
    fn refused(self) -> bool {
        matches!(self, Self::Denied | Self::Unreadable)
    }

    /// The state a measured folder entry carries; an entry that carries no
    /// state this build reads is the error naming the folder and why.
    fn of_entry(folder: &str, entry: &Value) -> Result<Self, CmdError> {
        <Self as serde::Deserialize>::deserialize(&entry["state"]).map_err(|error| {
            CmdError::click(format!(
                "the beacon's privacy entry for {folder} carries no state this Stado reads \
                 ({error}): {entry}"
            ))
        })
    }
}

impl std::fmt::Display for FolderAccess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}

/// The beacon's `privacy` block: the measuring program and, per folder, its
/// [`FolderAccess`] state, with the operating system's own error as `detail`
/// for a read that failed for any reason but absence. Only the folders
/// `target` declares for this program are opened; every other is `not
/// measured`, with the reason.
pub(super) fn measure(home: &Path, target: &ComputeTarget) -> Value {
    let program = std::env::current_exe();
    let (declared, grants_error) = match (&program, target.privacy_grants()) {
        (Ok(program), Ok(grants)) => (crate::targets::folders_for(&grants, home, program), None),
        (Err(_), Ok(_)) => (std::collections::BTreeSet::new(), None),
        (_, Err(error)) => (std::collections::BTreeSet::new(), Some(error.to_string())),
    };
    let mut folders = Map::new();
    for (key, name) in FOLDERS {
        let path = home.join(name);
        let entry = if !declared.contains(*key) {
            json!({
                "state": FolderAccess::NotMeasured, "path": path,
                "detail": format!(
                    "targets.{}.privacy_grants declares {name} for no program this process runs, \
                     so it does not open it",
                    target.name
                ),
            })
        } else {
            match std::fs::read_dir(&path) {
                Ok(_) => json!({"state": FolderAccess::Granted, "path": path}),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    json!({"state": FolderAccess::Absent, "path": path})
                }
                Err(error)
                    if cfg!(target_os = "macos")
                        && error.raw_os_error() == Some(nix::libc::EPERM) =>
                {
                    json!({"state": FolderAccess::Denied, "path": path, "detail": error.to_string()})
                }
                Err(error) => json!({
                    "state": FolderAccess::Unreadable, "path": path, "detail": error.to_string()
                }),
            }
        };
        folders.insert(key.to_string(), entry);
    }
    let mut block = match program {
        Ok(program) => json!({"program": program, "folders": folders}),
        Err(error) => json!({
            "program": Value::Null,
            "program_error": error.to_string(),
            "folders": folders,
        }),
    };
    if let Some(error) = grants_error {
        block["grants_error"] = json!(error);
    }
    block
}

/// One declared grant beside what the beacon measured for it.
struct Verdict<'a> {
    grant: &'a PrivacyGrant,
    path: PathBuf,
    state: FolderAccess,
}

impl Verdict<'_> {
    fn to_json(&self) -> Value {
        json!({
            "program": self.grant.program, "folder": self.grant.folder,
            "reason": self.grant.reason, "path": self.path, "state": self.state,
            "refused": self.state.refused(),
        })
    }
}

/// The name of the folder or decision `key` declares, or the key itself for
/// one this build does not know.
fn folder_name(key: &str) -> &str {
    match crate::targets::privacy_keys().find(|(declared, _)| *declared == key) {
        Some((_, name)) => name,
        None => key,
    }
}

/// `stado host privacy TARGET [--json] [--open] [--since WINDOW]` — TARGET's
/// declared grants beside what its Stado process may read, from its latest
/// beacon, and with `since` the privacy requests TARGET's unified log
/// recorded against that program in the window. Exits non-zero when a
/// declared grant is denied, the beacon carries no measurement, or a request
/// reached for protected data no grant declares; `--open` opens the Files and
/// Folders pane when TARGET is the machine running this command.
pub async fn privacy(
    target: &str,
    json: bool,
    open: bool,
    since: Option<&str>,
) -> Result<(), CmdError> {
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(CmdError::from)?;
    if let Some(sentence) = notice {
        eprintln!("{sentence}");
    }
    let resolved = crate::cli::resolved_host(&registry, target)?;
    let grants = resolved
        .privacy_grants()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let store = crate::cli::host::checks::health::beacon_store().await?;
    let report = crate::monitor::host_health::load_host_health(&store, target)
        .await
        .map_err(CmdError::from)?;
    // The beacon names the host it measured; one that names none is not a
    // measurement this command can attribute.
    let Some(host) = report.beacon.get("host").and_then(Value::as_str) else {
        return Err(CmdError::click(format!(
            "{target}'s latest beacon names no host, so its privacy measurement cannot be \
             attributed"
        )));
    };
    let reported_at = match report.beacon.get("reported_at").and_then(Value::as_str) {
        Some(at) => at.to_string(),
        None => "a time the beacon does not record".to_string(),
    };
    let Some(block) = report.beacon.get("privacy") else {
        return Err(CmdError::click(format!(
            "{host}'s latest beacon (reported {reported_at}) carries no privacy measurement: the \
             Stado publishing it predates the measurement; install a current Stado on {host}"
        ))
        .stating(FailureCode::NotFound));
    };
    // The program that measured; a beacon whose program could not name
    // itself carries the reason instead, and then no grant was measured.
    let program = block.get("program").and_then(Value::as_str);
    let program_line = match (program, block.get("program_error").and_then(Value::as_str)) {
        (Some(program), _) => program.to_string(),
        (None, Some(error)) => format!("not named ({error})"),
        (None, None) => "not named, and the beacon says no reason".to_string(),
    };
    // The measured folders sit in the measuring user's home, which is what
    // `~/` in a declared program means on that host.
    let Some(home) = block["folders"]["documents"]["path"]
        .as_str()
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf)
    else {
        return Err(CmdError::click(format!(
            "{host}'s privacy measurement (reported {reported_at}) names no Documents path, so \
             the home its declared programs live in is not known: {block}"
        )));
    };
    // Local Network is no folder the beacon opens: with `--since` the host's
    // log says what macOS last decided for each program a grant names.
    let local_network_programs: Vec<String> = grants
        .iter()
        .filter(|grant| grant.folder == crate::targets::LOCAL_NETWORK.0)
        .map(|grant| grant.program_on(&home).to_string_lossy().into_owned())
        .collect();
    let local_network = match since {
        Some(window) => crate::deploy::host_privacy::local_network::decisions_on_host(
            resolved,
            &local_network_programs,
            window,
            &crate::deploy::production_runner(),
        )
        .await
        .map_err(CmdError::from)?,
        None => Vec::new(),
    };
    let mut verdicts: Vec<Verdict> = Vec::new();
    for grant in &grants {
        let path = grant.program_on(&home);
        let state = if grant.folder == crate::targets::LOCAL_NETWORK.0 {
            use crate::deploy::host_privacy::local_network::LocalNetwork;
            match local_network
                .iter()
                .find(|decision| Path::new(&decision.program) == path.as_path())
                .map(|decision| decision.state)
            {
                Some(LocalNetwork::Granted) => FolderAccess::Granted,
                Some(LocalNetwork::Denied) => FolderAccess::Denied,
                Some(LocalNetwork::Pending) => FolderAccess::Pending,
                Some(LocalNetwork::NotMeasured) | None => FolderAccess::NotMeasured,
            }
        } else if program.is_some_and(|program| path.as_path() == Path::new(program)) {
            FolderAccess::of_entry(&grant.folder, &block["folders"][grant.folder.as_str()])?
        } else {
            FolderAccess::NotMeasured
        };
        verdicts.push(Verdict { grant, path, state });
    }
    let refused: Vec<&Verdict> = verdicts
        .iter()
        .filter(|verdict| verdict.state.refused())
        .collect();
    // What macOS recorded against the measured program, when asked: read on
    // the host, joined there, and judged against the folders declared for
    // that very program.
    let requests = match (since, program) {
        (None, _) => None,
        (Some(_), None) => {
            return Err(CmdError::click(format!(
                "{host}'s latest beacon (reported {reported_at}) names no program, so the privacy \
                 requests recorded against it cannot be read: {program_line}"
            )))
        }
        (Some(window), Some(program)) => {
            let runner = crate::deploy::production_runner();
            Some(
                crate::deploy::host_privacy::requests_on_host(resolved, program, window, &runner)
                    .await
                    .map_err(CmdError::from)?,
            )
        }
    };
    let declared_here = match program {
        Some(program) => crate::targets::folders_for(&grants, &home, Path::new(program)),
        None => std::collections::BTreeSet::new(),
    };
    let undeclared: Vec<&crate::deploy::host_privacy::PrivacyRequest> = requests
        .iter()
        .flatten()
        .filter(|request| request.undeclared(&declared_here))
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
        let status =
            crate::wait::status(std::process::Command::new("/usr/bin/open").arg(SETTINGS_URL))
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
                "requests_since": since, "requests": requests,
                "undeclared_requests": undeclared, "local_network": local_network,
            }))?
        );
    } else {
        println!("host:        {host}");
        println!("measured_at: {reported_at}");
        println!("program:     {program_line}");
        for (key, name) in FOLDERS {
            let entry = &block["folders"][key];
            let state = FolderAccess::of_entry(key, entry)?;
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
        if let (Some(window), Some(requests)) = (since, &requests) {
            println!("requests:    {} in the last {window}", requests.len());
            for request in requests {
                let verdict = if request.undeclared(&declared_here) {
                    " UNDECLARED"
                } else {
                    ""
                };
                println!("request:     {request}{verdict}");
            }
        }
    }
    let mut failures = Vec::new();
    if !refused.is_empty() {
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
        failures.push(format!(
            "macOS denies declared grants on {host}: {}; allow each in System Settings → Privacy \
             & Security → Files and Folders (or add the program to Full Disk Access), which \
             `stado host privacy {target} --open` opens on {host}",
            named.join("; ")
        ));
    }
    if !undeclared.is_empty() {
        let named: Vec<String> = undeclared
            .iter()
            .map(|request| {
                format!(
                    "{} at {} by {}",
                    request.service, request.at, request.accessing
                )
            })
            .collect();
        failures.push(format!(
            "{host}'s {program_line} reached for protected data no targets.{target}.privacy_grants \
             entry declares: {}; the code path that opened it must go through the one privacy \
             list, or the need must be declared",
            named.join("; ")
        ));
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(failures.join("; and ")).stating(FailureCode::Refused))
}
