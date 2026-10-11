//! `stado release logs` for a product rolled out by replacing a managed unit:
//! that unit's own logs.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::{ReleaseLogsArgs, StreamArg};

/// The logs of a product whose policy replaces a managed unit in place: the
/// release agent restarts the unit the policy's logical `service` runs as on
/// the target, and that unit's own StandardOutPath and StandardErrorPath are
/// where the product writes, so a readiness failure's cause is there and
/// nowhere under `logs_root`. The unit is named by the rule the rollout
/// itself uses, and the read is the one `stado service logs` makes, so this
/// command cannot look at a unit the rollout never restarted or a file the
/// unit never writes. `--head` has no meaning for a unit log, which is read
/// whole or from its tail, and is refused by name.
pub(super) async fn unit_logs(
    args: &ReleaseLogsArgs,
    target_name: &str,
    service: &str,
    version: &str,
) -> Result<(), CmdError> {
    if args.head {
        return Err(CmdError::usage(format!(
            "{} rolls out by replacing the managed unit its service {service} runs as, whose \
             logs are read whole or from their tail; drop --head",
            args.product
        )));
    }
    let document = crate::cli::registry::fetch_document().await?;
    let unit = crate::service_resolution::managed_unit_on(&document, service, target_name)
        .map_err(|refusal| {
            CmdError::click(format!(
                "{} rolls out by replacing the unit its service {service} runs as on \
                 {target_name}, and none can be named: {refusal}",
                args.product
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let declared = crate::cli::service::declared_matching(&unit, Some(target_name)).await?;
    let runner = crate::deploy::production_runner();
    let mut tails = Vec::new();
    for declared_unit in &declared {
        let target = crate::cli::canonical_host(&declared_unit.host).await?;
        tails.push(
            crate::deploy::service::tail_unit_logs(
                &target,
                declared_unit.unit_id(),
                &declared_unit.path,
                args.lines.map(std::num::NonZeroUsize::get),
                &runner,
            )
            .await
            .map_err(CmdError::from)?,
        );
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "product": args.product,
                "target": target_name,
                "version": version,
                "service": service,
                "unit": unit,
                "selection": if args.lines.is_some() { "tail" } else { "whole" },
                "units": tails.iter().map(crate::deploy::service::ServiceLog::to_json).collect::<Vec<Value>>(),
            }))?
        );
        return Ok(());
    }
    for tail in &tails {
        if matches!(args.stream, StreamArg::Out | StreamArg::Both) {
            println!(
                "--- {} {} stdout ({}): the managed unit {} {} rolls out onto",
                tail.host, tail.unit, tail.origin, args.product, version
            );
            print!("{}", tail.body);
            if !tail.body.ends_with('\n') {
                println!();
            }
        }
        if let (true, Some(error_origin)) = (
            matches!(args.stream, StreamArg::Err | StreamArg::Both),
            &tail.error_origin,
        ) {
            println!(
                "--- {} {} stderr ({}): the managed unit {} {} rolls out onto",
                tail.host, tail.unit, error_origin, args.product, version
            );
            print!("{}", tail.error_body);
            if !tail.error_body.is_empty() && !tail.error_body.ends_with('\n') {
                println!();
            }
        }
    }
    Ok(())
}
