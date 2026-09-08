//! The startup script a dispatched agent VM runs, rendered exactly as
//! dispatch renders it.

use crate::catalog::GPU_SIZING;
use crate::config;
use crate::coordinator;
use crate::doctor::{Check, Findings, Status};
use crate::scheduler::dispatch::agent;

// ---------------------------------------------------------------------------
// 6. Agent template render
// ---------------------------------------------------------------------------

pub(in crate::doctor) const TEMPLATE_ID: &str = "template";
pub(in crate::doctor) const TEMPLATE_TITLE: &str = "Agent template";
pub(in crate::doctor) const TEMPLATE_REMEDY: &str =
    "publish one immutable Python/model runtime bundle and configure \
     STADO_AGENT_RUNTIME_BUNDLE_URI + STADO_AGENT_RUNTIME_BUNDLE_SHA256; \
     configure deployment storage/backup and dedicated agent.skarbiec settings. \
     Every template must export the full scheduler-owned placeholder contract";

/// A representative accelerator for `provider`: the smallest VRAM tier of
/// its [`GPU_SIZING`] ladder. Deterministic (`BTreeMap` order) and real,
/// and its absence doubles as "this provider dispatches no agent VMs".
fn representative_accel(provider: &str) -> Option<&'static str> {
    GPU_SIZING
        .get(provider)?
        .values()
        .next()
        .map(|(_machine_type, accel)| *accel)
}

/// Render each configured provider's startup script exactly as dispatch
/// would and assert no `${PLACEHOLDER}` survives.
///
/// This is the check that would have caught the Azure cutover: the
/// template exports `${WC_STORAGE_BACKEND}` and friends under `set -u`,
/// no producer supplied them, and every dispatched VM aborted before the
/// agent started — billing for instances that ran nothing.
pub(in crate::doctor) async fn check_agent_template() -> Check {
    if config::wc_providers()
        .iter()
        .all(|provider| representative_accel(provider).is_none())
    {
        return Check::pass(
            TEMPLATE_ID,
            TEMPLATE_TITLE,
            "active providers dispatch no cloud agent VMs; no startup template credentials are \
             required"
                .to_string(),
            TEMPLATE_REMEDY,
        );
    }
    let secrets = match coordinator::secrets_from_skarbiec().await {
        Ok(secrets) => secrets,
        Err(err) => {
            return Check::fail(
                TEMPLATE_ID,
                TEMPLATE_TITLE,
                format!("cannot resolve template credentials from Skarbiec: {err}"),
                TEMPLATE_REMEDY,
            )
        }
    };
    let mut findings = Findings::default();
    for name in config::wc_providers() {
        let Some(accel) = representative_accel(name) else {
            findings.note(
                Status::Pass,
                format!("{name}: dispatches no agent VMs, no startup template to render"),
            );
            continue;
        };
        let Some(template) = agent::bundled_template_for(name) else {
            findings.note(
                Status::Fail,
                format!("{name}: no execution template registered in the capability catalog"),
            );
            findings.remedy(TEMPLATE_REMEDY);
            continue;
        };
        let deployment = agent::deployment_substitutions(name);
        match agent::render_agent_startup_script(name, template, accel, &secrets, &deployment) {
            Ok(script) => findings.note(
                Status::Pass,
                format!(
                    "{name}: rendered {} byte(s) with complete storage, scoped-grant, and \
                     immutable-runtime exports for {accel}",
                    script.len()
                ),
            ),
            Err(err) => {
                findings.note(Status::Fail, format!("{name} ({accel}): {err}"));
                findings.remedy(TEMPLATE_REMEDY);
            }
        }
    }
    findings.into_check(TEMPLATE_ID, TEMPLATE_TITLE, TEMPLATE_REMEDY)
}
