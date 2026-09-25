//! Which received mail the billing watch reads as a provider notice.
//!
//! Providers announce closure, failed payment and credit expiry by email
//! days before the API starts refusing calls. Every message from a
//! provider's sender domain inside the window is read; the analysis in
//! `crate::mail` then says which of them ask for action.

pub(super) const SENDER_DOMAINS: &[&str] = &[
    "microsoft.com",
    "azure.microsoft.com",
    "google.com",
    "googlecloud.com",
    "payments-noreply.google.com",
];

/// Two weeks covers the notice period providers give before suspending an
/// account; an older notice has either been acted on or already shows up as
/// a refused API call the watch reads directly.
pub(super) const NEWER_THAN_DAYS: i64 = 14;

/// How many of Skrzynka's newest messages, across every enabled mailbox, one
/// sweep reads before filtering: a fortnight of an operator's inbox with room
/// to spare.
pub(super) const MESSAGES_READ: usize = 500;
