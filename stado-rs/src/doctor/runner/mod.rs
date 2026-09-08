//! The command itself: every selected probe, run concurrently under its own
//! deadline, assembled into one ordered report.

use chrono::{SecondsFormat, Utc};

use self::bounds::{
    agent_skarbiec_deadline, alerts_deadline, object_auth_deadline, registry_probe_deadline,
    selected, selected_within, storage_round_trip_deadline,
};
use crate::doctor::fleet::credentials::agent::{
    check_agent_skarbiec, AGENT_SKARBIEC_ID, AGENT_SKARBIEC_REMEDY, AGENT_SKARBIEC_TITLE,
};
use crate::doctor::fleet::credentials::contract::{
    skarbiec_contract_check, CONTRACT_ID, CONTRACT_REMEDY, CONTRACT_TITLE,
};
use crate::doctor::fleet::credentials::providers::{
    check_provider_auth, check_vm_identity, IDENTITY_ID, IDENTITY_REMEDY, IDENTITY_TITLE,
    PROVIDERS_ID, PROVIDERS_REMEDY, PROVIDERS_TITLE,
};
use crate::doctor::fleet::credentials::vault::{
    check_owner_vault, OWNER_VAULT_ID, OWNER_VAULT_REMEDY, OWNER_VAULT_TITLE,
};
use crate::doctor::fleet::hosts::placement::{
    check_placement, PLACEMENT_ID, PLACEMENT_REMEDY, PLACEMENT_TITLE,
};
use crate::doctor::fleet::hosts::registry::{
    check_registry, REGISTRY_ID, REGISTRY_REMEDY, REGISTRY_TITLE,
};
use crate::doctor::fleet::hosts::shape::{
    check_fleet_shape, FLEET_SHAPE_DEADLINE, SHAPE_ID, SHAPE_REMEDY, SHAPE_TITLE,
};
use crate::doctor::fleet::releases::channel::{
    check_release_channel, RELEASE_ID, RELEASE_REMEDY, RELEASE_TITLE,
};
use crate::doctor::fleet::releases::integrity::{
    check_release_integrity, INTEGRITY_DEADLINE, INTEGRITY_ID, INTEGRITY_REMEDY, INTEGRITY_TITLE,
};
use crate::doctor::fleet::units::alerts::{check_alerts, ALERTS_ID, ALERTS_REMEDY, ALERTS_TITLE};
use crate::doctor::fleet::units::queue::{
    check_queue_control, CONTROL_ID, CONTROL_REMEDY, CONTROL_TITLE,
};
use crate::doctor::fleet::units::template::{
    check_agent_template, TEMPLATE_ID, TEMPLATE_REMEDY, TEMPLATE_TITLE,
};
use crate::doctor::plane::config::{check_config, CONFIG_ID, CONFIG_REMEDY, CONFIG_TITLE};
use crate::doctor::plane::object_auth::{
    check_object_auth, OBJECT_AUTH_ID, OBJECT_AUTH_REMEDY, OBJECT_AUTH_TITLE,
};
use crate::doctor::plane::quota::{check_quota, QUOTA_ID, QUOTA_REMEDY, QUOTA_TITLE};
use crate::doctor::plane::storage::backup::{check_backup, BACKUP_ID, BACKUP_REMEDY, BACKUP_TITLE};
use crate::doctor::plane::storage::round_trip::{
    check_storage_round_trip, STORAGE_ID, STORAGE_REMEDY, STORAGE_TITLE,
};
use crate::doctor::{Report, RunScope};
use crate::queue::JobStorage;

mod bounds;

