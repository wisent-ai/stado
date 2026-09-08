//! Downstream impact: one row per component the failed dependency reaches,
//! each with the data it needs, who consumes it, and why the state is what it
//! is. Severity is stated per component; nothing is collapsed into one verdict.

use crate::cli::blast_radius::{DownstreamImpact, StorageReport, REGISTRY};
use crate::config;
use crate::queue::copy::CANONICAL_PREFIXES;

use super::{dependency_owns_backend, dependency_owns_release};

pub(in crate::cli::blast_radius) fn downstream_impacts(
    dependency: &crate::capabilities::CapabilityVariant,
    primary: &StorageReport,
    backup: &StorageReport,
) -> Vec<DownstreamImpact> {
    let storage_hit = dependency_owns_backend(dependency, config::wc_storage_backend());
    let primary_down = primary.state != "reachable";
    let backup_readable = backup.state == "reachable";
    let storage_state = if !storage_hit {
        "unaffected"
    } else if !primary_down {
        "at_risk"
    } else if backup_readable {
        "blocked_backup_requires_explicit_promotion"
    } else {
        "blocked_no_readable_backup"
    };

    let mut impacts = vec![
        impact(
            "queue_store",
            "critical",
            storage_state,
            CANONICAL_PREFIXES,
            &[
                "coordinator",
                "scheduler",
                "workers",
                "dashboard",
                "desktop",
                "status/results/cancel",
                "machine API",
            ],
            "the mutable queue and every lifecycle transition use the configured storage backend",
        ),
        impact(
            "registry",
            "critical",
            storage_state,
            REGISTRY,
            &["coordinators", "host management"],
            "registry.json lives in the configured primary store; credentials live in the globally selected credential store",
        ),
    ];

    let provider_enabled = dependency.provider.is_some_and(|owner| {
        config::wc_providers()
            .iter()
            .any(|provider| owner.matches(provider))
    });
    impacts.push(impact(
        "compute_provider",
        "critical",
        if provider_enabled {
            "blocked_or_degraded"
        } else {
            "unaffected"
        },
        &[],
        &[
            "scheduler dispatch",
            "ephemeral GPU workers",
            "quota inspection",
        ],
        "configured compute providers are the scheduler's VM creation and lifecycle surface",
    ));

    let release_hit = dependency_owns_release(dependency, &config::stado_api_url());
    impacts.push(impact(
        "release_channel",
        "high",
        if release_hit { "blocked" } else { "unaffected" },
        &["releases/stado/"],
        &[
            "self update",
            "bootstrap",
            "new cloud workers",
            "host repair",
        ],
        "new processes need the binary and checksum channel even when existing processes still run",
    ));

    let pubsub_hit = dependency.provider == Some(crate::capabilities::ProviderId::Gcp)
        && !config::alerts_topic().is_empty();
    impacts.push(impact(
        "pubsub_alert_sink",
        "medium",
        if pubsub_hit { "degraded" } else { "unaffected" },
        &[],
        &["incident notifications"],
        "Pub/Sub is one alert sink; independently configured Slack, Telegram or SendGrid sinks can survive",
    ));

    impacts.push(impact(
        "provider_billing_visibility",
        "high",
        if dependency.provider == Some(crate::capabilities::ProviderId::Local) {
            "unaffected"
        } else {
            "degraded"
        },
        &["billing_health/"],
        &["overview", "billing watch", "credit depletion alerts"],
        "provider account and billing APIs become unreadable with the failed provider",
    ));

    impacts.push(impact(
        "supabase_deployment_metadata",
        "low",
        "unaffected",
        &[],
        &[
            "deployment list",
            "deployment grants",
            "infrastructure target metadata",
        ],
        "Supabase is outside the queue store and cloud-provider project",
    ));

    impacts
}

fn impact(
    component: &str,
    severity: &str,
    state: &str,
    data: &[&str],
    consumers: &[&str],
    reason: &str,
) -> DownstreamImpact {
    DownstreamImpact {
        component: component.to_string(),
        severity: severity.to_string(),
        state: state.to_string(),
        data: data.iter().map(|value| (*value).to_string()).collect(),
        consumers: consumers.iter().map(|value| (*value).to_string()).collect(),
        reason: reason.to_string(),
    }
}
