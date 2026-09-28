//! The native units the transaction stops and restores: the exact bytes and
//! ownership of a unit file, putting those bytes back through `sudo -n
//! install`, and proof that a stopped writer's listener is closed.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::process::{Command, Stdio};

use base64::Engine;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{expand_home, home};

/// The permission bits `stat.S_IMODE` keeps.
const MODE_BITS: u32 = 0o7777;
const PRIVATE_DIRECTORY: u32 = 0o700;
const PRIVATE_FILE: u32 = 0o600;

/// Prove nothing accepts connections on the loopback port. The caller has
/// already seen the writer's process and unit gone, so a port that still
/// accepts is held by something else: the probe connection is closed at once
/// and the step refuses, naming the port, instead of waiting on a peer.
pub(super) fn listener_closed(port: u16) -> Result<(), String> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    if let Ok(connection) = TcpStream::connect(address) {
        drop(connection);
        return Err(format!(
            "port {port} still accepts connections after its writer stopped"
        ));
    }
    println!("STADO_LISTENER_CLOSED\t{port}");
    Ok(())
}

/// The unit file's exact bytes, digest, mode and owner, or `absent`.
pub(super) fn unit_snapshot(path: &str) -> Result<(), String> {
    let path = expand_home(path)?;
    let info = match fs::symlink_metadata(&path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("STADO_UNIT_SNAPSHOT\tabsent");
            return Ok(());
        }
        Err(error) => return Err(format!("cannot inspect {path}: {error}")),
    };
    if !info.file_type().is_file() {
        return Err("unit path is not a regular non-symlink file".to_string());
    }
    let body = fs::read(&path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let snapshot = json!({
        "body_base64": base64::engine::general_purpose::STANDARD.encode(&body),
        "gid": info.gid(),
        "mode": info.mode() & MODE_BITS,
        "sha256": hex::encode(Sha256::digest(&body)),
        "uid": info.uid(),
    });
    println!("STADO_UNIT_SNAPSHOT\t{snapshot}");
    Ok(())
}

fn sudo_install(staged: &str, path: &str, mode: u32, uid: u32, gid: u32) -> Result<(), String> {
    let output = Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/install", "-m", &format!("{mode:o}")])
        .args(["-o", &uid.to_string(), "-g", &gid.to_string(), staged, path])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run sudo install: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Err(if stderr.is_empty() { stdout } else { stderr })
}

/// Put captured unit bytes back at `path` with their mode and owner, and
/// prove the file now holds exactly them. The bytes arrive base64-encoded
/// in `STADO_UNIT_BODY`.
pub(super) fn restore_unit(
    path: &str,
    sha256: &str,
    mode: u32,
    uid: u32,
    gid: u32,
) -> Result<(), String> {
    let path = expand_home(path)?;
    let encoded = std::env::var("STADO_UNIT_BODY")
        .map_err(|_| "STADO_UNIT_BODY carries no captured unit bytes".to_string())?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| format!("captured unit bytes are not base64: {error}"))?;
    if hex::encode(Sha256::digest(&body)) != sha256 {
        return Err("captured unit bytes fail their digest".to_string());
    }
    let work = format!("{}/.stado/work/storage-root-reconcile-units", home()?);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIRECTORY)
        .create(&work)
        .map_err(|error| format!("cannot create {work}: {error}"))?;
    let staged = format!("{work}/unit.{}", uuid::Uuid::new_v4().simple());
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE)
        .open(&staged)
        .and_then(|mut file| file.write_all(&body).and_then(|()| file.sync_all()))
        .map_err(|error| format!("cannot stage unit bytes: {error}"));
    let installed = written.and_then(|()| sudo_install(&staged, &path, mode, uid, gid));
    let removed = fs::remove_file(&staged);
    installed?;
    if let Err(error) = removed {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("cannot remove staged unit bytes {staged}: {error}"));
        }
    }
    let info = fs::symlink_metadata(&path)
        .map_err(|error| format!("cannot inspect restored unit {path}: {error}"))?;
    if !info.file_type().is_file() {
        return Err("restored unit is not a regular file".to_string());
    }
    let observed = (info.mode() & MODE_BITS, info.uid(), info.gid());
    if observed != (mode, uid, gid) {
        return Err(format!(
            "restored unit mode/uid/gid mismatch: expected {:?}, observed {observed:?}",
            (mode, uid, gid)
        ));
    }
    let restored = fs::read(&path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if hex::encode(Sha256::digest(&restored)) != sha256 {
        return Err("restored unit digest mismatch".to_string());
    }
    println!("STADO_UNIT_RESTORED\t{sha256}");
    Ok(())
}
