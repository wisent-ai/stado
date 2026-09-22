//! `stado product`: the fleet-facing doorway into the canonical Wisent product
//! catalogue. Product identity and recipes live in `wisent-ai/wisent-products`;
//! Stado owns hosts and services, so it delegates the product lifecycle to that
//! executable rather than copying its catalogue or installer logic.

use clap::Subcommand;
use tokio::process::Command;

use crate::cli::CmdError;
#[derive(Debug, Subcommand)]
pub enum ProductCommands {
    /// Every canonical Wisent product and its installable surfaces.
    Catalog {
        #[arg(long)]
        json: bool,
    },
    /// Install one product surface from its canonical recipe.
    Install(ProductMutation),
    /// Read the recorded lifecycle state of one product surface.
    Status(ProductMutation),
    /// Re-run the recipe, retaining the previous installation for rollback.
    Update(ProductMutation),
    /// Restore the most recent retained installation.
    Rollback(ProductMutation),
    /// Remove one product surface and its recorded files/service.
    Remove(ProductMutation),
    /// Re-install every catalogued product on one surface that is behind `origin/main`.
    Sync(ProductSweep),
    /// Inspect stable macOS signing identities, or reconcile installed native code.
    Signatures(ProductSignatures),
}

/// The surfaces a product can be installed on. Written as a type so the
/// three names exist once: clap derives the accepted values and the help
/// from it, and the three `value_parser` lists that used to carry them
/// could not disagree about which surfaces exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lower")]
pub enum Surface {
    Cli,
    Desktop,
    Service,
}

impl Surface {
    /// The name the recipe and the recorded state use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Desktop => "desktop",
            Self::Service => "service",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct ProductMutation {
    product: String,
    #[arg(long, value_enum)]
    surface: Surface,
    /// Required only for a service surface.
    #[arg(long)]
    host: Option<String>,
    /// Existing signed release to install without compiling; install/update only.
    #[arg(long, requires = "source_commit")]
    release_version: Option<String>,
    /// Full accepted source commit of the requested release.
    #[arg(long, requires = "release_version")]
    source_commit: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
pub struct ProductSignatures {
    product: String,
    #[arg(long, value_enum, default_value_t = Surface::Cli)]
    surface: Surface,
    #[arg(long)]
    apply: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, clap::Args)]
pub struct ProductSweep {
    #[arg(long, value_enum)]
    surface: Surface,
    /// `git fetch origin` in each checkout first.
    #[arg(long)]
    fetch: bool,
    /// Decide and report without installing.
    #[arg(long)]
    dry_run: bool,
    /// Required only for a service surface.
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    json: bool,
}

/// The Skarbiec item Wisent Products resolves the signing certificate and key
/// from. Only the item id crosses this boundary: `wisent-products` reads both
/// fields itself and loads them into a temporary keychain it removes
/// afterwards, so no key material passes through Stado, an environment value,
/// or a command line.
///
/// It is named here because a product install signs native code, and the
/// machine running it keeps no identity of its own. Without this, an install
/// could only be signed where the fleet certificate happened to sit in a
/// personal keychain: on 2026-09-20 `stado product update skrzynka --surface
/// cli` refused with "Apple signing identity is missing or ambiguous: Apple
/// Development: Created via API (685D4U2G83)" on a Mac whose keychain held one
/// unrelated certificate, while that exact certificate was in the vault the
/// whole time.
const SIGNING_CREDENTIAL_ITEM: &str = "desktop-signing-apple-development";

async fn invoke(arguments: Vec<String>) -> Result<(), CmdError> {
    let sdk = crate::deploy::native_signing::runtime::local(&crate::deploy::production_runner())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut command = Command::new(sdk);
    command.args(&arguments);
    // An operator who has already chosen a credential keeps it: this supplies
    // the fleet's item only when nothing else was named.
    if std::env::var_os("WISENT_CODESIGN_CREDENTIAL_ITEM").is_none()
        && std::env::var_os("WISENT_CODESIGN_CERTIFICATE_PEM").is_none()
    {
        command.env("WISENT_CODESIGN_CREDENTIAL_ITEM", SIGNING_CREDENTIAL_ITEM);
    }
    let output = command
        .output()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(CmdError::click(if detail.is_empty() {
        format!("wisent-products exited with {}", output.status)
    } else {
        detail
            .strip_prefix("Error: ")
            .unwrap_or(&detail)
            .to_string()
    }))
}

fn mutation_args(verb: &str, value: ProductMutation) -> Vec<String> {
    let mut args = vec![
        verb.to_string(),
        value.product,
        "--surface".to_string(),
        value.surface.as_str().to_string(),
    ];
    if let Some(host) = value.host {
        args.extend(["--host".to_string(), host]);
    }
    if let Some(version) = value.release_version {
        args.extend(["--release-version".to_string(), version]);
    }
    if let Some(commit) = value.source_commit {
        args.extend(["--source-commit".to_string(), commit]);
    }
    if value.json {
        args.push("--json".to_string());
    }
    args
}

fn sweep_args(value: ProductSweep) -> Vec<String> {
    let mut args = vec![
        "sync".to_string(),
        "--surface".to_string(),
        value.surface.as_str().to_string(),
    ];
    if value.fetch {
        args.push("--fetch".to_string());
    }
    if value.dry_run {
        args.push("--dry-run".to_string());
    }
    if let Some(host) = value.host {
        args.extend(["--host".to_string(), host]);
    }
    if value.json {
        args.push("--json".to_string());
    }
    args
}

pub async fn dispatch(command: ProductCommands) -> Result<(), CmdError> {
    match command {
        ProductCommands::Catalog { json } => {
            let mut args = vec!["catalog".to_string()];
            if json {
                args.push("--json".to_string());
            }
            invoke(args).await
        }
        ProductCommands::Install(value) => invoke(mutation_args("install", value)).await,
        ProductCommands::Status(value) => invoke(mutation_args("status", value)).await,
        ProductCommands::Update(value) => invoke(mutation_args("update", value)).await,
        ProductCommands::Rollback(value) => invoke(mutation_args("rollback", value)).await,
        ProductCommands::Remove(value) => invoke(mutation_args("remove", value)).await,
        ProductCommands::Sync(value) => invoke(sweep_args(value)).await,
        ProductCommands::Signatures(value) => {
            let mut args = if value.apply {
                vec!["signing".into(), "reconcile".into(), value.product]
            } else {
                vec!["signing".into(), "report".into(), value.product]
            };
            args.extend(["--surface".into(), value.surface.as_str().into()]);
            if value.json {
                args.push("--json".into());
            }
            invoke(args).await
        }
    }
}
