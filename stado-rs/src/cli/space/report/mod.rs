//! `stado space report TARGET`: disk, memory, inventory, build caches, the
//! disk-full rule's verdict and the janitor's last pass as one document.

mod accelerators;
mod lines;

use lines::print_volumes;

use super::*;

pub(super) fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(super) fn cache_json(
    declaration: &crate::deploy::host_build_caches::BuildCacheDeclaration,
    report: &crate::deploy::host_build_caches::BuildCacheReport,
) -> Value {
    json!({
        "scan": {
            "source": "the disk-full rule: the janitor's build_caches cleaner walks the whole home",
            "root": declaration.root,
        },
        "entries": report.entries.iter().map(|entry| json!({
            "verdict": entry.state,
            "path": entry.path,
            "kib": entry.kib,
        })).collect::<Vec<Value>>(),
        "error": report.error,
    })
}

pub(super) async fn report(target_name: &str, json_output: bool) -> Result<(), CmdError> {
    let stages = crate::deploy::host_reclaim::declared_stages().map_err(CmdError::from)?;
    let target = crate::cli::canonical_host(target_name).await?;
    let runner = crate::deploy::production_runner();
    let cache_declaration =
        crate::deploy::host_build_caches::home_scan_for_target(&target, &runner)
            .await
            .map_err(CmdError::from)?;
    // One walk of the home for `CACHEDIR.TAG`: the verdict's. The disk read
    // runs without its own census beside it, and the census it reports is
    // the verdict's sized rows, so a report no longer walks the home twice at
    // once for the same tags.
    let (disk, cache_report) = tokio::join!(
        crate::deploy::host_disk::disk_read(
            &target,
            &runner,
            crate::deploy::host_disk::DiskScope::FullWithoutCensus
        ),
        crate::deploy::host_build_caches::report_declaration_on_host(
            &target,
            &cache_declaration,
            &runner
        ),
    );
    let mut disk = disk.map_err(CmdError::from)?;
    let (census, census_read) = cache_report.census();
    disk.reading.tagged_build_caches = census;
    disk.reading.tagged_build_caches_read = census_read;
    // `disk_report` always returns an object; the fields below join it.
    let mut document = match crate::deploy::host_disk::disk_report(&target, disk) {
        Value::Object(document) => document,
        _not_an_object => unreachable!("host_disk::disk_report returns an object"),
    };
    // Which walk the census rows came from, so a reader (and the real test)
    // can tell the verdict's one walk from a disk read's own census.
    document.insert(
        "tagged_build_output_source".to_string(),
        json!("the build_caches verdict's walk of the home"),
    );
    document.insert(
        "reclaim_stages".to_string(),
        Value::Array(
            stages
                .iter()
                .map(|stage| json!({"name": stage.name, "description": stage.description}))
                .collect(),
        ),
    );
    // Serde writes an absent option as `null`.
    document.insert("work_root".to_string(), json!(target.work_root));
    let mount = lines::work_root_mount(&Value::Object(document.clone()));
    document.insert("work_root_mount".to_string(), json!(mount));
    document.insert(
        "build_caches".to_string(),
        cache_json(&cache_declaration, &cache_report),
    );
    // The account's own home, so a declared `~/` root is expanded to the path
    // the host's own `du` walked. Guessing `/Users` or `/home` from a platform
    // is what makes a coverage report name a directory that does not exist.
    let home = crate::deploy::host_channel::remote_home(&target, &runner)
        .await
        .map_err(CmdError::from)?;
    let report = Value::Object(document);
    let weles_recordings_dir = target
        .weles
        .as_ref()
        .and_then(|weles| weles.recordings_dir.clone());
    let coverage = super::coverage::section(
        &report,
        stages,
        &home,
        &target.release_platform,
        weles_recordings_dir.as_deref(),
    );
    let mut document = match report {
        Value::Object(document) => document,
        _not_an_object => unreachable!("the report was built from an object above"),
    };
    document.insert("rule".to_string(), coverage["rule"].clone());
    document.insert("coverage".to_string(), coverage.clone());
    document.insert(
        "accelerators".to_string(),
        accelerators::accelerators_json(&target).await,
    );
    let report = Value::Object(document);
    if json_output {
        print_json(&report)?;
    } else {
        let usage = &report["usage"];
        let state = &report["cleanup_state"];
        // A host whose janitor has never run records no last pass.
        let last_pass = match state.get("last_pass_at").and_then(Value::as_str) {
            Some(stamp) => stamp,
            None => "never",
        };
        println!("{} space", target.name);
        if let Some(error) = report.get("inventory_incomplete").and_then(Value::as_str) {
            println!("inventory incomplete: {error}");
            println!("coverage lists only measured paths; missing paths have not been checked");
        }
        let usage_field = |key: &str| match usage.get(key).and_then(Value::as_str) {
            Some(text) => text.to_string(),
            None => format!("<{key} not reported>"),
        };
        println!(
            "disk: {} free KiB on {} ({})",
            usage_field("available_kb"),
            usage_field("filesystem"),
            usage_field("capacity"),
        );
        print_volumes(&report);
        lines::print_unread(&report);
        super::coverage::print_coverage(&coverage);
        println!("last pass: {last_pass}");
        lines::print_memory(&report["memory"]);
        if let Some(line) = report
            .get("accelerators")
            .and_then(|block| block.get("line"))
            .and_then(Value::as_str)
        {
            println!("accelerators: {line}");
        }
        for entry in &cache_report.entries {
            println!("cache\t{}\t{}\t{}", entry.state, entry.kib, entry.path);
        }
    }
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_disk::OK_STATUS) {
        // The host's own error, else the status it did report, so a read
        // that failed without a sentence is still told apart from one that
        // reported no status at all.
        let cause = match (report.get("error").and_then(Value::as_str), report.get("status")) {
            (Some(error), _) => error.to_string(),
            (None, Some(status)) => format!("it reported status {status} with no error"),
            (None, None) => "it reported neither a status nor an error".to_string(),
        };
        return Err(CmdError::click(format!(
            "{} space report could not read the host: {cause}",
            target.name
        ))
        .machine_readable(json_output));
    }
    // A verdict that ran out of seconds, or a walk the host's `find` ended
    // before it printed any tag, is reported as incomplete, not fatal: every
    // other figure above it was already read, and the host did answer. A
    // host that refused the read still fails the command.
    if let Some(error) = cache_report.error.filter(|error| !error.is_empty()) {
        let walk_failed = cache_report
            .entries
            .iter()
            .any(|entry| entry.state == crate::deploy::host_build_caches::SCAN_FAILED);
        if cache_report.timed_out || walk_failed {
            eprintln!("{} build cache verdict incomplete: {error}", target.name);
            return Ok(());
        }
        return Err(CmdError::click(format!(
            "{} build cache declaration could not be read: {error}",
            target.name
        ))
        .machine_readable(json_output));
    }
    Ok(())
}
