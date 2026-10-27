//! `stado host local repair --product P`: return one release
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
    CmdError::refused(detail)
}

/// A non-empty environment value, `None` when the variable is unset or
/// empty, and a refusal naming it when it is set to something not UTF-8.
fn env(name: &str) -> Result<Option<String>, CmdError> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(refuse(format!("{name} cannot be read: {error}"))),
    }
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
    // A document without a `storage` block declares no store path; the
    // pointers below then find nothing and each refusal names where it looked.
    let storage = document.get("storage");
    let declared = |path: &str| {
        storage
            .and_then(|storage| storage.pointer(path))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let primary = match env("WC_LOCAL_STORAGE_PATH")? {
        Some(path) => path,
        None => declared("/local/path").ok_or_else(|| {
            refuse(format!(
                "store_root unresolved: neither WC_LOCAL_STORAGE_PATH nor storage.local.path in {} \
                 names it; nothing repaired",
                config.display()
            ))
        })?,
    };
    let mut paths = vec![primary];
    let backend = match env("WC_BACKUP_STORAGE_BACKEND")? {
        Some(backend) => Some(backend),
        None => declared("/backup/backend"),
    };
    if backend.as_deref() == Some("local") {
        let backup = match env("WC_BACKUP_LOCAL_STORAGE_PATH")? {
            Some(path) => path,
            None => declared("/backup/local/path").ok_or_else(|| {
                refuse(format!(
                    "backup_store_root unresolved: the backup backend is local and neither \
                     WC_BACKUP_LOCAL_STORAGE_PATH nor storage.backup.local.path in {} names it; \
                     nothing repaired",
                    config.display()
                ))
            })?,
        };
        paths.push(backup);
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
    if !nix::unistd::Uid::from_raw(observed.uid()).is_root() && observed.uid() != uid {
        return Err(refuse(format!(
            "refused_foreign_owner uid={} {}",
            observed.uid(),
            path.display()
        )));
    }
    Ok(Some(observed))
}

fn name_of(command: &str, argument: &str) -> Result<String, CmdError> {
    let output = crate::wait::output(std::process::Command::new(command).arg(argument))
        .map_err(|error| refuse(format!("{command}: {error}")))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn release_store_repair_local(config: &str, product: &str) -> Result<(), CmdError> {
    if product.is_empty()
        || !product
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(refuse(format!("invalid_product {product}")));
    }
    let paths = store_paths(Path::new(config))?;
    let home = PathBuf::from(env("HOME")?.ok_or_else(|| refuse("HOME is not set".into()))?);
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
            return Err(refuse(format!(
                "store_root outside managed home: {}; nothing repaired",
                root.display()
            )));
        }
        if root.exists() {
            let canonical = std::fs::canonicalize(&root).map_err(|error| {
                refuse(format!("cannot resolve store_root {}: {error}; nothing repaired", root.display()))
            })?;
            if canonical != root {
                return Err(refuse(format!(
                    "store_root has a symlinked component: {}; nothing repaired",
                    root.display()
                )));
            }
        }
        if !root.is_dir() {
            return Err(refuse(format!(
                "store_root unresolved: {}; nothing repaired",
                root.display()
            )));
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
            // Every prefix of the relative path is a node; all but the last
            // are directories.
            let mut parts = relative.split('/').peekable();
            let mut path = root.clone();
            while let Some(part) = parts.next() {
                path = path.join(part);
                remember(&mut nodes, path.clone(), parts.peek().is_some());
            }
        }
    }
    // No mutation until the complete, bounded set in both stores is known.
    for path in &order {
        let observed = inspect(path, nodes[path], uid)?;
        let owner = match observed {
            Some(meta) => format!("uid={} gid={}", meta.uid(), meta.gid()),
            None => "absent".to_string(),
        };
        println!("observed {owner} {}", path.display());
    }
    let mut repaired: Vec<&PathBuf> = Vec::new();
    for path in &order {
        let Some(observed) = inspect(path, nodes[path], uid)? else {
            continue;
        };
        if observed.uid() == uid {
            continue;
        }
        let status = crate::wait::status(
            std::process::Command::new("/usr/bin/sudo")
                .args(["-n", "/usr/sbin/chown", "-h", &format!("{account}:{group}")])
                .arg(path),
        )
        .map_err(|error| refuse(format!("chown {}: {error}", path.display())))?;
        if !status.success() {
            return Err(refuse(format!("chown failed {}", path.display())));
        }
        println!("repaired root -> {account}:{group} {}", path.display());
        repaired.push(path);
    }
    for path in &order {
        let directory = nodes[path];
        let Some(observed) = inspect(path, directory, uid)? else {
            continue;
        };
        let access = if directory {
            nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::X_OK
        } else {
            nix::unistd::AccessFlags::W_OK | nix::unistd::AccessFlags::R_OK
        };
        if observed.uid() != uid || nix::unistd::access(path, access).is_err() {
            return Err(refuse(format!(
                "postcondition_failed owner_uid={} {}",
                observed.uid(),
                path.display()
            )));
        }
    }
    println!(
        "release_store_repaired product={product} account={account} changed={} stores={} bounded_paths={}",
        repaired.len(),
        roots.len(),
        nodes.len()
    );
    Ok(())
}
