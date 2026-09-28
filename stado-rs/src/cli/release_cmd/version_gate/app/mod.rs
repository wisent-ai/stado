//! `stado release version-gate app-surface|app-baseline|app-check`: one
//! version gate for every application that ships a bundle, instead of a
//! reader, a baseline recovery and a rule port copied into each app's
//! repository. The app names its surface sources; `app-check` is the quality
//! step its `.wisent-release.json` declares.

mod baseline;
mod cargo;
mod check;
mod javascript;
mod store;
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
    #[arg(long, required_unless_present_any = ["package_json", "tuist_project", "cargo_toml"])]
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
    /// A JavaScript file whose `TOOLS` array lists an MCP server's tools:
    /// each `name:` string is an `mcp:` name
    #[arg(long)]
    pub mcp_tools: Option<String>,
}

impl AppSources {
    /// The file the declared version is read from.
    pub fn version_source(&self) -> &str {
        self.info_plist
            .as_deref()
            .or(self.tuist_project.as_deref())
            .or(self.cargo_toml.as_deref())
            .or(self.package_json.as_deref())
            .expect("clap requires an Info.plist, a Tuist project, a Cargo.toml or a package.json")
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

/// Print `{"surface": [...]}` of the tree.
pub(super) fn surface(tree: AppTree) -> Result<(), CmdError> {
    let load = surface::tree(&tree.root);
    let names = surface::of(&load, &tree.sources).map_err(CmdError::click)?;
    println!("{}", pretty(&serde_json::json!({ "surface": names })));
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
    std::fs::write(&path, rendered)
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?;
    println!("wrote {}", path.display());
    Ok(())
}

/// The whole gate.
pub(super) fn check(tree: AppTree) -> Result<(), CmdError> {
    check::check(&tree.root, &tree.sources).map_err(CmdError::click)
}
