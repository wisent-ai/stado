//! The context one recommendation is built in, and the record it becomes.
//!
//! `RecommendationContext` carries the snapshot, the policy and the pass
//! clock, `recommendation` turns one finding into an authorized or blocked
//! decision record, and `deterministic_id` derives the identity that makes
//! republishing the same advice idempotent.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::autonomy::model::{
    DecisionKind, DecisionRecord, InventorySnapshot, ResourceRecord, SCHEMA_VERSION,
};
use crate::autonomy::policy::{ActionRisk, AutonomyPolicy};

pub(super) struct RecommendationContext<'a> {
    pub(super) snapshot: &'a InventorySnapshot,
    pub(super) policy: &'a AutonomyPolicy,
    pub(super) now: DateTime<Utc>,
}

impl RecommendationContext<'_> {
    pub(super) fn recommendation(
        &self,
        resource: &ResourceRecord,
        kind: DecisionKind,
        selected: serde_json::Value,
        explanation: &str,
        risk: ActionRisk,
    ) -> DecisionRecord {
        let authorization = self.policy.authorize(
            resource,
            risk,
            self.snapshot.complete,
            resource.current_hourly_cost_usd,
        );
        let expires =
            self.now + chrono::Duration::seconds(self.policy.limits.decision_ttl_seconds as i64);
        DecisionRecord {
            schema_version: SCHEMA_VERSION,
            decision_id: deterministic_id(resource, self.policy, self.snapshot, kind),
            kind,
            subject_id: resource.resource_id.clone(),
            created_at: self.now.to_rfc3339(),
            expires_at: expires.to_rfc3339(),
            inventory_snapshot_id: self.snapshot.snapshot_id.clone(),
            policy_version: self.policy.policy_version.clone(),
            selected: Some(selected),
            candidates: Vec::new(),
            constraints: vec![authorization.reason.clone()],
            explanation: explanation.to_string(),
            lease_token: None,
            state: if authorization.allowed {
                "authorized_recommendation".to_string()
            } else {
                "blocked_recommendation".to_string()
            },
        }
    }
}

fn deterministic_id(
    resource: &ResourceRecord,
    policy: &AutonomyPolicy,
    snapshot: &InventorySnapshot,
    kind: DecisionKind,
) -> String {
    let payload = format!(
        "{}|{}|{}|{:?}",
        resource.resource_id, policy.policy_version, snapshot.snapshot_id, kind
    );
    format!("advice-{}", hex::encode(Sha256::digest(payload.as_bytes())))
}
