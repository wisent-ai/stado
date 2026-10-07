//! One durable Apple Developer ID bundle for the fleet's desktop repositories.

use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::github::github_credential;
use crate::deploy::host_precheck_runner::declaration::runner_target;
use crate::deploy::host_precheck_runner::platform::{replace, Platform};
use crate::deploy::host_precheck_runner::signing::apple::{
    developer_id_bundle, issue_apple_capability, publish_developer_id_secrets,
    required_remote_file, APPLE_DEVELOPER_ID_ACTION, DEVELOPER_ID_FINISH, DEVELOPER_ID_KIND,
    DEVELOPER_ID_PREPARE, DEVELOPER_ID_ROLE,
};
use crate::deploy::host_precheck_runner::verdict::report::command_failure;
use crate::deploy::{
    host_capability, host_channel, production_runner, shlex_quote, weles_browser_task,
    weles_capture, DeployError,
};

/// Ensure one durable Developer ID bundle exists and grant it to desktop repositories.
///
/// The only interactive part is the Weles Account Holder trajectory. Stado creates
/// the private key and CSR on the selected host, queues the guarded trajectory,
/// waits for its downloaded certificate, stores the resulting PKCS#12 bundle in
/// Skarbiec, and publishes repository secrets. A later run reuses that bundle.
pub async fn bootstrap_developer_id(
    target_name: &str,
    account_role: &str,
    repositories: &[String],
) -> Result<Value, DeployError> {
    if repositories.is_empty() {
        return Err(DeployError(
            "at least one desktop repository is required".to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let github_token = github_credential().await?;
    if let Some((p12, password, identity, not_after)) = developer_id_bundle()? {
        publish_developer_id_secrets(repositories, &p12, &password, &identity, &github_token)?;
        return Ok(json!({
            "certificate_role": DEVELOPER_ID_ROLE,
            "identity": identity,
            "not_after": not_after,
            "repositories": repositories,
            "status": "reused",
            "target": target_name,
        }));
    }

    let target = runner_target(target_name).await?;
    if Platform::for_target(&target)? != Platform::DarwinArm64 {
        return Err(DeployError(format!(
            "{} cannot issue a Developer ID certificate: its release platform is {}",
            target.name, target.release_platform
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let remote_home = host_channel::remote_home(&target, &production_runner()).await?;
    let work = format!("{remote_home}/.stado/apple-developer-id");
    let prepare = replace(
        DEVELOPER_ID_PREPARE,
        &[("__WORK_DIR__", shlex_quote(&work))],
    );
    let prepare_output = host_channel::run_script(&target, &prepare, &production_runner()).await?;
    if !prepare_output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: Developer ID CSR preparation failed: {}",
            target.name,
            command_failure(&prepare_output, "remote CSR preparation failed")
        )));
    }

    let finish = replace(DEVELOPER_ID_FINISH, &[("__WORK_DIR__", shlex_quote(&work))]);
    let mut finish_output =
        host_channel::run_script(&target, &finish, &production_runner()).await?;
    // A completed Account Holder run downloads its certificate to
    // `{work}/certificate.cer`, which the finish script above reads, so a run
    // interrupted after the certificate was issued is already recovered there.
    if !finish_output.ok() {
        let admission = weles_capture::resolve_admission(&target.name).await?;
        let channel = weles_capture::open_channel(&admission).await?;
        let guard_id = uuid::Uuid::new_v4().to_string();
        let execution_agent = "weles-worker";
        // Resolved once: all three references must live in the same
        // broker state the worker's socket is served from, and that
        // broker is the host's, never this machine's.
        let broker = host_capability::resolve(
            &target,
            &weles_browser_task::weles_api_broker_files(),
            &production_runner(),
        )
        .await?;
        // The agent is not a label to choose. Skarbiec verifies the
        // redeemer's signature against the workload public key its vault
        // registered for the agent NAMED IN THE CAPABILITY, and the
        // acquisition catalog registers one consumer per coordinate. The
        // constant `weles-worker` this command used to name is registered
        // nowhere, which is why every redemption answered `no live vault
        // token registers a workload public key` — the same refusal
        // `weles_browser_task::scope_consumer` was written for after runs
        // 18e7cc47 and 47d89182 hit it.
        let routes = host_capability::routes(&target, &broker, &production_runner()).await?;
        let scopes = weles_browser_task::host_scopes(
            &target,
            weles_browser_task::REGISTERED_SCOPES_FILE,
            &production_runner(),
        )
        .await?;
        let registered_agent = |resource: &str| -> Result<String, DeployError> {
            let routed = weles_browser_task::routed_item(&routes, resource)?;
            weles_browser_task::scope_consumer(&scopes, &routed.item, &routed.field)
                .map(str::to_string)
                .ok_or_else(|| {
                    DeployError(format!(
                        "{}: {} registers no identity for {}/{}, so a capability for \
                         {resource} could only name an agent this host's vault does not \
                         know and its broker would deny at fill time",
                        target.name,
                        weles_browser_task::REGISTERED_SCOPES_FILE,
                        routed.item,
                        routed.field
                    ))
                    .stating(crate::primitives::failure::FailureCode::Config)
                })
        };
        let email_resource = "origin:https://idmsa.apple.com/email";
        let password_resource = "origin:https://idmsa.apple.com/password";
        let email_agent = registered_agent(email_resource)?;
        let password_agent = registered_agent(password_resource)?;
        let email = issue_apple_capability(
            &target,
            &broker,
            &email_agent,
            "weles.browser.fill",
            email_resource,
            &guard_id,
            &production_runner(),
        )
        .await?;
        let password = issue_apple_capability(
            &target,
            &broker,
            &password_agent,
            "weles.browser.fill",
            password_resource,
            &guard_id,
            &production_runner(),
        )
        .await?;
        // A challenge resource routes to no vault field by design - its
        // value is written later, by the relay - so no catalog row can
        // name it. It still needs an agent the vault registers, and the
        // password consumer is the one this run has already proven is
        // registered against the worker's workload key.
        let two_factor = issue_apple_capability(
            &target,
            &broker,
            &password_agent,
            "weles.apple.2fa",
            &format!("challenge:apple/{guard_id}"),
            &guard_id,
            &production_runner(),
        )
        .await?;
        let _action_id = weles_capture::run_action(
            &channel,
            APPLE_DEVELOPER_ID_ACTION,
            json!({
                // The Apple account is asked for by role; the worker selects
                // the Skarbiec item that plays it.
                "login_role": account_role,
                "apple_csr_path": format!("{work}/request.csr"),
                "apple_certificate_path": format!("{work}/certificate.cer"),
                "system_consent": "account-holder-2fa",
                "apple_auth_guard_id": guard_id,
                "apple_execution_host": target.name,
                "apple_execution_agent": execution_agent,
                "apple_login_capabilities": {
                    "email": email,
                    "password": password,
                    "two_factor": {
                        "mode": "capability",
                        "capability": two_factor,
                    },
                },
            }),
        )
        .await?;
        finish_output = host_channel::run_script(&target, &finish, &production_runner()).await?;
        if !finish_output.ok() {
            return Err(DeployError::unreachable(format!(
                "{}: Developer ID bundle export failed: {}",
                target.name,
                command_failure(&finish_output, "remote certificate export failed")
            )));
        }
    }

    let p12 = required_remote_file(&target, &format!("{work}/certificate.p12.b64")).await?;
    let password = required_remote_file(&target, &format!("{work}/certificate.password")).await?;
    let identity = required_remote_file(&target, &format!("{work}/certificate.identity")).await?;
    let not_after = required_remote_file(&target, &format!("{work}/certificate.not-after")).await?;
    crate::credential_store::owner::write_role_item(
        DEVELOPER_ID_ROLE,
        DEVELOPER_ID_KIND,
        &json!({
            "certificate_p12_base64": p12,
            "certificate_password": password,
            "sign_identity": identity,
            "not_after": not_after,
        }),
        &json!({
            "issuer": "Apple Developer",
            "purpose": "desktop-release-signing",
        }),
    )
    .map_err(DeployError::from)?;
    publish_developer_id_secrets(repositories, &p12, &password, &identity, &github_token)?;

    let cleanup = replace(
        "set -eu\nwork=__WORK_DIR__\nrm -f \"$work/private-key.pem\" \"$work/request.csr\" \"$work/certificate.cer\" \"$work/certificate.pem\" \"$work/certificate.p12\" \"$work/certificate.p12.b64\" \"$work/certificate.password\"\n",
        &[("__WORK_DIR__", shlex_quote(&work))],
    );
    let cleanup_output = host_channel::run_script(&target, &cleanup, &production_runner()).await?;
    if !cleanup_output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: certificate was stored but remote private material cleanup failed: {}",
            target.name,
            command_failure(&cleanup_output, "remote cleanup failed")
        )));
    }

    Ok(json!({
        "certificate_role": DEVELOPER_ID_ROLE,
        "identity": identity,
        "not_after": not_after,
        "repositories": repositories,
        "status": "issued",
        "target": target.name,
    }))
}
