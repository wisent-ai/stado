//! Which received mail the billing watch reads as a provider notice.
//!
//! Providers announce closure, failed payment and credit expiry by email
//! days before the API starts refusing calls. Every message from a
//! provider's sender domain inside the window the caller states
//! (`--mail-days`) is read; the analysis in `crate::mail` then says which of
//! them ask for action.

pub(super) const SENDER_DOMAINS: &[&str] = &[
    "microsoft.com",
    "azure.microsoft.com",
    "google.com",
    "googlecloud.com",
    "payments-noreply.google.com",
];
