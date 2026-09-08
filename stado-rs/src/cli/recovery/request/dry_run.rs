//! `--dry-run`: the whole request rendered, and nothing performed.
//!
//! The plan is printed after validation and after the config has been prepared
//! in memory, so what it shows is the request that would actually run — not a
//! restatement of the flags.

use crate::cli::recovery::request::{PreparedConfig, RecoveryMigrateArgs, ServiceRef};
use crate::queue::copy::Endpoint;

pub(in crate::cli::recovery) fn print_plan(
    args: &RecoveryMigrateArgs,
    source: &Endpoint,
    destination: &Endpoint,
    prepared: &PreparedConfig,
) {
    println!("DRY RUN — no network call, file write, service action, or billing change");
    println!("source:      {}", source.describe());
    println!("destination: {}", destination.describe());
    println!("config:      {}", prepared.path.display());
    println!("providers:   {}", args.enable_providers.join(", "));
    println!("writers:     {}", display_refs(&args.writers));
    println!("activate:    {}", display_refs(&args.activate));
    println!("resume:      {}", args.resume);
    println!(
        "billing:     {}",
        if args.manage_gcp_billing {
            "bounded GCP window"
        } else {
            "unchanged"
        }
    );
    println!("steps: destination fence+drain -> source billing window -> source fence+drain -> stop writers -> canonical copy -> full body+metadata verify -> close billing -> atomic config cutover -> selected restart -> optional resume");
}

fn display_refs(references: &[ServiceRef]) -> String {
    if references.is_empty() {
        return "none".to_string();
    }
    references
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<String>>()
        .join(", ")
}
