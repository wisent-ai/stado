//! The compiler cache every Cargo build Stado runs goes through: Kache, at
//! the version `stado-rs/data/work/compiler-cache.json` declares.
//!
//! Each Stado build compiles into a target directory of its own (an install
//! run, a release job's scratch tree), so without a cache every build of a
//! product compiled every dependency again. Kache keys each `rustc`
//! invocation by the content of its inputs and restores the output from its
//! local store, so a crate built once on a host is restored in every later
//! build there, whichever target directory asks.
//!
//! Stado runs Kache as Cargo's `RUSTC_WRAPPER` on the commands it starts
//! itself and touches no Cargo configuration of the account. A host without
//! the declared version gets it from `cargo install` on its first build; a
//! build whose cache cannot be installed fails at that step, naming it,
//! rather than silently compiling without it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::{capture, emit, toolchain_command};

/// The declaration's path in the Stado repository, for messages.
pub const DECLARATION_PATH: &str = "stado-rs/data/work/compiler-cache.json";
const DECLARATION: &str = include_str!("../../data/work/compiler-cache.json");
const SCHEMA: &str = "stado.compiler-cache.v1";

/// The compiler cache Stado's Cargo builds use.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub schema: String,
    /// The executable `cargo install` places, also the name on `PATH`.
    pub tool: String,
    /// The crates.io crate that provides it.
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// The exact version every host runs.
    pub version: String,
    pub repository: String,
    /// Why this tool and this version.
    pub source: String,
}

/// The declared compiler cache, refused when the document is not the shape
/// this build reads.
pub fn declaration() -> Result<Declaration> {
    let declaration: Declaration = serde_json::from_str(DECLARATION)
        .with_context(|| format!("{DECLARATION_PATH} is not a compiler-cache declaration"))?;
    if declaration.schema != SCHEMA {
        bail!(
            "{DECLARATION_PATH} declares schema {}; this Stado reads {SCHEMA}",
            declaration.schema
        );
    }
    Ok(declaration)
}

/// The compiler cache a build runs through: the executable and the version
/// it reported.
#[derive(Debug, Clone)]
pub struct Wrapper {
    pub path: PathBuf,
    pub version: String,
}

impl Wrapper {
    pub fn report(&self, declaration: &Declaration) -> Value {
        json!({
            "tool": declaration.tool,
            "path": self.path,
            "version": self.version,
            "declared_version": declaration.version,
            "declared_by": DECLARATION_PATH,
        })
    }
}

/// Where `cargo install` places executables for this account:
/// `$CARGO_INSTALL_ROOT/bin`, else `$CARGO_HOME/bin`, else `~/.cargo/bin`.
pub fn install_directory(home: &Path) -> PathBuf {
    let set = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    match (set("CARGO_INSTALL_ROOT"), set("CARGO_HOME")) {
        (Some(root), _) => PathBuf::from(root).join("bin"),
        (None, Some(cargo_home)) => PathBuf::from(cargo_home).join("bin"),
        (None, None) => home.join(".cargo/bin"),
    }
}

/// The tool as `cargo install` placed it for this account. A Kache from
/// another source on `PATH` is not the declared one and is never used.
fn find(home: &Path, tool: &str) -> Option<PathBuf> {
    Some(install_directory(home).join(tool)).filter(|installed| installed.is_file())
}

