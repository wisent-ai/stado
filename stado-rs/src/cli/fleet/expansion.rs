//! Public expansion commands; plan writes only an immutable analysis receipt.
use crate::fleet_expansion::{self as expansion, Catalog, ExpansionReport};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum ExpansionCommands {
    /// Read the versioned catalog of sourced investment assumptions.
    Catalog {
        #[arg(long)]
        json: bool,
    },
    /// Atomically replace the catalog; blank economic values remain unknown.
    Set {
        /// Complete catalog JSON, or '-' to read JSON from stdin.
        #[arg(long)]
        document: String,
        /// Required current catalog version after its initial creation.
        #[arg(long)]
        expect_version: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Save an exact budget-constrained comparison; never buy or enroll.
    Plan {
        /// Total upfront AND operating expenditure budget in USD.
        #[arg(long)]
        budget_usd: f64,
        #[arg(long, default_value_t = expansion::constants::DEFAULT_HORIZON_MONTHS)]
        horizon_months: u32,
        #[arg(long, default_value_t = crate::primitives::constants::NEEDS_DEFAULT_WINDOW_DAYS)]
        days: i64,
        #[arg(long)]
        json: bool,
    },
    /// Read an immutable analysis, including its original evidence and estimates.
    Show {
        plan_id: String,
        #[arg(long)]
        json: bool,
    },
    /// Read retained plans, newest first.
    History {
        #[arg(long)]
        json: bool,
    },
}

fn emit<T: serde::Serialize>(value: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn report(report: &ExpansionReport, json: bool) -> Result<(), String> {
    if json {
        emit(report)
    } else {
        print!("{}", expansion::render_report(report));
        Ok(())
    }
}

pub async fn run(command: ExpansionCommands) -> Result<bool, String> {
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|e| format!("open expansion store: {e}"))?;
    match command {
        ExpansionCommands::Catalog { json } => {
            let record = expansion::read_catalog(&store).await?;
            if !json {
                println!("Expansion catalog (null amounts are unknown; estimates are not measured earnings):");
            }
            emit(&record)?;
        }
        ExpansionCommands::Set {
            document,
            expect_version,
            json,
        } => {
            let raw = if document == "-" {
                std::io::read_to_string(std::io::stdin())
                    .map_err(|e| format!("read expansion catalog stdin: {e}"))?
            } else {
                document
            };
            let catalog: Catalog =
                serde_json::from_str(&raw).map_err(|e| format!("decode expansion catalog: {e}"))?;
            let saved =
                expansion::replace_catalog(&store, catalog, expect_version.as_deref()).await?;
            if !json {
                println!("Expansion catalog persisted and read back:");
            }
            emit(&saved)?;
        }
        ExpansionCommands::Plan {
            budget_usd,
            horizon_months,
            days,
            json,
        } => {
            let registry = crate::cli::registry::read_registry()
                .await
                .map_err(|e| format!("read expansion registry: {e}"))?;
            let planned =
                expansion::create_plan(&store, &registry, budget_usd, horizon_months, days).await?;
            report(&planned, json)?;
            return Ok(matches!(planned.status.as_str(), "ready" | "no_needs"));
        }
        ExpansionCommands::Show { plan_id, json } => {
            report(&expansion::read_plan(&store, &plan_id).await?, json)?
        }
        ExpansionCommands::History { json } => {
            let plans = expansion::history(&store).await?;
            if json {
                emit(&serde_json::json!({ "plans": plans }))?;
            } else if plans.is_empty() {
                println!("no expansion plans recorded");
            } else {
                for plan in plans {
                    println!(
                        "{} {} {} — {:.2} USD / {} months",
                        plan.plan_id,
                        plan.generated_at,
                        plan.status,
                        plan.budget_usd,
                        plan.horizon_months
                    );
                }
            }
        }
    }
    Ok(true)
}
