// Its only caller is the `#[cfg(target_os = "macos")]` branch below, and the
// function it names carries the same gate, so the import needs it too: on
// Linux the item does not exist.
#[cfg(target_os = "macos")]
use super::stale::selected_macos_shell;
use crate::deploy::service::*;

/// The identity of the file at `path` right now, and when it was last written.
///
/// Symlinks are followed, which is the point and not a convenience: a declared
/// program is routinely a link — `~/.local/bin/transcript-lake` is one, and
/// every staged release reaches its binary through a `current` link — and the
/// identity that matters is the one an `exec` of that path would land on
/// today.
pub fn installed_image(path: &Path) -> Result<(ImageIdentity, i64), String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    #[cfg(target_os = "macos")]
    let (path, metadata) = if std::fs::metadata("/bin/sh")
        .is_ok_and(|shell| shell.dev() == metadata.dev() && shell.ino() == metadata.ino())
    {
        let selected = selected_macos_shell()?;
        let selected_metadata = std::fs::metadata(&selected).map_err(|error| error.to_string())?;
        (std::borrow::Cow::Owned(selected), selected_metadata)
    } else {
        (std::borrow::Cow::Borrowed(path), metadata)
    };
    Ok((
        ImageIdentity {
            path: path.to_string_lossy().into_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.size(),
            links: metadata.nlink(),
        },
        metadata.mtime(),
    ))
}

/// The image each of `pids` is executing, keyed by pid.
///
/// A pid absent from the returned map is one whose image this account could
/// not read — it exited, or it belongs to another user — and the caller
/// reports that as unknown rather than dropping it. `Err` is the reader itself
/// failing, which is one cause for every pid and is reported once.
pub fn running_images(pids: &[u32]) -> Result<BTreeMap<u32, ImageIdentity>, String> {
    if pids.is_empty() {
        return Ok(BTreeMap::new());
    }
    if cfg!(target_os = "linux") {
        Ok(proc_exe_images(pids))
    } else {
        lsof_images(pids)
    }
}

/// Linux: `/proc/<pid>/exe`. `read_link` names the image and appends
/// ` (deleted)` once it has been unlinked; `metadata` follows the magic link
/// to the inode itself, so it answers for a file with no directory entry left
/// exactly as it does for one that has one.
fn proc_exe_images(pids: &[u32]) -> BTreeMap<u32, ImageIdentity> {
    use std::os::unix::fs::MetadataExt;
    const DELETED: &str = " (deleted)";
    let mut images = BTreeMap::new();
    for &pid in pids {
        let link = PathBuf::from(format!("/proc/{pid}/exe"));
        let Ok(metadata) = std::fs::metadata(&link) else {
            continue;
        };
        let path = std::fs::read_link(&link).map_or_else(
            |_| format!("/proc/{pid}/exe"),
            |target| {
                let rendered = target.to_string_lossy().into_owned();
                rendered
                    .strip_suffix(DELETED)
                    .map_or_else(|| rendered.clone(), str::to_string)
            },
        );
        images.insert(
            pid,
            ImageIdentity {
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
                bytes: metadata.size(),
                links: metadata.nlink(),
            },
        );
    }
    images
}

/// macOS: one `lsof` over every pid at once.
///
/// `-d txt` restricts the listing to text mappings and the FIRST of them is
/// the process's own executable; the rest are `dyld` and the frameworks it
/// pulled in. Some of those are routinely unlinked too — a `.plist-cache`
/// under `/Library/Preferences/Logging` is, on this machine — so "any unlinked
/// text mapping" is not the question and is not asked.
///
/// One invocation rather than one per pid: this runs inside a diagnostic that
/// already walks every unit on the host, and `host_disk` records what a
/// per-item `lsof` costs there.
fn lsof_images(pids: &[u32]) -> Result<BTreeMap<u32, ImageIdentity>, String> {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<String>>()
        .join(",");
    // A pid that has exited makes lsof exit non-zero while still reporting
    // every pid that has not, so the status is deliberately not consulted: the
    // map is the answer, and an absent pid is already the unknown.
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &list, "-d", "txt", "-F", "pDsikn"])
        .output()
        .map_err(|error| format!("/usr/sbin/lsof did not run: {error}"))?;
    let rendered = String::from_utf8_lossy(&output.stdout);
    let mut images: BTreeMap<u32, ImageIdentity> = BTreeMap::new();
    let mut pid: Option<u32> = None;
    let mut current = ImageIdentity {
        path: String::new(),
        device: 0,
        inode: 0,
        bytes: 0,
        links: 0,
    };
    for line in rendered.lines() {
        let Some((tag, value)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => pid = value.trim().parse().ok(),
            // lsof prints the device as `0x`-prefixed hex; the number is the
            // same `st_dev` `installed_image` reads, so the two sides compare
            // without a second convention.
            "D" => {
                current.device = value
                    .trim()
                    .strip_prefix("0x")
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    .or_else(|| value.trim().parse().ok())
                    .unwrap_or_default();
            }
            "s" => current.bytes = value.trim().parse().unwrap_or_default(),
            "i" => current.inode = value.trim().parse().unwrap_or_default(),
            "k" => current.links = value.trim().parse().unwrap_or_default(),
            "n" => {
                current.path = value.to_string();
                if let Some(pid) = pid {
                    images.entry(pid).or_insert_with(|| current.clone());
                }
            }
            _ => {}
        }
    }
    Ok(images)
}

/// Every process this account can see: pid, how long it has been alive, and
/// the argument vector, which is what joins a process back to the unit that
/// declares it.
///
/// `etime` and not `etimes`: macOS `ps` has no `etimes` keyword at all, and
/// asking for one makes it reject the entire format string and print an
/// unlabelled table instead of failing.
pub fn process_table() -> Result<Vec<(u32, Option<i64>, String)>, String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-axo", "pid=,etime=,args="])
        .output()
        .map_err(|error| format!("/bin/ps did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "/bin/ps exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let rendered = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in rendered.lines() {
        let Some((pid, rest)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let Some((elapsed, argv)) = rest.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        rows.push((pid, parse_etime(elapsed), argv.trim().to_string()));
    }
    Ok(rows)
}

/// `ps` elapsed time — `[[dd-]hh:]mm:ss` — in seconds.
fn parse_etime(elapsed: &str) -> Option<i64> {
    let (days, clock) = elapsed
        .split_once('-')
        .map_or((0i64, elapsed), |(days, clock)| {
            (days.trim().parse().unwrap_or_default(), clock)
        });
    let mut seconds = 0i64;
    for field in clock.split(':') {
        seconds = seconds * 60 + field.trim().parse::<i64>().ok()?;
    }
    Some(days * 86_400 + seconds)
}
