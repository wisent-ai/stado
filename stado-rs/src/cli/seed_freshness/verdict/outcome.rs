//! The verdict itself: its code on the wire, whether it asks an operator for
//! anything, and the repair it names.

/// The exact operator path that stores a new seed, as
/// `skarbiec/scripts/store-login-totp-seed.sh` documents itself: the seed
/// arrives on standard input, never in an argument, because an authenticator
/// secret on a command line is a secret in every process table on the host.
const SEED_REPAIR_COMMAND: &str = "printf '%s' '<seed from the authenticator app>' \
     | ACCOUNT=<login-item> skarbiec/scripts/store-login-totp-seed.sh";

/// The verdict for one login row. Six outcomes, because collapsing any two of
/// them would name the wrong repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Seed present, and the newest attempt that actually submitted a code was
    /// not refused. The seed matched an enrolment as recently as this instant.
    LastKnownGood { at: String },
    /// Seed present, and every attempt that submitted a code since this
    /// instant was refused. The stored seed no longer matches the enrolment.
    RejectedSince {
        since: String,
        attempts: usize,
        locked_out: bool,
    },
    /// Seed present, but nothing has ever submitted a code from it, so its
    /// freshness is untested rather than good.
    PresentUntested,
    /// Seed present and sign-ins are failing, but never at the authenticator
    /// step. Not a seed condition: do not re-enrol on this verdict.
    PresentFailingElsewhere { attempts: usize },
    /// The row's kind declares `totp_secret` and it carries nothing usable.
    FieldEmpty,
    /// The row's kind has no `totp_secret` field at all.
    FieldAbsent,
    /// The vault could not open the row; nothing about the seed is known.
    VaultRowUnreadable,
    /// This host's Skarbiec cannot be asked for seed state at all.
    VaultReadUnsupported,
}

impl Verdict {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LastKnownGood { .. } => "seed_last_known_good",
            Self::RejectedSince { .. } => "seed_rejected_since",
            Self::PresentUntested => "seed_present_untested",
            Self::PresentFailingElsewhere { .. } => "seed_present_failing_elsewhere",
            Self::FieldEmpty => "seed_field_empty",
            Self::FieldAbsent => "seed_field_absent",
            Self::VaultRowUnreadable => "vault_row_unreadable",
            Self::VaultReadUnsupported => "vault_read_unsupported",
        }
    }

    /// Whether an operator has to do something. `PresentUntested` is not a
    /// fault; `PresentFailingElsewhere` is a fault whose repair is elsewhere.
    pub fn needs_reenrolment(&self) -> bool {
        matches!(self, Self::RejectedSince { .. } | Self::FieldEmpty)
    }

    /// The repair, naming the exact command where one exists.
    pub fn repair(&self, login_item: &str) -> String {
        match self {
            Self::RejectedSince { locked_out, .. } => {
                let lockout = if *locked_out {
                    " Google has locked the authenticator method on this account, so re-enrolment \
                     has to wait for that lockout to clear."
                } else {
                    ""
                };
                format!(
                    "re-enrol Google Authenticator on this account, then store the new seed: {}.{}",
                    SEED_REPAIR_COMMAND.replace("<login-item>", login_item),
                    lockout
                )
            }
            Self::FieldEmpty => format!(
                "this row declares totp_secret and carries nothing; enrol Google Authenticator \
                 and store the seed: {}",
                SEED_REPAIR_COMMAND.replace("<login-item>", login_item)
            ),
            Self::FieldAbsent => String::from(
                "this row's kind declares no totp_secret field, so no sign-in of it can answer an \
                 authenticator prompt; store the account as a `login` item before storing a seed",
            ),
            Self::PresentFailingElsewhere { .. } => String::from(
                "sign-ins are failing before the authenticator step, so the seed is not the \
                 condition; read the attempt markers and repair that cause instead",
            ),
            Self::VaultRowUnreadable => String::from(
                "the vault could not open this row; repair vault access before judging its seed",
            ),
            Self::VaultReadUnsupported => String::from(
                "this host's Skarbiec has no `totp-seed-state`, so only the sign-in history \
                 half of the verdict is available; release a Skarbiec carrying it to complete it",
            ),
            Self::LastKnownGood { .. } | Self::PresentUntested => String::new(),
        }
    }
}
