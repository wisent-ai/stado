//! `service auth-check`.

use super::*;

pub(crate) struct AuthCheckOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) item: Option<&'a str>,
    pub(crate) field: &'a str,
    pub(crate) consumer: Option<&'a str>,
    pub(crate) token_file: Option<&'a str>,
    pub(crate) url: &'a str,
    pub(crate) post_empty_json: bool,
    pub(crate) expect_status: Option<u16>,
    pub(crate) repair: bool,
    pub(crate) take_over_listener: bool,
    pub(crate) variable: Option<&'a str>,
    pub(crate) env_file: Option<&'a str>,
    pub(crate) as_json: bool,
}

pub(crate) async fn auth_check(options: AuthCheckOptions<'_>) -> Result<(), CmdError> {
    let AuthCheckOptions {
        name,
        host,
        item,
        field,
        consumer,
        token_file,
        url,
        post_empty_json,
        expect_status,
        repair,
        take_over_listener,
        variable,
        env_file,
        as_json,
    } = options;
    let repair_target = if repair {
        Some((
            variable.ok_or_else(|| CmdError::click("--repair requires --variable"))?,
            env_file.ok_or_else(|| CmdError::click("--repair requires --env-file"))?,
        ))
    } else {
        None
    };
    if item.is_none() && (variable.is_none() || env_file.is_none()) {
        return Err(CmdError::usage(
            "give --item, or both --variable and --env-file to read the bearer from the unit's own runtime environment",
        ));
    }
    // The bearer source is a property of the invocation, not of the host:
    // either a Skarbiec item read on the host by its own identity, or the
    // exact runtime assignment the unit already runs with. Neither mode
    // brings the secret back over the channel; only the HTTP outcome does.
    // This internal dispatcher preserves the CLI's two mutually exclusive
    // bearer sources; grouping the flags would only duplicate AuthCheckOptions.
    #[allow(clippy::too_many_arguments)]
    async fn check(
        target: &crate::targets::ComputeTarget,
        declared: &ManagedService,
        url: &str,
        item: Option<&str>,
        field: &str,
        consumer: Option<&str>,
        token_file: Option<&str>,
        variable: Option<&str>,
        env_file: Option<&str>,
        post_empty_json: bool,
        expect_status: Option<u16>,
        runner: &crate::deploy::Runner,
    ) -> Result<crate::deploy::service::RemoteReport, CmdError> {
        match (item, variable, env_file) {
            (Some(item), _, _) => service::check_service_item_bearer(
                target,
                declared,
                url,
                item,
                field,
                consumer,
                token_file,
                post_empty_json,
                expect_status,
                runner,
            )
            .await
            .map_err(click),
            (None, Some(variable), Some(env_file)) => service::check_service_env_bearer(
                target,
                declared,
                url,
                env_file,
                variable,
                post_empty_json,
                expect_status,
                runner,
            )
            .await
            .map_err(click),
            _ => unreachable!("usage guard above"),
        }
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let initial = check(
            &target,
            declared,
            url,
            item,
            field,
            consumer,
            token_file,
            variable,
            env_file,
            post_empty_json,
            expect_status,
            &runner,
        )
        .await?;
        let mut final_report = initial.clone();
        let mut synced = None;
        let mut restarted = None;
        let mut listener_reset = None;

        if !initial.succeeded("auth_ok") {
            if let Some((variable, env_file)) = repair_target {
                let Some(item) = item else {
                    return Err(CmdError::click(
                        "--repair synchronizes from a Skarbiec item; in --env-file mode the runtime file is already the source",
                    ));
                };
                let sync_report = service::sync_service_item_secret(
                    &target, declared, env_file, variable, item, field, &runner,
                )
                .await
                .map_err(click)?;
                let sync_ok = sync_report.succeeded("secret_synced");
                synced = Some(sync_report);
                if sync_ok {
                    let restart_report = service::restart_service(&target, declared, &runner)
                        .await
                        .map_err(click)?;
                    let restart_ok = restart_report.succeeded("restarted");
                    restarted = Some(restart_report);
                    if restart_ok {
                        final_report = check(
                            &target,
                            declared,
                            url,
                            Some(item),
                            field,
                            consumer,
                            token_file,
                            Some(variable),
                            Some(env_file),
                            post_empty_json,
                            expect_status,
                            &runner,
                        )
                        .await?;
                    }
                }
            }
        }

        if repair
            && take_over_listener
            && !final_report.succeeded("auth_ok")
            && synced
                .as_ref()
                .is_some_and(|report| report.succeeded("secret_synced"))
        {
            let reset_report = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            let reset_ok = reset_report.succeeded("listener_stopped")
                || reset_report.succeeded("listener_absent");
            listener_reset = Some(reset_report);
            if reset_ok {
                let restart_report = service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?;
                let restart_ok = restart_report.succeeded("restarted");
                restarted = Some(restart_report);
                if restart_ok {
                    final_report = check(
                        &target,
                        declared,
                        url,
                        item,
                        field,
                        consumer,
                        token_file,
                        variable,
                        env_file,
                        post_empty_json,
                        expect_status,
                        &runner,
                    )
                    .await?;
                }
            }
        }

        let ok = final_report.succeeded("auth_ok");
        if !ok {
            failures.push(format!("{}: {}", declared.host, final_report.failure()));
        }
        let repair_status = listener_reset
            .as_ref()
            .or(synced.as_ref())
            .map(|report| report.status.as_str())
            .unwrap_or("-");
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&final_report.status),
            dash(repair_status),
            dash(&final_report.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "url": url,
            "post_empty_json": post_empty_json,
            "expect_status": expect_status,
            "initial": initial.to_json(),
            "sync": synced.as_ref().map(|report| report.to_json()),
            "restart": restarted.as_ref().map(|report| report.to_json()),
            "listener_reset": listener_reset.as_ref().map(|report| report.to_json()),
            "final": final_report.to_json(),
            "ok": ok,
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "AUTH", "REPAIR", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "authentication check")
}