/// The version an executable reports: the last word of `<tool> --version`.
fn version_of(path: &Path) -> Result<String> {
    let output = Command::new(path)
        .arg("--version")
        .output()
        .with_context(|| format!("cannot run {} --version", path.display()))?;
    if !output.status.success() {
        bail!(
            "{} --version exited {}: {}",
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .last()
        .map(str::to_owned)
        .with_context(|| format!("{} --version printed nothing", path.display()))
}

/// What is installed against what is declared, without changing anything.
pub fn status(home: &Path) -> Result<Value> {
    let declaration = declaration()?;
    let found = find(home, &declaration.tool);
    let installed = found.as_deref().map(version_of).transpose();
    let (version, error) = match installed {
        Ok(version) => (version, None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let state = match (&found, &version) {
        (None, _) => "absent",
        (Some(_), Some(version)) if *version == declaration.version => "ready",
        (Some(_), Some(_)) => "other-version",
        (Some(_), None) => "unreadable",
    };
    Ok(json!({
        "tool": declaration.tool,
        "crate": declaration.crate_name,
        "declared_version": declaration.version,
        "declared_by": DECLARATION_PATH,
        "repository": declaration.repository,
        "install_directory": install_directory(home),
        "path": found,
        "installed_version": version,
        "state": state,
        "error": error,
    }))
}

/// The declared compiler cache, installed with `cargo install --locked
/// --version <declared>` when it is absent or another version, and read
/// back afterwards: a build only runs through a Kache that answered with the
/// declared version.
pub fn ensure(home: &Path) -> Result<Wrapper> {
    let declaration = declaration()?;
    let installed = install_directory(home).join(&declaration.tool);
    if installed.is_file() {
        if let Ok(version) = version_of(&installed) {
            if version == declaration.version {
                return Ok(Wrapper {
                    path: installed,
                    version,
                });
            }
        }
    }
    let mut install = toolchain_command("cargo");
    install.args([
        "install",
        "--locked",
        "--version",
        &declaration.version,
        &declaration.crate_name,
    ]);
    let output = capture(&mut install).with_context(|| {
        format!(
            "cannot run cargo to install the compiler cache {} {} ({DECLARATION_PATH})",
            declaration.crate_name, declaration.version
        )
    })?;
    if !output.status.success() {
        bail!(
            "cargo install --locked --version {} {} failed ({}), so no build runs through the compiler cache {DECLARATION_PATH} declares: {}",
            declaration.version,
            declaration.crate_name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if !installed.is_file() {
        bail!(
            "cargo install {} {} succeeded but {} does not exist; CARGO_INSTALL_ROOT or CARGO_HOME placed it elsewhere",
            declaration.crate_name,
            declaration.version,
            installed.display()
        );
    }
    let version = version_of(&installed)?;
    if version != declaration.version {
        bail!(
            "{} reports version {version} after cargo install of {}; {DECLARATION_PATH} declares {}",
            installed.display(),
            declaration.version,
            declaration.version
        );
    }
    Ok(Wrapper {
        path: installed,
        version,
    })
}

/// Take back `ensure`: `cargo uninstall` of the declared crate.
pub fn remove(home: &Path) -> Result<Value> {
    let declaration = declaration()?;
    let installed = install_directory(home).join(&declaration.tool);
    if !installed.is_file() {
        bail!(
            "{} is not installed at {}; nothing to remove",
            declaration.tool,
            installed.display()
        );
    }
    let output = capture(toolchain_command("cargo").args(["uninstall", &declaration.crate_name]))?;
    if !output.status.success() {
        bail!(
            "cargo uninstall {} failed ({}): {}",
            declaration.crate_name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(json!({ "tool": declaration.tool, "path": installed, "state": "absent" }))
}

/// The shell lines a remote build script runs before Cargo: install the
/// declared version with `"$cargo"` when it is absent or another version,
/// then export it as `RUSTC_WRAPPER`. The script must have set `cargo`.
pub fn shell_ensure() -> Result<String> {
    let declaration = declaration()?;
    let tool = &declaration.tool;
    let version = &declaration.version;
    let krate = &declaration.crate_name;
    Ok(format!(
        "kache_bin=\"${{CARGO_INSTALL_ROOT:-${{CARGO_HOME:-$HOME/.cargo}}}}/bin/{tool}\"\n\
         case \"$(\"$kache_bin\" --version 2>/dev/null)\" in\n\
         *\" {version}\") ;;\n\
         *) \"$cargo\" install --locked --version {version} {krate} || {{ printf '%s\\n' 'cargo install --locked --version {version} {krate} failed, so no build runs through the compiler cache {DECLARATION_PATH} declares' >&2; exit 69; }} ;;\n\
         esac\n\
         export RUSTC_WRAPPER=\"$kache_bin\"\n"
    ))
}

/// `stado product compiler-cache status|ensure|remove [--json]`.
pub fn run(operation: &str, json_output: bool, home: &Path) -> Result<i32> {
    let report = match operation {
        "status" => status(home)?,
        "ensure" => {
            let declaration = declaration()?;
            let wrapper = ensure(home)?;
            let mut report = wrapper.report(&declaration);
            report["state"] = json!("ready");
            report
        }
        "remove" => remove(home)?,
        other => bail!("unknown compiler-cache operation {other}; use status, ensure or remove"),
    };
    if json_output {
        emit(&report)?;
    } else {
        let field = |name: &str| match &report[name] {
            Value::String(text) => text.clone(),
            Value::Null => "-".to_owned(),
            other => other.to_string(),
        };
        println!(
            "{} {} at {} (declared {})",
            field("tool"),
            field("state"),
            field("path"),
            field("declared_version")
        );
        if let Value::String(error) = &report["error"] {
            eprintln!("{error}");
        }
    }
    Ok(if report["state"] == "ready" || operation == "remove" {
        0
    } else {
        1
    })
}
