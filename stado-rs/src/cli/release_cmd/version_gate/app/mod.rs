//! `stado release version-gate app-surface|app-baseline|app-check`: one
//! version gate for every application that ships a bundle, instead of a
//! reader, a baseline recovery and a rule port copied into each app's
//! repository. The app names its surface sources; `app-check` is the quality
//! step its `.wisent-release.json` declares.

mod baseline;
mod cargo;
mod check;
mod javascript;
mod python;
mod surface;
mod tuist;

use std::path::PathBuf;

use clap::Args;

use crate::cli::CmdError;

/// The sources an application's surface is read from, repository-relative.
#[derive(Args, Clone)]
pub struct AppSources {
    /// The bundle's Info.plist: `bundle-id:` and every `url-scheme:`; its
    /// CFBundleShortVersionString is the declared version
    #[arg(long, required_unless_present_any = ["package_json", "tuist_project", "cargo_toml", "pyproject", "setup_py"])]
    pub info_plist: Option<String>,
    /// A package.json: `package:`, every `export:` key and every `bin:`
    /// command; its `version` is the declared version when no Info.plist is named
    #[arg(long)]
    pub package_json: Option<String>,
    /// A condition set the package's consumers resolve `exports` with, as a
    /// comma-separated set of condition names (for example
    /// `import,node,default`); `exports` keys are tried in their written
    /// order and the first one in the set decides, as in Node. Repeat once
    /// per environment. Required when the package.json declares `exports`
    #[arg(long = "export-conditions", requires = "package_json")]
    pub export_conditions: Vec<String>,
    /// A Package.swift whose executable products are `product:` names
    #[arg(long)]
    pub products: Option<String>,
    /// A Swift file whose `.appending(path: "...")` literals are
    /// `harness-path:` names; repeatable
    #[arg(long = "appended-paths")]
    pub appended_paths: Vec<String>,
    /// A Tuist Project.swift: every shipping target's bundle identifiers, URL
    /// schemes, localizations, quick actions, extension points and
    /// entitlements; its one `.marketingVersion("...")` is the declared
    /// version when no Info.plist is named
    #[arg(long)]
    pub tuist_project: Option<String>,
    /// A Swift file declaring `static let <member>: Target = .target(...)`
    /// for a `.member` entry of the project's `targets:`; repeatable
    #[arg(long = "tuist-helper", requires = "tuist_project")]
    pub tuist_helpers: Vec<String>,
    /// The app is sold on the App Store, and this workflow writes its
    /// `appstore/<version>(<build>)` tags once App Store Connect reports a
    /// version for sale: the baseline is the newest such tag, and app-check
    /// also compares against the version the App Store serves
    #[arg(long)]
    pub app_store_tags: Option<String>,
    /// A Rust binary's advertised command table, `FILE:TABLE` for
    /// `static TABLE: &[...] = &[ ... ];`: every `name: "..."` row is a
    /// `command:` name
    #[arg(long, requires = "cargo_toml")]
    pub command_table: Option<String>,
    /// A Cargo.toml whose `[package]` version is the declared version when
    /// no Info.plist or Tuist project is named
    #[arg(long)]
    pub cargo_toml: Option<String>,
    /// Read the named exports of every module the package.json's `exports`
    /// maps a subpath to as `api:` names
    #[arg(long, requires = "package_json")]
    pub js_exports: bool,
    /// A JavaScript file whose `USAGE` literal lists the CLI's commands under
    /// `commands:`: each is a `cmd:` name
    #[arg(long)]
    pub usage_commands: Option<String>,
    /// A JavaScript file and the array in it that lists the CLI's commands,
    /// `FILE:ARRAY`: each `name:` string inside the array is a `cmd:` name.
    /// For a CLI whose usage text is built from that array
    #[arg(long)]
    pub command_array: Option<String>,
    /// A JavaScript file whose `TOOLS` array lists an MCP server's tools:
    /// each `name:` string is an `mcp:` name
    #[arg(long)]
    pub mcp_tools: Option<String>,
    /// A pyproject.toml: every `[project.scripts]` key is a
    /// `console-script:` name; its `[project]` version is the declared
    /// version when nothing above declares one
    #[arg(long)]
    pub pyproject: Option<String>,
    /// A setup.py whose literal `version="..."` is the declared version
    #[arg(long, conflicts_with = "pyproject")]
    pub setup_py: Option<String>,
    /// A Python module whose `__all__` entries are `api:` names; repeatable
    #[arg(long = "python-all")]
    pub python_all: Vec<String>,
    /// A Python module whose argparse subparsers registered with `help=`
    /// are `cli:` names; repeatable
    #[arg(long = "python-argparse")]
    pub python_argparse: Vec<String>,
    /// A Python module or directory and the decorator its package registers
    /// functions with, `PATH:DECORATOR`: every module-level function carrying
    /// `@DECORATOR` is a `<DECORATOR>:<function>` name; repeatable
    #[arg(long = "python-registry")]
    pub python_registry: Vec<String>,
    /// A directory of Python modules whose constants ending with a
    /// --manifest-suffix assign dict or list literals: each string key is
    /// `<family>:<key>`, the family being the first directory below it
    #[arg(long = "python-manifests")]
    pub python_manifests: Option<String>,
    /// A constant-name suffix --python-manifests reads; repeatable
    #[arg(long = "manifest-suffix", requires = "python_manifests")]
    pub manifest_suffixes: Vec<String>,
}

