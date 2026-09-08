//! The systemd half of the post-swap reconcile: address the same owner and
//! runtime the service verbs use, then try-restart each running unit whose
//! main process still maps a replaced inode.

use std::path::Path;

use super::recycle::defers_to_release_handshake;

async fn systemctl_stdout(user: bool, args: &[&str]) -> Result<String, String> {
    let mut command = tokio::process::Command::new("systemctl");
    if user {
        // A system-scoped queue worker does not inherit a login's user-bus
        // environment. Address the same owner and runtime as service verbs.
        // SAFETY: geteuid has no arguments or memory preconditions.
        let uid = unsafe { nix::libc::geteuid() };
        let runtime = format!("/run/user/{uid}");
        command.arg("--user").env("XDG_RUNTIME_DIR", &runtime).env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        );
    }
    let output = command
        .args(args)
        .output()
        .await
        .map_err(|error| format!("cannot execute systemctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl {} exited {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("systemctl returned invalid UTF-8: {error}"))
}

pub(super) async fn recycle_systemd(
    context: &str,
    paths: &[String],
    ours: u32,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, String> {
    let mut restarted = 0usize;
    for user in [false, true] {
        let listing = systemctl_stdout(
            user,
            &[
                "list-units",
                "--type=service",
                "--state=running",
                "--no-legend",
                "--plain",
            ],
        )
        .await
        .map_err(|error| {
            format!(
                "{context}: {} systemd manager did not enumerate running services: {error}",
                if user { "user" } else { "system" }
            )
        })?;
        for line in listing.lines() {
            let Some(unit) = line.split_whitespace().next() else {
                continue;
            };
            let show_args = ["show", "-p", "MainPID", "--value", unit];
            let main_pid = systemctl_stdout(user, &show_args)
                .await
                .map_err(|error| format!("{context}: {unit} did not report MainPID: {error}"))?
                .trim()
                .parse::<u32>()
                .map_err(|error| format!("{context}: {unit} reported invalid MainPID: {error}"))?;
            if main_pid == 0 {
                return Err(format!(
                    "{context}: {unit} was listed running but reported MainPID=0"
                ));
            }
            if main_pid == ours {
                continue;
            }
            let images = crate::deploy::service::running_images(&[main_pid]).map_err(|error| {
                format!("{context}: cannot read {unit} pid {main_pid}: {error}")
            })?;
            let running = images
                .get(&main_pid)
                .ok_or_else(|| format!("{context}: no kernel image for {unit} pid {main_pid}"))?;
            let Some(path) = paths
                .iter()
                .find(|path| running.path.trim_end_matches(" (deleted)") == path.as_str())
            else {
                continue;
            };
            let (installed, _) = crate::deploy::service::installed_image(Path::new(path))
                .map_err(|error| format!("{context}: cannot identify installed {path}: {error}"))?;
            if running.is_same_file(&installed) {
                continue;
            }
            let argv = crate::deploy::service::process_table()
                .ok()
                .and_then(|rows| {
                    rows.into_iter()
                        .find(|(pid, _, _)| *pid == main_pid)
                        .map(|(_, _, argv)| argv)
                })
                .ok_or_else(|| format!("{context}: cannot read argv for {unit} pid {main_pid}"));
            let argv = argv?;
            let tokens: Vec<&str> = argv.split_whitespace().collect();
            if defers_to_release_handshake(&tokens) {
                log_fn(&format!(
                    "{context}: {unit} is the queue agent and defers to its installed-release handshake"
                ));
                continue;
            }
            systemctl_stdout(user, &["try-restart", unit])
                .await
                .map_err(|error| {
                    format!(
                        "{context}: {unit} pid {main_pid} could not be restarted onto the installed inode: {error}"
                    )
                })?;
            let mut replacement = None;
            for _ in 0..60 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let Some(new_pid) = systemctl_stdout(user, &show_args)
                    .await
                    .ok()
                    .and_then(|value| value.trim().parse::<u32>().ok())
                    .filter(|pid| *pid != 0)
                else {
                    continue;
                };
                let verified = crate::deploy::service::running_images(&[new_pid])
                    .ok()
                    .is_some_and(|images| {
                        images
                            .get(&new_pid)
                            .is_some_and(|image| image.is_same_file(&installed))
                    });
                if verified {
                    replacement = Some(new_pid);
                    break;
                }
            }
            let Some(new_pid) = replacement else {
                return Err(format!(
                    "{context}: {unit} restarted but no replacement pid mapped the installed inode within 30s"
                ));
            };
            log_fn(&format!(
                "{context}: restarted {unit}; pid {main_pid} held the replaced inode and pid {new_pid} maps the installed inode"
            ));
            restarted += 1;
        }
    }
    Ok(restarted)
}
