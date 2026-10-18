//! The `weles-browser-task` workload: one objective, one session label, and
//! the sign-in material a page is allowed to receive.

use serde_json::{json, Value};

use crate::cli::workload::plan::{flag, print_json, required_text};
use crate::cli::CmdError;

pub(crate) async fn run_weles_browser_task(
    target: &str,
    plan: &Value,
    json_output: bool,
) -> Result<(), CmdError> {
    let url = required_text(Some(plan), "url")?;
    let mut objective = required_text(Some(plan), "objective")?.to_string();
    if let Some(path) = objective.strip_prefix('@') {
        objective = std::fs::read_to_string(path)
            .map_err(|error| {
                CmdError::usage(format!(
                    "workload objective file {path} cannot be read: {error}"
                ))
            })?
            .trim()
            .to_string();
    } else {
        objective = objective.trim().to_string();
    }
    if objective.is_empty() {
        return Err(CmdError::usage(
            "weles-browser-task plan declares an empty objective; add the task to the plan",
        ));
    }
    let session_label = required_text(Some(plan), "session_label")?;
    // The action and the allowlist are the plan's when it names them, and
    // the declared defaults of the browser-task module when it does not.
    let action = match plan.get("action").and_then(Value::as_str) {
        Some(action) => action,
        None => crate::deploy::weles_browser_task::DEFAULT_ACTION,
    };
    let allowlist_file = match plan.get("allowlist_file").and_then(Value::as_str) {
        Some(file) => file,
        None => crate::deploy::weles_browser_task::DEFAULT_ALLOWLIST_FILE,
    };
    let login_item = plan.get("login_item").and_then(Value::as_str);
    let account_id = plan.get("account_id").and_then(Value::as_str);
    let fresh_profile = flag(plan, "fresh_profile")?;
    let allow_login = flag(plan, "allow_login")?;
    let sign_in_origin = plan.get("sign_in_origin").and_then(Value::as_str);
    let sign_in_item = plan.get("sign_in_item").and_then(Value::as_str);
    let defer_fills = flag(plan, "defer_fills")?;
    let prefill_all = flag(plan, "prefill_all")?;
    let flow_name = plan.get("flow_name").and_then(Value::as_str);
    let windowed = flag(plan, "windowed")?;

    if defer_fills && prefill_all {
        return Err(CmdError::usage(
            "weles-browser-task plan cannot enable both defer_fills and prefill_all; choose one",
        ));
    }
    let sign_in =
        match (sign_in_origin, sign_in_item) {
            (None, None) => None,
            (Some(_), None) => {
                return Err(CmdError::usage(
                    "weles-browser-task plan sign_in_origin needs sign_in_item; add the vault item",
                ))
            }
            (None, Some(_)) => return Err(CmdError::usage(
                "weles-browser-task plan sign_in_item needs sign_in_origin; add the page origin",
            )),
            (Some(origin), Some(item)) => {
                if !allow_login {
                    return Err(CmdError::usage(
                        "weles-browser-task plan sign_in_origin requires allow_login=true",
                    ));
                }
                let origin = crate::deploy::weles_browser_task::exact_origin(origin)
                    .map_err(|error| CmdError::usage(error.to_string()))?;
                Some((origin, item))
            }
        };
    let parsed = url::Url::parse(url)
        .map_err(|error| CmdError::usage(format!("workload plan url is not a URL: {error}")))?;
    if !crate::primitives::http::origin::is_web(&parsed) || !parsed.username().is_empty() {
        return Err(CmdError::usage(
            "weles-browser-task plan url must be HTTP or HTTPS without embedded credentials",
        ));
    }
    if let Some(item) = login_item {
        // A Weles item id follows the grammar of every Weles identifier the
        // account id is checked by, and begins with a letter or digit.
        let item = crate::deploy::weles_capture::checked_account_id(item).map_err(|error| {
            CmdError::usage(format!(
                "weles-browser-task plan login_item is not a valid Weles item id: {error}"
            ))
        })?;
        if !item
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item is not a valid Weles item id: it must begin \
                 with a letter or digit",
            ));
        }
        if !action.ends_with("_login") {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item requires an action ending in _login",
            ));
        }
        if !allow_login {
            return Err(CmdError::usage(
                "weles-browser-task plan login_item requires allow_login=true",
            ));
        }
    }
    let account_id = match account_id {
        Some(pinned) => Some(
            crate::deploy::weles_capture::checked_account_id(pinned)
                .map_err(CmdError::from)?
                .to_string(),
        ),
        None => fresh_profile.then(|| format!("stado-fresh-profile-{}", uuid::Uuid::new_v4())),
    };

    let replay = match plan.get("replay") {
        None => None,
        Some(value) => {
            let steps = value
                .as_array()
                .filter(|steps| !steps.is_empty())
                .ok_or_else(|| {
                    CmdError::usage("workload plan replay must be a nonempty array of steps")
                })?;
            for (index, step) in steps.iter().enumerate() {
                if !step.is_object()
                    || step
                        .get("tool")
                        .and_then(Value::as_str)
                        .is_none_or(|tool| tool.trim().is_empty())
                    || step.get("args").is_some_and(|args| !args.is_object())
                {
                    return Err(CmdError::usage(format!(
                        "workload plan replay step {index} requires a nonempty tool string and optional args object"
                    )));
                }
            }
            if action != "generic_browser_task" {
                return Err(CmdError::usage(
                    "workload plan replay requires action generic_browser_task",
                ));
            }
            Some(value.clone())
        }
    };
    let params = match plan.get("params") {
        None => serde_json::Map::new(),
        Some(value) => {
            let declared = value.as_object().ok_or_else(|| {
                CmdError::usage(
                    "workload plan params must be an object of the action's own parameters",
                )
            })?;
            let reserved = crate::deploy::weles_browser_task::TASK_PARAMETER_NAMES;
            if let Some(taken) = declared
                .keys()
                .find(|name| reserved.contains(&name.as_str()))
            {
                return Err(CmdError::usage(format!(
                    "workload plan params names {taken:?}, which the task itself sets; declare it in the plan's own field"
                )));
            }
            declared.clone()
        }
    };

    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let allowlist =
        crate::deploy::weles_browser_task::host_allowlist(&resolved, allowlist_file, &runner)
            .await
            .map_err(CmdError::from)?;
    crate::deploy::weles_browser_task::ensure_allowed(&resolved.name, action, &allowlist)
        .map_err(CmdError::from)?;

    let (credential_prefill, credential_deferred) = match &sign_in {
        None => (Vec::new(), Vec::new()),
        Some((origin, item)) => {
            let prefill = crate::deploy::weles_browser_task::issue_sign_in_prefill(
                &resolved,
                origin,
                item,
                crate::deploy::weles_browser_task::REGISTERED_SCOPES_FILE,
                &runner,
            )
            .await
            .map_err(CmdError::from)?;
            if !json_output {
                println!("sign-in:   {origin} as the account in {item}");
                println!(
                    "prefill:    {} field(s) to {}, issued on {}, single-use",
                    prefill.entries.len(),
                    prefill.agents.join(", "),
                    resolved.name
                );
                if !prefill.deferred.is_empty() {
                    println!(
                        "deferred:   {} field(s) handed over unspent, for the page that has them",
                        prefill.deferred.len()
                    );
                }
                if !prefill.unconfirmed.is_empty() {
                    println!(
                        "note:       this channel could not confirm {} in the vault; the worker broker reads it at fill time",
                        prefill.unconfirmed.join(", ")
                    );
                }
            }
            if defer_fills {
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (Vec::new(), all)
            } else if prefill_all {
                let mut all = prefill.entries;
                all.extend(prefill.deferred);
                (all, Vec::new())
            } else {
                (prefill.entries, prefill.deferred)
            }
        }
    };

    let task = crate::deploy::weles_browser_task::BrowserTask {
        action,
        url: parsed.as_str(),
        objective: &objective,
        session_label,
        login_item,
        account_id: account_id.as_deref(),
        fresh_profile,
        allow_login,
        headless: !windowed,
        credential_prefill,
        replay,
        params,
    };
    let outcome =
        crate::deploy::weles_browser_task::submit(target, &task, flow_name, &credential_deferred)
            .await
            .map_err(|error| CmdError::from(error).within(target))?;
    if json_output {
        let mut report = outcome.to_report(&resolved.name, action);
        report.insert("kind".to_string(), json!("weles-browser-task"));
        print_json(&Value::Object(report));
    } else {
        println!("host:      {}", resolved.name);
        println!("action:    {action}");
        println!("run:       {}", outcome.run_id);
        println!("outcome:   {}", if outcome.ok { "ok" } else { "failed" });
        if let Some(code) = outcome.exit_code {
            println!("exit:      {code}");
        }
        if let Some(profile) = &outcome.profile {
            println!(
                "profile:   {}",
                crate::cli::reporting::field(&profile["directory"])
            );
        }
        if !outcome.result.is_null() {
            println!("result:    {}", serde_json::to_string(&outcome.result)?);
        }
        if let Some(outputs) = outcome.outputs.as_object() {
            for name in outputs.keys() {
                println!("output:    {name}");
            }
        }
        if let Some(errors) = outcome.output_errors.as_ref().and_then(Value::as_object) {
            for (file, error) in errors {
                println!("unreadable output: {file}: {error}");
            }
        }
        if !outcome.stdout_tail.is_empty() {
            println!("{}", outcome.stdout_tail.trim_end());
        }
    }
    if outcome.ok {
        Ok(())
    } else {
        let detail = match (outcome.stdout_tail.trim(), outcome.stderr_tail.trim()) {
            ("", "") => "Weles returned no process output".to_string(),
            (stdout, stderr) => format!("stdout:\n{stdout}\nstderr:\n{stderr}"),
        };
        Err(CmdError::click(format!(
            "{}: {action} run {} did not succeed: {detail}",
            resolved.name, outcome.run_id
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown))
    }
}
