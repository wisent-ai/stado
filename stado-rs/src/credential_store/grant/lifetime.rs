//! How long a Skarbiec grant lives, read from the grant and handed back to
//! `skarbiec grant issue` unchanged.
//!
//! Stado states no lifetime of its own. A grant it issues for a consumer it
//! keeps alive lives until `grant revoke` withdraws it: re-issuing one with
//! the same bearer to push an expiry forward protects nothing a revocation
//! does not, and a missed renewal is an outage. A grant it re-issues keeps
//! the lifetime the vault records for it, so a widening never extends a grant
//! that was given an end and never ends one that was given none.

/// The expiry Skarbiec stores for a grant issued `--until-revoked`, and for a
/// migrated grant that never had one: no clock reaches it.
const UNTIL_REVOKED_EXPIRY: u64 = u64::MAX;

/// A grant's lifetime as `skarbiec grant issue` takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantLifetime {
    /// Until `grant revoke` withdraws it.
    UntilRevoked,
    /// This many seconds from the issue.
    Seconds(u64),
}

impl GrantLifetime {
    /// What a grant recorded with `expires_at` has left at `now`, or `None`
    /// once it has ended.
    pub fn left(expires_at: u64, now: u64) -> Option<Self> {
        if expires_at == UNTIL_REVOKED_EXPIRY {
            return Some(Self::UntilRevoked);
        }
        (expires_at > now).then(|| Self::Seconds(expires_at - now))
    }

    /// The lifetime a `--ttl-seconds` flag states: until revoked when the
    /// flag is absent, `None` for zero, which states no lifetime at all.
    pub fn stated(ttl_seconds: Option<u64>) -> Option<Self> {
        match ttl_seconds {
            None => Some(Self::UntilRevoked),
            Some(0) => None,
            Some(seconds) => Some(Self::Seconds(seconds)),
        }
    }
    /// Whether a grant recorded with `expires_at` lives until revoked.
    pub fn is_until_revoked(expires_at: u64) -> bool {
        expires_at == UNTIL_REVOKED_EXPIRY
    }

    /// The `grant issue` arguments that state this lifetime.
    pub fn args(self) -> Vec<String> {
        match self {
            Self::UntilRevoked => vec!["--until-revoked".to_string()],
            Self::Seconds(seconds) => vec!["--ttl-seconds".to_string(), seconds.to_string()],
        }
    }

    /// The same arguments as one shell fragment; neither form carries a
    /// character a shell would read.
    pub fn shell(self) -> String {
        self.args().join(" ")
    }
}

impl std::fmt::Display for GrantLifetime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UntilRevoked => formatter.write_str("until revoked"),
            Self::Seconds(seconds) => write!(formatter, "for {seconds} more seconds"),
        }
    }
}
