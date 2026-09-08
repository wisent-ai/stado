//! The `verify` verb: run the registry's verification adapter over a ref and
//! print its report, exiting 1 (silently, the report is already on stdout)
//! when the report did not pass.

use serde_json::Value;

use crate::cli::CmdError;

use super::format::{json_pretty_sorted, parse_ref};
use super::registry;

pub(super) async fn verify(r#ref: &str, full: bool, as_json: bool) -> Result<(), CmdError> {
    let registry = registry().await?;
    let report = registry.verify(&parse_ref(r#ref)?, full).await?;
    if as_json {
        let value = serde_json::to_value(&report)?;
        println!("{}", json_pretty_sorted(&value));
    } else {
        println!(
            "{} ({})",
            if report.passed { "PASSED" } else { "FAILED" },
            report.adapter
        );
        for issue in &report.issues {
            println!("- {issue}");
        }
        if !report.summary.is_empty() {
            println!(
                "{}",
                json_pretty_sorted(&Value::Object(report.summary.clone()))
            );
        }
    }
    if !report.passed {
        return Err(CmdError::silent(1));
    }
    Ok(())
}
