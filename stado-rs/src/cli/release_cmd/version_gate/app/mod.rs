//! `stado release version-gate app-surface|app-baseline|app-check`: one
//! version gate for every application that ships a bundle, instead of a
//! reader, a baseline recovery and a rule port copied into each app's
//! repository. The app names its surface sources; `app-check` is the quality
//! step its `.wisent-release.json` declares.

mod baseline;
mod check;
mod surface;

use std::path::PathBuf;

use clap::Args;

use crate::cli::CmdError;

/// The sources an application's surface is read from, repository-relative.
#[derive(Args, Clone)]
pub struct AppSources {
    /// The bundle's Info.plist: `bundle-id:` and every `url-scheme:`; its
    /// CFBundleShortVersionString is the declared version
    #[arg(long, required_unless_present = "package_json")]
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
}

impl AppSources {
    /// The file the declared version is read from.
    pub fn version_source(&self) -> &str {
        self.info_plist
            .as_deref()
            .or(self.package_json.as_deref())
            .expect("clap requires an Info.plist or a package.json")
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
