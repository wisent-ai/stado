//! `stado credentials seed-freshness --host TARGET` — is each login row's stored
//! authenticator seed still the one its account has enrolled?
//!
//! # Why the name, and why Stado owns it
//!
//! The answer is a join of two things neither of which can settle it alone, so
//! the command is named after what it reports — authenticator seed freshness —
//! rather than after Weles, which merely happens to have produced the log
//! lines.
//!
//! The vault knows whether a seed EXISTS. It cannot know whether that seed
//! still MATCHES an enrolment, because matching is only observable where a
//! computed code is submitted to a provider and accepted or refused. So
//! `skarbiec totp-seed-state` answers the vault's half — `present`,
//! `declared_empty`, `field_absent` — and deliberately stops there.
//!
//! The other half is written by the sign-in loop. Brama's journal
//! (`$HOME/.brama/journal.jsonl`) appends one `subscription_sign_in` record
//! per attempt carrying the exact `login_item`, the verdict, the instant, and
//! the trajectory's own tail — which is where the Google SSO driver's
//! `[google_sso] …` markers and Google's own sentences land. That is the run
//! history: per-account, timestamped, and the evidence nobody read for six
//! days while a stale seed was resubmitted every thirty minutes.
//!
//! Since one half lives in a vault and the other in a service's journal, the
//! join is fleet-level, and Stado is the only surface that reaches both. It
//! sits beside `weles-activity` and `weles-run-diagnostics`, which is where an
//! operator already looks when asking what the sign-in loop did.
//!
//! # Why it reads the host's own files rather than the Weles API
//!
//! `weles-activity` already reads the run store off the host filesystem and
//! only PROBES the worker API. That matters here: on 2026-09-02 the admission
//! unit on charless-mac-mini was crash-looping on
//! `ERR_MODULE_NOT_FOUND: Cannot find module …/dist/worker/dispatch.js`, so
//! every `weles-run-diagnostics` call failed — which is exactly the state in
//! which somebody asks this question. A diagnostic that depends on the thing
//! that is broken answers nothing.
//!
//! # What never crosses the channel
//!
//! Not the seed, not a password, not a one-time code, and not the journal's
//! raw `detail` — that field carries up to 1800 characters of trajectory tail
//! and rendered page text. The host-side reader matches a fixed marker
//! vocabulary and returns marker NAMES and counts only. Nothing here computes
//! a code; `skarbiec totp` is deliberately not the call this makes.
//!
//! # The components
//!
//! [`verdict`] is the decision and nothing else — the vault's vocabulary, one
//! reduced attempt, the eight outcomes and the classification between them.
//! [`remote`] performs the two read-only host reads that supply those two
//! halves, [`report`] joins them into one document and renders it, and
//! [`command`] is the operator-facing call. Every name this module exposed
//! before the split is re-exported here, so `crate::cli::seed_freshness::NAME`
//! still resolves.

mod command;
mod remote;
mod report;
mod verdict;

pub use crate::cli::seed_freshness::command::authenticator_seed_freshness;
pub use crate::cli::seed_freshness::report::join::attempts_of;
pub use crate::cli::seed_freshness::report::join::build_report;
pub use crate::cli::seed_freshness::verdict::classify::classify;
pub use crate::cli::seed_freshness::verdict::inputs::Attempt;
pub use crate::cli::seed_freshness::verdict::inputs::SEED_DECLARED_EMPTY;
pub use crate::cli::seed_freshness::verdict::inputs::SEED_FIELD_ABSENT;
pub use crate::cli::seed_freshness::verdict::inputs::SEED_PRESENT;
pub use crate::cli::seed_freshness::verdict::inputs::SEED_READ_UNSUPPORTED;
pub use crate::cli::seed_freshness::verdict::inputs::SEED_UNREADABLE;
pub use crate::cli::seed_freshness::verdict::outcome::Verdict;
