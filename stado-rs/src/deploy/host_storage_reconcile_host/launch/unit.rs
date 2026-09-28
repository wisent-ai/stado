//! The resident worker's native unit: a system launchd daemon on Darwin or a
//! systemd service on Linux, running the transaction tool as the managed
//! account. The operation lock is released just before the manager starts
//! the worker, so the worker takes it.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use plist::{Dictionary, Value as Plist};

use super::Launch;
use crate::deploy::host_storage_reconcile_host::{checked, home};
use crate::deploy::shlex_quote;

const SEARCH_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const WORKER_WRAPPER_MODE: u32 = 0o700;
/// `launchctl bootout` exit statuses that mean there was nothing to remove.
const BOOTOUT_ABSENT: [i32; 3] = [0, 3, 113];

fn account() -> Result<String, String> {
    let output = checked(&["/usr/bin/id", "-un"], &[0])?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Write `bytes` beside `path` durably, then rename it into place.
fn replace_durably(path: &str, bytes: &[u8], mode: Option<u32>) -> Result<(), String> {
    let staged = format!("{path}.new");
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode.unwrap_or(0o644))
            .open(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if let Some(mode) = mode {
            fs::set_permissions(&staged, fs::Permissions::from_mode(mode))?;
        }
        fs::rename(&staged, path)
    })();
    written.map_err(|error| format!("cannot write {path}: {error}"))
}

fn sudo(arguments: &[&str], accepted: &[i32]) -> Result<(), String> {
    let mut argv = vec!["/usr/bin/sudo", "-n"];
    argv.extend_from_slice(arguments);
    checked(&argv, accepted).map(|_| ())
}

fn install_launchd(
    launch: &Launch,
    release_api: &str,
    unlock: impl FnOnce(),
) -> Result<(), String> {
    let home = home()?;
    let mut environment = Dictionary::new();
    environment.insert("HOME".into(), Plist::String(home.clone()));
    environment.insert("PATH".into(), Plist::String(SEARCH_PATH.to_string()));
    environment.insert(
        "STADO_API_URL".into(),
        Plist::String(release_api.to_string()),
    );
    let mut unit = Dictionary::new();
    unit.insert("Label".into(), Plist::String(launch.label.clone()));
    let argv = launch.argv.iter().cloned().map(Plist::String).collect();
    unit.insert("ProgramArguments".into(), Plist::Array(argv));
    unit.insert(
        "EnvironmentVariables".into(),
        Plist::Dictionary(environment),
    );
    unit.insert("WorkingDirectory".into(), Plist::String(home));
    unit.insert("RunAtLoad".into(), Plist::Boolean(true));
    unit.insert("KeepAlive".into(), Plist::Boolean(false));
    unit.insert(
        "ProcessType".into(),
        Plist::String("Background".to_string()),
    );
    unit.insert("UserName".into(), Plist::String(account()?));
    unit.insert(
        "StandardOutPath".into(),
        Plist::String(launch.log_path.clone()),
    );
    unit.insert(
        "StandardErrorPath".into(),
        Plist::String(launch.log_path.clone()),
    );
    let mut bytes = Vec::new();
    Plist::Dictionary(unit)
        .to_writer_xml(&mut bytes)
        .map_err(|error| format!("cannot render the worker unit: {error}"))?;
    let prepared = format!("{}/native-worker.plist", launch.work);
    replace_durably(&prepared, &bytes, None)?;
    let label = &launch.label;
    let unit_path = format!("/Library/LaunchDaemons/{label}.plist");
    let service = format!("system/{label}");
    sudo(&["/bin/launchctl", "bootout", &service], &BOOTOUT_ABSENT)?;
    sudo(
        &[
            "/usr/bin/install",
            "-m",
            "644",
            "-o",
            "root",
            "-g",
            "wheel",
            &prepared,
            &unit_path,
        ],
        &[0],
    )?;
    sudo(&["/bin/launchctl", "enable", &service], &[0])?;
    unlock();
    sudo(&["/bin/launchctl", "bootstrap", "system", &unit_path], &[0])?;
    sudo(&["/bin/launchctl", "kickstart", &service], &[0])
}

fn install_systemd(
    launch: &Launch,
    release_api: &str,
    unlock: impl FnOnce(),
) -> Result<(), String> {
    let home = home()?;
    let wrapper = format!("{}/native-worker", launch.work);
    let command: Vec<String> = launch.argv.iter().map(|word| shlex_quote(word)).collect();
    let script = format!("#!/bin/sh\nexec {}\n", command.join(" "));
    replace_durably(&wrapper, script.as_bytes(), Some(WORKER_WRAPPER_MODE))?;
    let label = &launch.label;
    let service = format!("{label}.service");
    let unit_path = format!("/etc/systemd/system/{service}");
    let prepared = format!("{}/native-worker.service", launch.work);
    let unit = [
        "[Unit]".to_string(),
        format!(
            "Description=Stado storage authority reconciliation {}",
            launch.transaction
        ),
        "After=network-online.target".to_string(),
        "[Service]".to_string(),
        "Type=simple".to_string(),
        format!("User={}", account()?),
        format!("Environment=HOME={home}"),
        format!("Environment=STADO_API_URL={release_api}"),
        format!("WorkingDirectory={home}"),
        format!("ExecStart={wrapper}"),
        "Restart=no".to_string(),
        "[Install]".to_string(),
        "WantedBy=multi-user.target".to_string(),
        String::new(),
    ]
    .join("\n");
    replace_durably(&prepared, unit.as_bytes(), None)?;
    sudo(
        &[
            "/usr/bin/install",
            "-m",
            "644",
            "-o",
            "root",
            "-g",
            "root",
            &prepared,
            &unit_path,
        ],
        &[0],
    )?;
    sudo(&["/bin/systemctl", "daemon-reload"], &[0])?;
    sudo(&["/bin/systemctl", "enable", &service], &[0])?;
    unlock();
    sudo(&["/bin/systemctl", "start", &service], &[0])
}

/// Install and start the worker's unit; `unlock` releases the operation
/// lock at the moment the manager is about to start it.
pub(super) fn install(
    launch: &Launch,
    release_api: &str,
    unlock: impl FnOnce(),
) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        install_launchd(launch, release_api, unlock)
    } else {
        install_systemd(launch, release_api, unlock)
    }
}