impl AppSources {
    /// The file the declared version is read from.
    pub fn version_source(&self) -> &str {
        self.info_plist
            .as_deref()
            .or(self.tuist_project.as_deref())
            .or(self.cargo_toml.as_deref())
            .or(self.pyproject.as_deref())
            .or(self.setup_py.as_deref())
            .or(self.package_json.as_deref())
            .expect("clap requires an Info.plist, a Tuist project, a Cargo.toml, a pyproject.toml, a setup.py or a package.json")
    }
}

#[derive(Args)]
pub struct AppTree {
    /// The tree to read: a checkout, or the release build's source directory
    #[arg(long, default_value = ".")]
    pub root: PathBuf,
    #[command(flatten)]
    pub sources: AppSources,
}

fn pretty(document: &serde_json::Value) -> String {
    serde_json::to_string_pretty(document).expect("a JSON value serialises")
}

/// Print `{"version": "...", "surface": [...]}` of the tree: the version it
/// declares and the surface it offers. A source the app names that cannot be
/// read as its surface is the app's declaration to fix, so it is `config`.
pub(super) fn surface(tree: AppTree) -> Result<(), CmdError> {
    let load = surface::tree(&tree.root);
    let version =
        surface::declared_version(&load, &tree.sources).map_err(CmdError::declaration)?;
    let names = surface::of(&load, &tree.sources).map_err(CmdError::declaration)?;
    println!(
        "{}",
        pretty(&serde_json::json!({ "version": version, "surface": names }))
    );
    Ok(())
}

/// Write `released-surface.json` from the best reachable artifact, or print it.
pub(super) fn baseline(tree: AppTree, stdout: bool) -> Result<(), CmdError> {
    let document = baseline::build(&tree.root, &tree.sources).map_err(CmdError::click)?;
    let rendered = format!("{}\n", pretty(&document));
    if stdout {
        print!("{rendered}");
        return Ok(());
    }
    let path = tree.root.join("released-surface.json");
    std::fs::write(&path, rendered).map_err(|error| {
        CmdError::click(format!("{}: {error}", path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    println!("wrote {}", path.display());
    Ok(())
}

/// The whole gate. Every failure it answers is the gate refusing the tree it
/// was given (a version, surface, baseline or provenance that does not hold),
/// so it is `refused`: changing the tree, not retrying, is what helps.
pub(super) fn check(tree: AppTree) -> Result<(), CmdError> {
    check::check(&tree.root, &tree.sources).map_err(CmdError::refused)
}
