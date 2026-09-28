//! `stado host release-store-repair-local --product P`: return one release
//! catalog coordinate in this host's local stores to the managed account.
//!
//! Every primary and backup path is validated before anything changes, the
//! store is never walked or chowned recursively, and only the bounded set of
//! object, metadata and lock paths (plus their parent directories) is
//! touched. A node owned by root is handed back with `sudo -n chown -h`; a
//! node owned by anyone else, a symbolic link or a wrong type refuses the
//! whole repair. Output lines are the same as the former host program's.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::CmdError;

fn refuse(detail: String) -> CmdError {
    CmdError::click(detail)
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// `~` expansion and lexical normalisation, as `os.path.abspath(expanduser())`.
fn absolute(raw: &str, home: &Path) -> PathBuf {
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if raw == "~" => home.to_path_buf(),
        None => PathBuf::from(raw),
    };
    let mut out = PathBuf::from("/");
    for component in expanded.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            _ => {}
        }
    }
    out
}

fn store_paths(config: &Path) -> Result<Vec<String>, CmdError> {
    let document: Value = std::fs::read_to_string(config)
        .map_err(|error| refuse(format!("{}: {error}", config.display())))
        .and_then(|text| serde_json::from_str(&text).map_err(|error| refuse(error.to_string())))?;
    let storage = document.get("storage").cloned().unwrap_or(Value::Null);
    let text = |value: Option<&Value>| value.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let primary = env("WC_LOCAL_STORAGE_PATH")
        .or_else(|| text(storage.pointer("/local/path")))
        .ok_or_else(|| refuse("store_root unresolved; nothing repaired".into()))?;
    let mut paths = vec![primary];
    let backend = env("WC_BACKUP_STORAGE_BACKEND").or_else(|| text(storage.pointer("/backup/backend")));
    if backend.as_deref() == Some("local") {
        paths.push(
            env("WC_BACKUP_LOCAL_STORAGE_PATH")
                .or_else(|| text(storage.pointer("/backup/local/path")))
                .ok_or_else(|| refuse("backup_store_root unresolved; nothing repaired".into()))?,
        );
    }
    Ok(paths)
}

/// The metadata of one bounded node, `None` when absent; refuses a link, a
/// wrong type, or an owner other than root and this account.
fn inspect(path: &Path, directory: bool, uid: u32) -> Result<Option<std::fs::Metadata>, CmdError> {
    let observed = match std::fs::symlink_metadata(path) {
        Ok(observed) => observed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(refuse(format!("{error} {}", path.display()))),
    };
    if observed.file_type().is_symlink() {
        return Err(refuse(format!("refused_symlink {}", path.display())));
    }
    if (directory && !observed.is_dir()) || (!directory && !observed.is_file()) {
        return Err(refuse(format!("refused_wrong_type {}", path.display())));
    }
    if observed.uid() != 0 && observed.uid() != uid {
        return Err(refuse(format!("refused_foreign_owner uid={} {}", observed.uid(), path.display())));
    }
    Ok(Some(observed))
}

fn name_of(command: &str, argument: &str) -> Result<String, CmdError> {
    let output = std::process::Command::new(command)
        .arg(argument)
        .output()
        .map_err(|error| refuse(format!("{command}: {error}")))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn release_store_repair_local(config: &str, product: &str) -> Result<(), CmdError> {
    if product.is_empty() || !product.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        return Err(refuse(format!("invalid_product {product}")));
    }
    let paths = store_paths(Path::new(config))?;
    let home = PathBuf::from(env("HOME").ok_or_else(|| refuse("HOME is not set".into()))?);
    let uid = nix::unistd::geteuid().as_raw();
    let account = name_of("/usr/bin/id", "-un")?;
    let group = name_of("/usr/bin/id", "-gn")?;
    let managed_home = home.join(".stado");
    let key = format!("ecosystem/system/release-catalog/{product}.json");
    let lock_name = hex::encode(Sha256::digest(key.as_bytes()));
    println!("release_catalog_uri stado://system/release-catalog/{product}.json");
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut nodes: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut order: Vec<PathBuf> = Vec::new();
    let mut remember = |nodes: &mut BTreeMap<PathBuf, bool>, path: PathBuf, directory: bool| {
        if nodes.insert(path.clone(), directory).is_none() {
            order.push(path);
        }
    };
    for raw in &paths {
        let root = absolute(raw, &home);
        if roots.contains(&root) {
            continue;
        }
        if !root.starts_with(&managed_home) || root == managed_home {
            return Err(refuse(format!("store_root outside managed home: {}; nothing repaired", root.display())));
        }
        if std::fs::canonicalize(&root).ok().as_deref() != Some(root.as_path()) && root.exists() {
            return Err(refuse(format!("store_root has a symlinked component: {}; nothing repaired", root.display())));
        }
        if !root.is_dir() {
            return Err(refuse(format!("store_root unresolved: {}; nothing repaired", root.display())));
        }
        roots.push(root.clone());
        let files = [
            ("physical_object", key.clone()),
            ("physical_metadata", format!(".metadata/{key}")),
            ("physical_lock", format!(".locks/{lock_name}")),
        ];
        for (label, relative) in files {
            println!("{label} {}", root.join(&relative).display());
            remember(&mut nodes, root.clone(), true);
            let components: Vec<&str> = relative.split('/').collect();
            for index in 1..=components.len() {
                let path = components[..index].iter().fold(root.clone(), |path, part| path.join(part));
                remember(&mut nodes, path, index < components.len());
            }
        }
    }
    // No mutation until the complete, bounded set in both stores is known.
    for path in &order {
        let observed = inspect(path, nodes[path], uid)?;
        let owner = observed.map_or("absent".to_string(), |meta| format!("uid={} gid={}", meta.uid(), meta.gid()));
        println!("observed {owner} {}", path.display());
    }
    let mut repaired = 0;
    for path in &order {
        let Some(observed) = inspect(path, nodes[path], uid)? else { continue };
        if observed.uid() == uid {
            continue;
        }
        let status = std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "/usr/sbin/chown", "-h", &format!("{account}:{group}")])
            .arg(path)
            .status()
            .map_err(|error| refuse(format!("chown {}: {error}", path.display())))?;
        if !status.success() {
            return Err(refuse(format!("chown failed {}", path.display())));
        }
        println!("repaired root -> {account}:{group} {}", path.display());
        repaired += 1;
    }
    for path in &order {
        let directory = nodes[path];
        let Some(observed) = inspect(path, directory, uid)? else { continue };
        let access = if directory { nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK } else { nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::R_OK };
        if observed.uid() != uid || nix::unistd::access(path, access).is_err() {
            return Err(refuse(format!("postcondition_failed owner_uid={} {}", observed.uid(), path.display())));
        }
    }
    println!(
        "release_store_repaired product={product} account={account} changed={repaired} stores={} bounded_paths={}",
        roots.len(),
        nodes.len()
    );
    Ok(())
}
