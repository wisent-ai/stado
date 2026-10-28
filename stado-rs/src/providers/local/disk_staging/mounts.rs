//! Which mounts may host staging: the RAM filesystem probe on `/tmp` that arms the
//! redirect, the /proc/mounts filter and its live reader, the write probe the
//! agent user has to pass, and the root-only traversal repair that recovers a
//! candidate whose parents are not world-traversable.
//!
//! A candidate is a mount whose source is a block device, identified from
//! its metadata rather than a filesystem-name classifier. This establishes
//! block backing, not physical media: the kernel's queue supplies the medium
//! separately, and an unreadable medium remains explicitly unknown.

use std::os::unix::fs::FileTypeExt;

use nix::sys::stat::Mode;

use super::*;

/// The kernel-reported RAM filesystem at `/tmp`, or no redirection trigger.
/// A failed probe is not evidence that `/tmp` is disk-backed.
pub async fn tmp_ram_filesystem() -> Result<Option<&'static str>, String> {
    let out = crate::wait::output_async(
        tokio::process::Command::new("stat").args(["-f", "-c", "%T", "/tmp"]),
    )
    .await
    .map_err(|error| format!("the filesystem of /tmp cannot be read (stat -f): {error}"))?;
    if !out.status.success() {
        return Err(format!(
            "the filesystem of /tmp cannot be read: stat -f exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let filesystem = std::str::from_utf8(&out.stdout)
        .map_err(|error| {
            format!("stat -f returned an unreadable filesystem type for /tmp: {error}")
        })?
        .trim();
    if filesystem.is_empty() {
        return Err("stat -f returned no filesystem type for /tmp".to_string());
    }
    Ok(match filesystem {
        "tmpfs" => Some("tmpfs"),
        "ramfs" => Some("ramfs"),
        _ => None,
    })
}

/// The kernel escapes whitespace and backslashes in mount-table fields.
/// Decode each escape once, so a literal backslash followed by digits is
/// never interpreted again as an escaped space.
pub(super) fn decode_mount_field(field: &str) -> Result<std::borrow::Cow<'_, str>, String> {
    if !field.contains('\\') {
        return Ok(std::borrow::Cow::Borrowed(field));
    }
    let mut decoded = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let escaped = match (chars.next(), chars.next(), chars.next()) {
            (Some('0'), Some('4'), Some('0')) => ' ',
            (Some('0'), Some('4'), Some('3')) => '#',
            (Some('0'), Some('1'), Some('1')) => '\t',
            (Some('0'), Some('1'), Some('2')) => '\n',
            (Some('1'), Some('3'), Some('4')) => '\\',
            escape => {
                return Err(format!(
                    "mount field {field:?} carries an invalid escape {escape:?}"
                ))
            }
        };
        decoded.push(escaped);
    }
    Ok(std::borrow::Cow::Owned(decoded))
}

/// Whether `source` names a block device on this host. Kernel sources such
/// as `tmpfs` are not paths; an unreadable device path is not a non-device.
fn is_block_device(source: &str) -> Result<bool, String> {
    if !Path::new(source).is_absolute() {
        return Ok(false);
    }
    std::fs::metadata(source)
        .map(|metadata| metadata.file_type().is_block_device())
        .map_err(|error| format!("mount source {source} cannot be inspected: {error}"))
}

/// Read-write mount points outside `/boot` whose source `backed` identifies
/// as a block device. An unreadable source is skipped with its own cause.
pub fn parse_mounts(
    text: &str,
    backed: impl Fn(&str) -> Result<bool, String>,
    log_fn: &mut dyn FnMut(&str),
) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut fields = line.split([' ', '\t']).filter(|field| !field.is_empty());
        let (Some(source), Some(mnt), Some(_fstype), Some(opts)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            log_fn(&format!(
                "staging: malformed /proc/mounts row {line:?}; skipping"
            ));
            continue;
        };
        if opts.split(',').any(|opt| opt == "ro") {
            continue;
        }
        let (source, mnt) = match decode_mount_field(source)
            .and_then(|source| decode_mount_field(mnt).map(|mnt| (source, mnt)))
        {
            Ok(fields) => fields,
            Err(cause) => {
                log_fn(&format!("staging: {cause}; skipping"));
                continue;
            }
        };
        if Path::new(mnt.as_ref()).starts_with("/boot") {
            continue;
        }
        match backed(&source) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(cause) => {
                log_fn(&format!("staging: candidate {mnt}: {cause}; skipping"));
                continue;
            }
        }
        out.push(mnt.into_owned());
    }
    out
}

/// Disk-backed, read-write mount points (from /proc/mounts); a table that
/// cannot be read is the error, not a host with no disk mounts.
pub fn candidate_mounts(log_fn: &mut dyn FnMut(&str)) -> Result<Vec<String>, String> {
    std::fs::read_to_string("/proc/mounts")
        .map(|text| parse_mounts(&text, is_block_device, log_fn))
        .map_err(|error| format!("/proc/mounts cannot be read: {error}"))
}

/// Whether this user can create and remove a file in `path`, or the
/// operating system's reason it cannot.
pub fn writable_for_self(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be inspected: {error}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    let mut probe =
        tempfile::NamedTempFile::with_prefix_in(".wc_staging_probe_", path).map_err(|error| {
            format!(
                "a probe file in {} cannot be created: {error}",
                path.display()
            )
        })?;
    use std::io::Write;
    let written = probe.write_all(b"x").map_err(|error| {
        format!(
            "a probe file in {} cannot be written: {error}",
            path.display()
        )
    });
    let removed = probe.close().map_err(|error| {
        format!(
            "a probe file in {} cannot be removed: {error}",
            path.display()
        )
    });
    match (written, removed) {
        (Err(mut cause), Err(removal)) => {
            cause.push_str("; ");
            cause.push_str(&removal);
            Err(cause)
        }
        (Err(cause), _) | (_, Err(cause)) => Err(cause),
        (Ok(()), Ok(())) => Ok(()),
    }
}

/// Python `_add_other_exec`: chmod o+x, then verify the bit stuck; why not,
/// when it did not.
fn add_other_exec(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let search = crate::primitives::file_mode::widen(Mode::S_IXOTH.bits());
    let mode = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be read: {error}", path.display()))?
        .permissions()
        .mode();
    if mode & search != search {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | search)).map_err(
            |error| {
                format!(
                    "{} cannot be made searchable by others: {error}",
                    path.display()
                )
            },
        )?;
    }
    let after = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be read back: {error}", path.display()))?
        .permissions()
        .mode();
    if after & search != search {
        return Err(format!("{} kept its mode without o+x", path.display()));
    }
    Ok(())
}

/// Python `_try_repair_traversal`: o+x every parent of `target` (root only).
pub(super) fn try_repair_traversal(target: &Path, log_fn: &mut dyn FnMut(&str)) {
    let Some(parent) = target.parent() else {
        return;
    };
    for p in parent.ancestors() {
        if p.as_os_str().is_empty() || p == Path::new("/") {
            break;
        }
        if let Err(cause) = add_other_exec(p) {
            log_fn(&format!(
                "staging: {cause}; an administrator should add o+x"
            ));
        }
    }
}