/// Run the selected preflight probes. Never returns an error: an unreachable
/// dependency is a FAIL row, not an aborted command.
pub async fn run(scope: RunScope) -> Report {
    // One facade for every selected store-backed probe. Its construction
    // failure is itself diagnostic — on the azure backend with an empty
    // account `JobStorage::with_bucket` hard-errors — so each dependent check
    // reports it instead of the whole command dying here. Exact release
    // verification does not construct storage at all.
    let store_result = if scope == RunScope::ReleaseVerification {
        None
    } else {
        Some(JobStorage::new().await)
    };
    let store_error = store_result
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .map(ToString::to_string)
        .unwrap_or_default();
    let store = store_result
        .as_ref()
        .and_then(|result| result.as_ref().ok());

    // Concurrent, like the two sections of
    // `monitor::billing::live_snapshot`; the fixed assembly order below is
    // what makes the report ordered.
    let (
        config_check,
        storage_check,
        backup_check,
        object_auth_check,
        providers_check,
        quota_check,
        release_check,
        integrity_check,
        template_check,
        agent_skarbiec_check,
        owner_vault_check,
        identity_check,
        registry_check,
        control_check,
        alerts_check,
        contract_check,
        placement_check,
        shape_check,
    ) = tokio::join!(
        selected(scope, CONFIG_ID, CONFIG_TITLE, CONFIG_REMEDY, async {
            check_config()
        }),
        selected_within(
            scope,
            storage_round_trip_deadline(),
            STORAGE_ID,
            STORAGE_TITLE,
            STORAGE_REMEDY,
            check_storage_round_trip(store, &store_error),
        ),
        selected(
            scope,
            BACKUP_ID,
            BACKUP_TITLE,
            BACKUP_REMEDY,
            check_backup(&store_error),
        ),
        selected_within(
            scope,
            object_auth_deadline(),
            OBJECT_AUTH_ID,
            OBJECT_AUTH_TITLE,
            OBJECT_AUTH_REMEDY,
            check_object_auth(),
        ),
        selected(
            scope,
            PROVIDERS_ID,
            PROVIDERS_TITLE,
            PROVIDERS_REMEDY,
            check_provider_auth(),
        ),
        selected(
            scope,
            QUOTA_ID,
            QUOTA_TITLE,
            QUOTA_REMEDY,
            check_quota(store, &store_error),
        ),
        selected(
            scope,
            RELEASE_ID,
            RELEASE_TITLE,
            RELEASE_REMEDY,
            check_release_channel(),
        ),
        selected_within(
            scope,
            INTEGRITY_DEADLINE,
            INTEGRITY_ID,
            INTEGRITY_TITLE,
            INTEGRITY_REMEDY,
            check_release_integrity(),
        ),
        selected(scope, TEMPLATE_ID, TEMPLATE_TITLE, TEMPLATE_REMEDY, async {
            check_agent_template().await
        },),
        selected_within(
            scope,
            agent_skarbiec_deadline(),
            AGENT_SKARBIEC_ID,
            AGENT_SKARBIEC_TITLE,
            AGENT_SKARBIEC_REMEDY,
            async { check_agent_skarbiec().await },
        ),
        selected(
            scope,
            OWNER_VAULT_ID,
            OWNER_VAULT_TITLE,
            OWNER_VAULT_REMEDY,
            async { check_owner_vault() },
        ),
        selected(scope, IDENTITY_ID, IDENTITY_TITLE, IDENTITY_REMEDY, async {
            check_vm_identity()
        },),
        selected_within(
            scope,
            registry_probe_deadline(),
            REGISTRY_ID,
            REGISTRY_TITLE,
            REGISTRY_REMEDY,
            check_registry(),
        ),
        selected(
            scope,
            CONTROL_ID,
            CONTROL_TITLE,
            CONTROL_REMEDY,
            check_queue_control(store, &store_error),
        ),
        selected_within(
            scope,
            alerts_deadline(),
            ALERTS_ID,
            ALERTS_TITLE,
            ALERTS_REMEDY,
            check_alerts(),
        ),
        selected(
            scope,
            CONTRACT_ID,
            CONTRACT_TITLE,
            CONTRACT_REMEDY,
            skarbiec_contract_check(),
        ),
        selected(
            scope,
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            PLACEMENT_REMEDY,
            check_placement(),
        ),
        selected_within(
            scope,
            FLEET_SHAPE_DEADLINE,
            SHAPE_ID,
            SHAPE_TITLE,
            SHAPE_REMEDY,
            check_fleet_shape(),
        ),
    );

    let mut checks = vec![
        config_check,
        storage_check,
        backup_check,
        object_auth_check,
        providers_check,
        quota_check,
        release_check,
        integrity_check,
        template_check,
        agent_skarbiec_check,
        owner_vault_check,
        identity_check,
        registry_check,
        control_check,
        alerts_check,
        contract_check,
        placement_check,
        shape_check,
    ];
    checks.retain(|check| scope.includes(check.id));

    Report {
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        // Preflight order: configuration, then the store everything else
        // reads, then credentials, then capacity, then the two things an
        // agent VM needs in order to exist at all, then fleet identity,
        // then the switches that explain an idle-but-healthy deployment.
        checks,
    }
}
