//! `stado release catalog adopt`: add an application checkout to the release
//! pipeline in one command.
//!
//! On 2026-09-23 the operator asked for turbot-ios and wisent-ios-repo to be
//! released like every other product. Neither had a `.wisent-release.json` or
//! release scripts, and the path to one was a manifest copied from another app
//! by hand, scripts edited by hand, `catalog declare-publisher`, then `catalog
//! sync`. This command writes the manifest and scripts from templates filled
//! with what the checkout's own project states, declares the publisher when
//! the fleet hosts are named, and registers the product, so the next app takes
//! the same path. Without `--apply` it only prints what it would write.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};

use crate::cli::CmdError;
use crate::release_pipeline::{self, PRODUCT_MANIFEST};

mod xcode;

const MANIFEST: &str = include_str!("templates/manifest.json");
const BUILD: &str = include_str!("templates/build.sh");
const QUALITY: &str = include_str!("templates/quality.sh");
const ARCHIVE_TREE: &str = include_str!("templates/archive-tree.py");

#[derive(Clone, Copy, ValueEnum)]
pub(super) enum Kind {
    /// An iOS app built from the one `*.xcodeproj` at the checkout root.
    IosXcode,
}

#[derive(Args)]
pub(super) struct AdoptArgs {
    /// The application checkout to add.
    checkout: PathBuf,
    /// What the checkout builds; it decides the scripts written.
    #[arg(long, value_enum)]
    kind: Kind,
    /// The product name; defaults to the checkout folder, refusing when its origin names another repository.
    #[arg(long)]
    product: Option<String>,
    /// The Xcode scheme to archive; defaults to the project's name.
    #[arg(long)]
    scheme: Option<String>,
    /// Registry host whose vault is authoritative; with --client, the
    /// publisher is declared as `catalog declare-publisher` declares it.
    #[arg(long, requires = "client")]
    owner: Option<String>,
    /// Registry host that runs `release submit`.
    #[arg(long, requires = "owner")]
    client: Option<String>,
    /// Further hosts that serve the release API; repeat for several.
    #[arg(long = "target")]
    targets: Vec<String>,
    /// HOST=SERVICE whose process caches the publisher table; repeat.
    #[arg(long = "reload")]
    reloads: Vec<String>,
    /// Write the files, declare the publisher and register the product.
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
}

struct Planned {
    path: PathBuf,
    text: String,
    executable: bool,
}

fn fill(template: &str, values: &[(&str, &str)]) -> String {
    values
        .iter()
        .fold(template.to_string(), |text, (key, value)| {
            text.replace(&format!("{{{{{key}}}}}"), value)
        })
}

fn plan(args: &AdoptArgs) -> Result<(PathBuf, String, Vec<Planned>), CmdError> {
    let checkout = args.checkout.canonicalize()?;
    if !checkout.join(".git").exists() {
        return Err(CmdError::click(format!(
            "{} is not a git checkout; a release builds a pushed commit",
            checkout.display()
        )));
    }
    if checkout.join(PRODUCT_MANIFEST).exists() {
        return Err(CmdError::click(format!(
            "{} already declares {PRODUCT_MANIFEST}; register it with `stado release catalog sync --root {}`",
            checkout.display(),
            checkout.display()
        )));
    }
    let product = match &args.product {
        Some(product) => product.clone(),
        None => checkout
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    // Preview can prepare a local checkout; apply must not register one that
    // cannot be pushed. An implicit product must agree with its origin's name.
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&checkout)
        .args(["remote", "get-url", "origin"])
        .output()?;
    if !output.status.success() && args.apply {
        return Err(CmdError::click(format!(
            "{} has no readable origin; add its release repository before --apply: {}",
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if output.status.success() && args.product.is_none() {
        let origin =
            String::from_utf8(output.stdout).map_err(|error| CmdError::click(error.to_string()))?;
        let repository = origin
            .trim()
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or_default();
        let repository = repository.strip_suffix(".git").unwrap_or(repository);
        if repository != product {
            return Err(CmdError::click(format!(
                "{} is named {product}, but origin {} names {repository}; pass --product {repository} \
                 for that source or use its canonical checkout",
                checkout.display(), origin.trim()
            )));
        }
    }
    let Kind::IosXcode = args.kind;
    let project = xcode::read(&checkout)?;
    let scheme = args.scheme.clone().unwrap_or_else(|| project.name.clone());
    let values = [
        ("PRODUCT", product.as_str()),
        ("PROJECT", project.name.as_str()),
        ("SCHEME", scheme.as_str()),
        ("APP", scheme.as_str()),
        ("BUNDLE_ID", project.bundle_id.as_str()),
        ("TEAM", project.team.as_str()),
    ];
    let files = vec![
        Planned {
            path: checkout.join(PRODUCT_MANIFEST),
            text: fill(MANIFEST, &values),
            executable: false,
        },
        Planned {
            path: checkout.join("release/build.sh"),
            text: fill(BUILD, &values),
            executable: true,
        },
        Planned {
            path: checkout.join("release/quality.sh"),
            text: fill(QUALITY, &values),
            executable: true,
        },
        Planned {
            path: checkout.join("release/archive-tree.py"),
            text: ARCHIVE_TREE.to_string(),
            executable: true,
        },
    ];
    if let Some(taken) = files.iter().find(|file| file.path.exists()) {
        return Err(CmdError::click(format!(
            "{} already exists; adopt writes only files the checkout does not have",
            taken.path.display()
        )));
    }
    let manifest = release_pipeline::parse_product_manifest(files[0].text.as_bytes())
        .map_err(CmdError::click)?;
    if super::product(&manifest) != product {
        return Err(CmdError::click(format!(
            "product name {product:?} is not a valid release identifier"
        )));
    }
    eprintln!(
        "{product}: {}.xcodeproj, scheme {scheme}, bundle {}, team {}, version {}",
        project.name, project.bundle_id, project.team, project.version
    );
    Ok((checkout, product, files))
}

fn write(files: &[Planned]) -> Result<(), CmdError> {
    for file in files {
        if let Some(parent) = file.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file.path, &file.text)?;
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

fn relative<'a>(checkout: &Path, path: &'a Path) -> std::borrow::Cow<'a, str> {
    path.strip_prefix(checkout)
        .unwrap_or(path)
        .to_string_lossy()
}

pub(super) async fn run(args: AdoptArgs) -> Result<(), CmdError> {
    let (checkout, product, files) = plan(&args)?;
    if !args.apply {
        for file in &files {
            println!("would write {}", relative(&checkout, &file.path));
        }
        println!("run again with --apply to write them and register {product}");
        return Ok(());
    }
    write(&files)?;
    for file in &files {
        println!("wrote {}", relative(&checkout, &file.path));
    }
    if let (Some(owner), Some(client)) = (&args.owner, &args.client) {
        let (targets, reloads) = (&args.targets, &args.reloads);
        super::publisher::declare_publisher(&product, owner, client, targets, reloads, args.json)
            .await?;
    } else {
        println!("publisher not declared: pass --owner and --client, or run");
        println!("  stado release catalog declare-publisher {product} --owner HOST --client HOST");
    }
    super::checkout::sync(&checkout, args.json).await?;
    println!(
        "next: commit and push {PRODUCT_MANIFEST} and release/, store the provisioning profile as \
         {product}-signing#provisioning_profile_base64, then `stado release submit` {product}"
    );
    Ok(())
}
