//! The verdict of one check, and the absence of one.

/// Verdict of one check — or, for [`Status::Unmeasured`], the absence of a
/// verdict.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing to do.
    #[default]
    Pass,
    /// The probe never answered, so this check says nothing about the world.
    ///
    /// Not a verdict. A probe that ran out of its budget used to be a FAIL,
    /// and on 2026-09-03 that made `doctor` report six failures of which four
    /// were 8- and 24-second timeouts under load the doctor itself was
    /// generating — the same three checks passed five minutes later. An
    /// operator who is shown four wolves learns to ignore the shepherd, and
    /// "the probe did not answer" is a statement about the probe, never about
    /// the deployment. This is the `absent` versus `unreachable` distinction
    /// this fleet already treats as load-bearing everywhere else.
    Unmeasured,
    /// Works, but a documented hazard is live.
    Warn,
    /// Blocking: the deployment cannot do its job in this state.
    Fail,
}

impl Status {
    /// Table rendering.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Unmeasured => "UNMEASURED",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }

    /// `--json` rendering.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Unmeasured => "unmeasured",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }

    /// The more severe of two verdicts. Lets a check that inspects several
    /// providers reach one verdict without ranking numbers.
    ///
    /// `Unmeasured` outranks `Pass` and nothing else: an incomplete sweep must
    /// not read as clean, and must not read as broken either.
    pub const fn worst(self, other: Self) -> Self {
        match (self, other) {
            (Self::Fail, _) | (_, Self::Fail) => Self::Fail,
            (Self::Warn, _) | (_, Self::Warn) => Self::Warn,
            (Self::Unmeasured, _) | (_, Self::Unmeasured) => Self::Unmeasured,
            _ => Self::Pass,
        }
    }

    /// Whether this row is a statement about the deployment at all.
    pub const fn is_verdict(self) -> bool {
        !matches!(self, Self::Unmeasured)
    }
}
