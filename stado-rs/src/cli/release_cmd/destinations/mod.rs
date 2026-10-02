//! Fleet-local delivery declarations, separate from host-independent recipes.

use crate::cli::{registry, CmdError};
use crate::release_pipeline::destinations;
use clap::{Args, Subcommand};
use serde_json::{json, Value};

pub(crate) mod adopt;
mod state;

#[derive(Args)]
pub struct DestinationArgs {
    #[command(subcommand)]
    command: DestinationCommand,
}

#[derive(Subcommand)]
enum DestinationCommand {
    /// List the canonical product destination declarations; does not install anything.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read one product's declared targets and their registry platforms.
    Show {
        product: String,
        #[arg(long)]
        json: bool,
    },
    /// Replace one product's complete target set through a verified registry write.
    Set {
        product: String,
        #[arg(long = "target", required = true)]
        targets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Adopt the existing catalog's explicit delivery hosts without changing them.
    Adopt {
        product: String,
        #[arg(long)]
        json: bool,
    },
    /// Remove a declaration. Already frozen release runs keep their destinations.
    Remove {
        product: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(args: DestinationArgs) -> Result<(), CmdError> {
    let (answer, as_json) = match args.command {
        DestinationCommand::List { json } => {
            let (document, generation) = registry::fetch_versioned_document().await?;
            destinations::validate(&document).map_err(CmdError::click)?;
            let mut products = Vec::new();
            if let Some(declarations) =
                destinations::declarations(&document).map_err(CmdError::click)?
            {
                for product in declarations.keys() {
                    products.push(serde_json::json!({
                        "product": product,
                        "destinations": destinations::read(&document, product).map_err(CmdError::click)?,
                    }));
                }
            }
            (
                serde_json::json!({"registry_generation": generation, "products": products}),
                json,
            )
        }
        DestinationCommand::Show { product, json } => (state::show(&product).await?, json),
        DestinationCommand::Set {
            product,
            targets,
            json,
        } => {
            let generation = state::set(&product, &targets).await?;
            (
                json!({"product": product, "state": "declared", "targets": targets, "registry_generation": generation}),
                json,
            )
        }
        DestinationCommand::Adopt { product, json } => {
            let (targets, generation) = adopt::from_catalog(&product).await?;
            (
                json!({"product": product, "state": "declared", "targets": targets, "registry_generation": generation}),
                json,
            )
        }
        DestinationCommand::Remove { product, json } => {
            let generation = state::remove(&product).await?;
            (
                json!({"product": product, "state": "absent", "registry_generation": generation}),
                json,
            )
        }
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&answer)?);
    } else {
        print(&answer);
    }
    Ok(())
}

fn print(answer: &Value) {
    println!(
        "registry generation {}",
        answer["registry_generation"].as_str().unwrap_or_default()
    );
    if let Some(products) = answer["products"].as_array() {
        for product in products {
            println!(
                "{}: {}",
                product["product"].as_str().unwrap_or_default(),
                product["destinations"]
            );
        }
    } else {
        println!(
            "{}: {}",
            answer["product"].as_str().unwrap_or_default(),
            answer
        );
    }
}
