//! The release agent's unit-image revisit pass: put ONE declared launchd unit
//! per reconcile invocation back on the file it declares, and record what
//! happened.
//!
//! `registry doctor` sees a unit whose live process executes a replaced or
//! unlinked image (#336) and `stado service refresh-image` repairs one named
//! unit on demand (#344). Neither revisits a unit nobody typed a command for:
//! `self_update::recycle_replaced_units` cycles units only inside the
//! invocation that replaced their bytes, so one it misses stays missed —
//! `com.wisent.compute.disk-cleanup.disk-cleanup` journalled `policy:ValueError`
//! 8,348 times over thirteen days that way, and an unrelated restart ended it.
//! The installed binary moved from 0.13.50 to 0.14.8 inside one day, so the
//! condition regenerates faster than a per-unit manual verb clears it.
//!
//! Four bounds, each enforced in one named place:
//!
//! - **The `release_unit_image_revisit` registry block names exact labels for
//!   one host and is absent by default.** Per product AND per target, because
//!   a label is a fact about one machine: the same product's Linux target runs
//!   different units under different names, and a product-level list would
//!   have authorised one platform's labels on every platform. It is a
//!   TOP-LEVEL, unmodelled key rather than a `release_control` field so that
//!   older builds preserve and ignore it instead of refusing the whole
//!   document — see [`registry_policy::REVISIT_POLICY_KEY`]. [`policy`]
//!   returns `None` for an absent key before
//!   [`registry_policy::scope::host_scope`] is called. For a present block,
//!   `host_scope` answers `None` when no product names a label for this target.
//! - **One unit per reconcile invocation.** One scheduled tick is one
//!   invocation; [`plan::revisit_plan`] picks one and records the rest as
//!   [`plan::RevisitSkip::OneUnitPerTick`].
//! - **Never a unit that recycles itself.**
//!   `self_update::defers_to_release_handshake` is reused, not re-derived.
//! - **The identity is read again afterwards.** #344's
//!   [`crate::cli::service_refresh_image::refresh_outcome`] decides, and the
//!   attempt is recorded so it is not tried again on the same pair of
//!   identities.
//!
//! The contract for one host — which state directory holds the ledger, and
//! which product authorises which label on this target — is computed from
//! EVERY product in the policy before `--product` is applied, so concurrent
//! product-scoped agents share one lock and cannot both spend a restart on the
//! same unchanged identity pair.
//!
//! A non-blocking host lock covers observe → record → kickstart → settle →
//! record. What it prevents is OVERLAP: two reconciles running concurrently
//! would each observe the same stale unit against the same unchanged identity
//! pair and each spend a restart on it, neither having seen the other's
//! ledger write. It is not a rate limit and defines no time window.
//! Sequential invocations are separate ticks, and each may act on one unit —
//! a different one, because the unit a tick handled is afterwards either on
//! its declared file or barred by its own record.
//!
//! **The attempt is written before the restart, not after.** A record written
//! only once the outcome is known is lost by any crash or write failure in
//! between, and the next tick then kickstarts the same unit again — the hot
//! loop every other bound here exists to prevent, reappearing exactly when the
//! host is already unhealthy. So an
//! [`ledger::identity::AttemptOutcome::Attempting`] record is committed first
//! and the side effect is refused if that write fails; the observed or refused
//! result then replaces it. A record left at `Attempting`
//! is a recorded INTENT with no result beside it: the pass stopped somewhere
//! between committing that intent and writing down what happened, so whether
//! `launchctl` was ever invoked is unknown. It bars the same identity pair for
//! exactly that reason, and says so on the `registry doctor` row.

pub(crate) mod ledger;
pub(crate) mod pass;
pub(crate) mod plan;
pub(crate) mod registry_policy;

// `release_agent::tick::once` names all three of these at
// `crate::release_unit_image::<name>`, and
// `registry::doctor::checks::target` names `annotations`;
// `targets::validation_registry` names `validate_registry_contract`.
pub(crate) use pass::annotate::annotations;
pub(crate) use pass::revisit_once;
pub(crate) use registry_policy::contract::validate_registry_contract;
pub(crate) use registry_policy::policy;
