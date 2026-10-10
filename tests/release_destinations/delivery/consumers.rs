use super::super::runner::{require, Journey};
use serde_json::json;
use stado::deploy::shlex_quote;
use std::fs;

pub(super) fn checks(journey: &mut Journey) -> Result<(), String> {
    let targets = journey.configuration.targets.clone();
    let checks = journey.configuration.checks.clone();
    for target in targets {
        for check in &checks {
            let mut command = format!("\"$HOME/.stado/bin/\"{}", shlex_quote(&check.binary));
            for argument in &check.arguments {
                command.push(' ');
                command.push_str(&shlex_quote(argument));
            }
            let request = json!({
                "client_request_id": uuid::Uuid::new_v4().to_string(), "command": command,
                "pinned_host": target, "repo_extras": "",
            });
            let path = journey.output.join(format!(
                "consumer-{}.json",
                request["client_request_id"].as_str().unwrap()
            ));
            fs::write(
                &path,
                serde_json::to_vec_pretty(&request).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            let submitted = journey.cli(
                &[
                    "machine",
                    "submit",
                    "--request-file",
                    path.to_str().ok_or("non-UTF8 request path")?,
                ],
                true,
            )?;
            let job_id = submitted["result"]["job"]["job_id"]
                .as_str()
                .ok_or("consumer request has no real job identity")?;
            let log = super::observe_job(journey, job_id, &target)?;
            if check.expect_version {
                require(
                    log.split_whitespace()
                        .any(|field| field == journey.configuration.version),
                    "installed consumer did not report the released version",
                )?;
            }
        }
    }
    Ok(())
}
