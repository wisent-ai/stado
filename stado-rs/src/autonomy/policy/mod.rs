//! Versioned autonomy policy and fail-closed mutation authorization.
//!
//! The components are the seams this file already carried: `knobs` holds the
//! risk vocabulary and the spend, placement and freshness dials, `limits`
//! holds the idle thresholds and the safety brake with the constants their
//! defaults derive from, `rules` holds one resource rule — the selector and
//! the grants it carries — and `document` is the policy document those
//! groups compose, together with the validation and the authorization ladder
//! asked of it. Every name a caller outside this module uses is re-exported
//! here, so `crate::autonomy::policy::<item>` resolves exactly as before,
//! and every field name is a published document key.

mod document;
mod knobs;
mod limits;
mod rules;

pub use document::{AuthorizationDecision, AutonomyPolicy};
pub use knobs::{ActionRisk, AutonomyMode, BudgetPolicy, FreshnessPolicy, PlacementPolicy};
pub use limits::{IdlePolicy, SafetyLimits};
pub use rules::ResourceRule;
