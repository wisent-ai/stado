//! Automated responder for Open Azure quota support tickets.
//!
//! Port of `stado/scheduler/dispatch/quota_replies.py`.
//!
//! Microsoft Capacity CX opens a support ticket for every Azure quota
//! increase and follows up with the same five-question template (Region /
//! Deployment Model / Service Type / Planned VM Families / Planned
//! Compute Usage in Cores). When the customer does not reply within a few
//! days, Microsoft archives the ticket and the quota request is silently
//! dropped. This module scans Open quota tickets in the configured
//! subscription and posts a single canonical reply per ticket so the
//! request progresses without manual triage.
//!
//! Uses Azure Resource Manager directly with the managed-identity or
//! `stado-azure` Skarbiec credential chain. No Azure CLI login or local token
//! cache is a credential source.
//!
//! The reply only fires when:
//!   - ticket.status == "Open"
//!   - the most recent communication is FROM Microsoft (sender domain
//!     contains "@techsupport.microsoft.com" or "@microsoft.com"),
//!     i.e. the customer has not already replied,
//!   - the ticket is a quota-classification (problemClassification
//!     contains "Quota" or "subscription limit").
//!
//! Dry-run prints the (ticket, region, planned body length) and skips
//! the create_communication call.

mod patterns;
mod respond;
mod runner;
mod tickets;

/// `respond_to_open_quota_tickets` is called twice by
/// `crate::cli::quota::submit` — once for the reply arm, once for the
/// escalate arm. `reply_body` and `escalation_body` are the two bodies it
/// renders and were published by the pre-image at the same paths.
pub use respond::{escalation_body, reply_body, respond_to_open_quota_tickets};
/// `RepliesError` is the error `crate::cli::quota::submit`'s
/// `support_permission_error` takes by reference; `SystemAzRunner` is the
/// production runner `crate::cli::quota::report` and
/// `crate::cli::quota::submit::increase` construct and hand in; and
/// `AzRunner` is the seam every re-exported signature below carries a
/// `&dyn` of, so it has to stay nameable alongside them. All three keep
/// their published `dispatch::quota_replies::` paths.
pub use runner::{AzRunner, RepliesError, SystemAzRunner};
/// `list_open_azure_tickets` is named by `crate::cli::quota::report`,
/// whose `stado quota requests` handler filters its rows.
/// `last_communication_is_from_ms` and `region_from_title` are the
/// per-ticket predicates the pre-image published beside it.
pub use tickets::{last_communication_is_from_ms, list_open_azure_tickets, region_from_title};
