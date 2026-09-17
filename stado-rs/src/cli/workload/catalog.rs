//! The workload declaration this build compiles in, and the reads over it.
//!
//! `stado-rs/data/work/workloads.json` is the declaration; nothing else in the
//! crate parses it.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

pub const DECLARATION_PATH: &str = "stado-rs/data/work/workloads.json";
const DECLARATION: &str = include_str!("../../../data/work/workloads.json");
const SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WorkloadCatalog {
    schema_version: u64,
    pub(crate) workloads: Vec<WorkloadKind>,
}

/// What one placed workload holds on its host while it runs. The numbers
/// are the declaration's, never a caller's guess: `stado workload attach`
/// and `stado workload run` acquire exactly this much before the process
/// starts, and the host's agent subtracts it from what it publishes.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
pub struct WorkloadReservation {
    pub cpu_cores: i64,
    pub ram_gb: f64,
    pub vram_gb: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkloadKind {
    pub kind: String,
    pub product: String,
    pub interactive: bool,
    pub registry_allowance: Option<String>,
    pub plan_schema: Option<String>,
    pub reservation: Option<WorkloadReservation>,
    pub report: Vec<String>,
}

impl WorkloadKind {
    /// The declared hold, refused loudly when the declaration forgot it:
    /// a kind that reserves nothing is a kind the fleet cannot pool.
    pub fn reservation(&self) -> Result<WorkloadReservation, CmdError> {
        self.reservation.ok_or_else(|| {
            CmdError::click(format!(
                "workload kind '{}' declares no reservation; add it to {DECLARATION_PATH}",
                self.kind
            ))
        })
    }
}

static CATALOG: LazyLock<Result<WorkloadCatalog, String>> = LazyLock::new(|| {
    let parsed: WorkloadCatalog = serde_json::from_str(DECLARATION)
        .map_err(|error| format!("{DECLARATION_PATH} is not readable JSON: {error}"))?;
    if parsed.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}; this build reads {SCHEMA_VERSION}",
            parsed.schema_version
        ));
    }
    if parsed.workloads.is_empty() {
        return Err(format!("{DECLARATION_PATH} declares no workloads"));
    }
    let mut names = BTreeSet::new();
    for workload in &parsed.workloads {
        if workload.kind.trim().is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} carries a workload with no kind"
            ));
        }
        if !names.insert(workload.kind.as_str()) {
            return Err(format!(
                "{DECLARATION_PATH} declares workload '{}' more than once",
                workload.kind
            ));
        }
        if workload.product.trim().is_empty() {
            return Err(format!(
                "{} declares no product; add it to {DECLARATION_PATH}",
                workload.kind
            ));
        }
        if workload.report.is_empty() {
            return Err(format!(
                "{} declares no report fields; add them to {DECLARATION_PATH}",
                workload.kind
            ));
        }
        match workload.reservation {
            None => {
                return Err(format!(
                    "workload kind '{}' declares no reservation; add it to {DECLARATION_PATH}",
                    workload.kind
                ))
            }
            Some(reservation)
                if reservation.cpu_cores < 0
                    || reservation.ram_gb < 0.0
                    || reservation.vram_gb < 0 =>
            {
                return Err(format!(
                    "workload kind '{}' declares a negative reservation; fix it in {DECLARATION_PATH}",
                    workload.kind
                ))
            }
            Some(_) => {}
        }
    }
    Ok(parsed)
});

pub(crate) fn catalog() -> Result<&'static WorkloadCatalog, CmdError> {
    CATALOG
        .as_ref()
        .map_err(|message| CmdError::click(message.clone()))
}

pub fn workload(kind: &str) -> Result<&'static WorkloadKind, CmdError> {
    catalog()?
        .workloads
        .iter()
        .find(|candidate| candidate.kind == kind)
        .ok_or_else(|| {
            CmdError::click(format!(
                "workload kind '{kind}' is not declared; add it to {DECLARATION_PATH}"
            ))
        })
}

pub(crate) fn list(json_output: bool) -> Result<(), CmdError> {
    let catalog = catalog()?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(catalog)?);
    } else {
        crate::cli::reporting::table::print(
            &[
                "KIND",
                "PRODUCT",
                "MODE",
                "ALLOWANCE",
                "PLAN SCHEMA",
                "REPORT",
            ],
            &catalog
                .workloads
                .iter()
                .map(|workload| {
                    vec![
                        workload.kind.clone(),
                        workload.product.clone(),
                        if workload.interactive {
                            "interactive"
                        } else {
                            "batch"
                        }
                        .to_string(),
                        workload
                            .registry_allowance
                            .as_deref()
                            .unwrap_or("none")
                            .to_string(),
                        workload
                            .plan_schema
                            .as_deref()
                            .unwrap_or("none")
                            .to_string(),
                        workload.report.join(", "),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    Ok(())
}
