//! The binary of a product a host runs under its own declaration rather than
//! under release control, attested by its delivery receipt when it has one.

use serde_json::json;

use crate::cli::CmdError;

/// The binary of a product this host runs under its own declaration rather
/// than under release control: `targets[].managed_versions` names the
/// version, and the host's software report names the program at that version.
///
/// `release version show` can judge such a host `in-sync` while this command
/// refuses it with `release product "skarbiec" has no target "<host>"`, and
/// Weles, which asks this command for the Skarbiec it runs, then does not
/// start. Both reads answer from what the host declares and reports.
pub(super) async fn declared_binary(
    product: &str,
    target: &crate::targets::ComputeTarget,
    target_name: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let declared = target.managed_versions.get(product).ok_or_else(|| {
        CmdError::click(format!(
            "release product {product:?} has no target {target_name:?}, and {target_name} \
             declares no managed version of {product} either"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    // This command only ever answers for the host it runs on, so a report
    // that names no such program, another version, or other bytes than the
    // file on disk is a report that is behind: this host is looked at again
    // before anything is judged. Each machine keeps its own report, and the
    // vault host's was a week old when it installed Skarbiec 0.4.7, so it
    // refused its own Skarbiec; after Skarbiec 0.4.12 replaced 0.4.10 the
    // report still named 0.4.10's digest under a matching version, and Weles,
    // which asks this command and checks the bytes, kept failing to start.
    // A path the report names that cannot be read is behind too: the bytes
    // are unproven, so the report is not trusted for them.
    let mut report = crate::host_software::load(target_name);
    let behind = report.find(product).is_none_or(|row| {
        &row.version != declared
            || !crate::release_control::sha256_file(std::path::Path::new(&row.path))
                .is_ok_and(|(_, on_disk)| on_disk == row.sha256)
    });
    if behind {
        let runner = crate::deploy::production_runner();
        report = crate::host_software::refresh(target, &[], &runner)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "{target_name}'s software report names no {product} {declared}, and looking \
                     at this host again failed: {error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
    }
    let row = report.find(product).ok_or_else(|| {
        CmdError::click(format!(
            "{target_name} declares {product} {declared}, but its software report ({}) names no \
             {product} program; `stado release version show --host {target_name}` records one",
            report.summary()
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    if &row.version != declared {
        return Err(CmdError::click(format!(
            "{target_name} declares {product} {declared}, and its software report ({}) names {} \
             at version {}; `stado release version show --host {target_name}` says which is stale",
            report.age(),
            row.path,
            row.version
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // Attested when it can be: the signed release of that version, and the
    // receipt its delivery left on this host, both name these bytes. A
    // program the host runs under its own declaration with no delivery on
    // this host (the vault owner, typically) is answered as `declared` with
    // its reported digest and path, and the caller checks the bytes itself:
    // refusing it keeps weles-admission down. A receipt that names other
    // bytes is still refused.
    let platform = target.release_platform.as_str();
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            CmdError::click("this account has no HOME, so no delivery receipt is found")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let receipt_path = home
        .join(".stado/releases")
        .join(product)
        .join(declared)
        .join(platform)
        .join("release-receipt.json");
    let receipt = super::super::receipt::read(&receipt_path)?;
    let Some(receipt) = receipt else {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "state": "declared",
                    "product": product,
                    "target": target_name,
                    "version": declared,
                    "sha256": row.sha256,
                    "path": row.path,
                }))?
            );
        } else {
            println!("{}", row.path);
        }
        return Ok(());
    };
    let artifact =
        crate::cli::release_cmd::verified_artifact_for_submit(product, declared, platform).await?;
    let archive = receipt["sha256"].as_str().unwrap_or_default();
    let installed = receipt["artifact_sha256"].as_str().unwrap_or_default();
    if archive != artifact.artifact_sha256 || installed != row.sha256 {
        return Err(CmdError::refused(format!(
            "{target_name} runs {product} {declared} at {}, but its delivery receipt names \
             archive {archive} and file {installed}, while the signed release names archive {} \
             and the host reports file {}",
            row.path, artifact.artifact_sha256, row.sha256
        )));
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": "active",
                "product": product,
                "target": target_name,
                "version": declared,
                "platform": platform,
                "artifact_sha256": artifact.artifact_sha256,
                "manifest_sha256": artifact.manifest_sha256,
                "path": row.path,
            }))?
        );
    } else {
        println!("{}", row.path);
    }
    Ok(())
}
