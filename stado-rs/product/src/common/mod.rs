mod archive;
mod files;
mod process;
pub mod runs;

use anyhow::{bail, Context, Result};
pub use archive::{copy_tree, file_members, platform, relative, unpack};
pub use files::{
    atomic_json, atomic_write, lock, lock_superseding, lock_waiting, mark_placing, sha256,
};
pub use process::{
    capture, checked, step_program, step_search_path, toolchain_command, CommandFailed,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    env,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

/// The authoritative catalog, relative to the workspace of canonical checkouts:
/// the Stado repository's own `catalog/products.yml`.
pub const CATALOG: &str = "stado/catalog/products.yml";

/// The workspace `--workspace` named for this process, which every runtime
/// the process builds uses, the command records included.
static WORKSPACE: OnceLock<PathBuf> = OnceLock::new();

/// Name the workspace of canonical checkouts this process works in. A second
/// different workspace in one process is refused: half its records would be
/// written under the first.
pub fn use_workspace(workspace: PathBuf) -> Result<()> {
    let named = WORKSPACE.get_or_init(|| workspace.clone());
    if *named != workspace {
        bail!(
            "this process already works in {}; {} cannot also be its workspace",
            named.display(),
            workspace.display()
        );
    }
    Ok(())
}

#[derive(Clone)]
pub struct Runtime {
    pub catalog: PathBuf,
    pub workspace: PathBuf,
    pub home: PathBuf,
    pub output: PathBuf,
    pub(crate) embedded_catalog: bool,
    pub(crate) checkouts: Arc<Mutex<Option<crate::source::WorkspaceIndex>>>,
    /// `--wait`: a surface writer lock held by another process is waited for
    /// instead of refused.
    pub wait_for_writer: bool,
    /// An installation or update: the source it builds, and the source its
    /// Cargo and Swift git dependencies resolve to, is cloned into the
    /// workspace when no canonical checkout holds it.
    pub create_checkouts: bool,
    /// This machine's registry target name, as `stado registry self` answers
    /// it, read once; `None` once asked and not a registry target.
    this_host: Arc<Mutex<Option<Option<String>>>>,
}

impl Runtime {
    /// Whether `host` names this machine's registry target. A machine the
    /// registry does not know is no host at all, so every host is another.
    pub fn is_this_host(&self, host: &str) -> bool {
        let mut cached = self.this_host.lock().expect("host identity lock");
        let answer = cached.get_or_insert_with(|| {
            capture(stado().args(["registry", "self"]))
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| {
                    String::from_utf8(output.stdout)
                        .ok()?
                        .split_whitespace()
                        .next()
                        .map(str::to_owned)
                })
        });
        answer.as_deref() == Some(host)
    }

    pub fn new(catalog: Option<PathBuf>) -> Result<Self> {
        let home = PathBuf::from(env::var_os("HOME").context("HOME is not set")?);
        let workspace = WORKSPACE
            .get()
            .cloned()
            .or_else(|| env::var_os("WISENT_WORKSPACE").map(PathBuf::from))
            .unwrap_or_else(|| home.join("Documents/CodingProjects/Wisent"));
        // Stado's own checkout keeps evidence in its ignored `.wisent-output`.
        // A machine without that checkout keeps it under Stado's home: created
        // in the workspace, the output directory would occupy the path an
        // installation clones `wisent-ai/stado` into.
        let output = env::var_os("WISENT_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let checkout = workspace.join("stado");
                if checkout.join(".git").is_dir() {
                    checkout.join(".wisent-output")
                } else {
                    home.join(".stado/products/output")
                }
            });
        let embedded_catalog = catalog.is_none() && !workspace.join(CATALOG).is_file();
        let catalog = catalog.unwrap_or_else(|| workspace.join(CATALOG));
        Ok(Self {
            catalog,
            workspace,
            home,
            output,
            embedded_catalog,
            checkouts: Arc::new(Mutex::new(None)),
            wait_for_writer: false,
            create_checkouts: false,
            this_host: Arc::new(Mutex::new(None)),
        })
    }

    /// The exclusive writer lock of one product surface. A holder still
    /// preparing (building, nothing placed) is superseded by this process, as
    /// a newer fleet build cancels the builds it supersedes; a holder already
    /// placing files is waited for. With `--wait` the holder is kept whatever
    /// it is doing and this process runs after it.
    pub fn surface_lock(&self, path: &std::path::Path) -> Result<std::fs::File> {
        if self.wait_for_writer {
            lock_waiting(path)
        } else {
            lock_superseding(path)
        }
    }
}

/// A Stado subcommand run by this same build. Product operations call Stado's
/// release and service commands; running them from `PATH` could reach a
/// different installed version than the one executing this operation.
pub fn stado() -> std::process::Command {
    std::process::Command::new(env::current_exe().unwrap_or_else(|_| PathBuf::from("stado")))
}

/// One string field of the one item that plays `role` — the item carrying
/// `stado:role:<role>` — read through this build's `stado credentials get
/// --role`, so it comes from the store `credentials.store` selects. No item id
/// is named anywhere: renaming the item changes nothing. The value stays in
/// memory: it is never logged or passed on as an argument.
pub fn credential_field(role: &str, field: &str) -> Result<String> {
    let output = stado()
        .args(["credentials", "get", "--role", role, "--field", field])
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| {
            format!("stado credentials get --role {role} --field {field} could not be started")
        })?;
    if !output.status.success() {
        bail!(
            "stado credentials get --role {role} --field {field} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let value = String::from_utf8(output.stdout)?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    if value.is_empty() {
        bail!("credential {role}#{field} is empty");
    }
    Ok(value)
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn emit(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn xml(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.contains(['&', '<', '>', '"', '\'']) {
        return std::borrow::Cow::Borrowed(value);
    }
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&apos;"),
            other => output.push(other),
        }
    }
    std::borrow::Cow::Owned(output)
}

pub fn absolute(path: &std::path::Path) -> Result<PathBuf> {
    use std::path::Component;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

pub fn slug(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
    {
        bail!(
            "invalid identifier {value:?}: expected lowercase letters, digits and internal hyphens"
        );
    }
    Ok(())
}

pub struct Arguments {
    pub positional: Vec<String>,
    values: BTreeMap<String, Vec<String>>,
}

impl Arguments {
    pub fn from_matches(mut matches: clap::ArgMatches) -> Self {
        let mut parsed = Self {
            positional: Vec::new(),
            values: BTreeMap::new(),
        };
        let identifiers: Vec<_> = matches.ids().cloned().collect();
        for identifier in identifiers {
            let name = identifier.as_str();
            if matches
                .try_get_one::<bool>(name)
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false)
            {
                parsed.values.insert(format!("--{name}"), Vec::new());
            } else if let Ok(Some(values)) = matches.try_remove_many::<String>(name) {
                let values = values.collect();
                if name == "positional" {
                    parsed.positional = values;
                } else {
                    parsed.values.insert(format!("--{name}"), values);
                }
            }
        }
        parsed
    }

    pub fn has(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
    pub fn many(&self, key: &str) -> &[String] {
        self.values.get(key).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn optional(&self, key: &str) -> Result<Option<&str>> {
        let values = self.many(key);
        if values.len() > 1 {
            bail!("{key} may only be supplied once");
        }
        Ok(values.first().map(String::as_str))
    }
    pub fn required(&self, key: &str) -> Result<&str> {
        self.optional(key)?
            .with_context(|| format!("{key} is required"))
    }
}
