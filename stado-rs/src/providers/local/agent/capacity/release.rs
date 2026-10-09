//! The installed-release handoff: when this process should end so its
//! supervisor starts a different image.

const INSTALLED_STADO_RELEASE_VERSION: &str = "stado.release-version";

/// What the managed binary on disk says it is, asked of the file itself.
fn managed_binary_version(managed: &std::path::Path) -> Option<String> {
    let output = crate::wait::output(std::process::Command::new(managed).arg("--version")).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace().nth(1).map(str::to_string)
}

/// A release handoff worth taking: the marker names a release this process is
/// not, AND the binary on disk is no longer this process's image -- so exiting
/// hands control to a genuinely different file and the supervisor's restart
/// changes something.
///
/// The marker alone cannot decide that, and reading it as authoritative can
/// cost a host its entire share of the queue: with a newer binary at the
/// managed path beside a marker still naming an older version, the agent
/// announces `installed <old> supersedes running <new>`, exits, is recreated
/// by launchd from that same file, and repeats it every ten seconds --
/// claiming nothing, while a release delivery pinned to that host sits
/// queued behind it. A handoff whose restart cannot change the
/// running image is not a handoff, it is a stall with an explanation.
///
/// So the file is asked what it is. When it answers this process's own version
/// the marker is merely stale: it is corrected here, once, instead of being
/// re-read forever. Only a file that answers something else is a release
/// waiting to be started. Source-tree recovery agents stay excluded, because
/// only an agent launched through the owner-managed binary belongs to a
/// supervisor that recreates it.
pub(crate) fn installed_stado_release_mismatch(log_fn: &mut dyn FnMut(&str)) -> Option<String> {
    let home = crate::config_file::expand_tilde("~");
    let managed = home.join(".stado").join("bin").join("stado");
    let argv0 = std::env::args_os().next().map(std::path::PathBuf::from)?;
    if argv0 != managed {
        return None;
    }
    let marker = home
        .join(".stado")
        .join("bin")
        .join(INSTALLED_STADO_RELEASE_VERSION);
    let installed = std::fs::read_to_string(&marker).ok()?;
    let installed = installed.trim();
    let running = env!("CARGO_PKG_VERSION");
    if installed.is_empty() {
        return None;
    }
    if installed == running {
        return replaced_image(&managed, running, log_fn);
    }
    match managed_binary_version(&managed) {
        Some(on_disk) if on_disk != running => Some(installed.to_string()),
        Some(on_disk) => {
            match std::fs::write(&marker, format!("{on_disk}\n")) {
                Ok(()) => log_fn(&format!(
                    "loop: release-marker-repaired: {} named {installed} while the managed binary \
                     is {on_disk}; the marker now names what is installed",
                    marker.display()
                )),
                Err(error) => log_fn(&format!(
                    "loop: release-marker-stale: {} names {installed} while the managed binary is \
                     {on_disk}, and it could not be corrected: {error}",
                    marker.display()
                )),
            }
            None
        }
        None => {
            log_fn(&format!(
                "loop: release-marker-unverified: {} names {installed} and the managed binary \
                 would not state its version; continuing rather than handing off to an image that \
                 may not differ",
                marker.display()
            ));
            None
        }
    }
}

/// The same version installed again under this process: the marker cannot
/// tell it apart, and the process keeps executing the replaced inode for good:
/// `stado product install stado` reinstalling the same version over a running
/// agent leaves the marker unchanged, nothing hands off, and the unit keeps an
/// image no longer on disk.
/// When this process's image is not the managed file, a restart does change
/// what runs, so the handoff is taken.
fn replaced_image(
    managed: &std::path::Path,
    running: &str,
    log_fn: &mut dyn FnMut(&str),
) -> Option<String> {
    let (installed, _) = crate::deploy::service::installed_image(managed).ok()?;
    let own = std::process::id();
    let images = crate::deploy::service::running_images(&[own]).ok()?;
    let image = images.get(&own)?;
    if image.is_same_file(&installed) {
        return None;
    }
    log_fn(&format!(
        "loop: release-image-replaced: {} was reinstalled at {running}, and this process still \
         executes the file it replaced; handing off so the supervisor starts the installed one",
        managed.display()
    ));
    Some(running.to_string())
}
