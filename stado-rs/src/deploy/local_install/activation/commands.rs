//! The argv each init system is driven with, and the uid that names a
//! per-login launchd domain.

use std::path::Path;

use crate::deploy::local_install::systemd_unit;
use crate::deploy::CommandSpec;

/// Python `_install_darwin` launchctl argv: (bootout, bootstrap, kickstart).
pub fn darwin_commands(label: &str, plist_path: &Path, uid: u32) -> [CommandSpec; 3] {
    [
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "bootout".to_string(),
            format!("gui/{uid}/{label}"),
        ]),
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "bootstrap".to_string(),
            format!("gui/{uid}"),
            plist_path.to_string_lossy().into_owned(),
        ]),
        CommandSpec::new(vec![
            "launchctl".to_string(),
            "kickstart".to_string(),
            "-k".to_string(),
            format!("gui/{uid}/{label}"),
        ]),
    ]
}

/// Python `_install_linux` systemctl argv: (daemon-reload, enable --now).
pub fn linux_commands(label: &str) -> [CommandSpec; 2] {
    [
        CommandSpec::new(vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "daemon-reload".to_string(),
        ]),
        CommandSpec::new(vec![
            "systemctl".to_string(),
            "--user".to_string(),
            "enable".to_string(),
            "--now".to_string(),
            systemd_unit(label),
        ]),
    ]
}

/// The uid for `gui/<uid>` launchd domains (Python `os.getuid()`).
pub fn current_uid() -> u32 {
    // SAFETY: getuid cannot fail.
    unsafe { nix::libc::getuid() }
}
