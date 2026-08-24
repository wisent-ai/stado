//! `stado product`: the fleet-facing doorway into the canonical Wisent product
//! catalogue. Product identity and recipes live in `wisent-ai/wisent-products`;
//! Stado owns hosts and services, so it delegates the product lifecycle to that
//! executable rather than copying its catalogue or installer logic.

use std::path::PathBuf;

use clap::Subcommand;
use tokio::process::Command;

use super::CmdError;
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
}

#[derive(Debug, clap::Args)]
pub struct ProductMutation {
    product: String,
    #[arg(long, value_parser = ["cli", "desktop", "service"])]
    surface: String,
    /// Required only for a service surface.
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    json: bool,
}

fn candidates() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|dir| dir.join("wisent-products"))
        .collect();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".local/bin/wisent-products"));
        paths.push(home.join(".local/pipx/venvs/wisent-products/bin/wisent-products"));
    }
    paths
}

fn executable() -> Result<PathBuf, CmdError> {
    candidates()
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| CmdError::click(
            "Wisent Products is not installed; install `wisent-ai/wisent-products` with pipx before using `stado product`"
        ))
}

async fn invoke(arguments: Vec<String>) -> Result<(), CmdError> {
    let output = Command::new(executable()?)
        .args(&arguments)
        .output()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
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
        value.surface,
    ];
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
    }
}
