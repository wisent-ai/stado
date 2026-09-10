use crate::targets::*;

// ---------------------------------------------------------------------------
// build versus registry — a build that refuses the document being published
// ---------------------------------------------------------------------------

/// Whether one host's installed build accepts the registry the control plane
/// publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildVerdict {
    /// The build was asked and refused. The refusal is a [`LastGoodRefusal`]
    /// rather than a sentence of its own: the same validator produced it, and
    /// an operator comparing this row with `resolver status` must not have to
    /// map two vocabularies onto one fault.
    Refused(LastGoodRefusal),
    /// The build could not be asked. A finding and never a silence, on the
    /// same rule [`crate::deploy::service::ImageState::Unread`] states: the
    /// defect this check exists to remove is an unread state rendered as a
    /// passing one, and a host silently omitted would be that defect wearing
    /// this check's name.
    Unmeasured {
        /// Why, named the way an operator would name it.
        reason: String,
    },
}

/// One host whose installed build was held against the published registry.
///
/// Only refusals and unmeasured hosts are built: a build that accepts the
/// document has nothing to report.
///
/// This is the condition that opened both silent windows and that nothing
/// reported. On `lukasz-macbook` the disk janitor journalled 8,348
/// `policy:ValueError` passes across two windows — 2026-08-20T20:18:05Z to
/// 2026-08-27T18:03:59Z and 2026-08-31T06:30:49Z to 2026-09-02T17:50:40Z —
/// while the registry was valid the whole way through and the running build
/// was too old to accept it. Neither window opened on a restart or a binary
/// replacement, so [`crate::deploy::service::StaleUnitImage`] fires nothing:
/// the installed file and the running image agreed and the REGISTRY was what
/// moved. Measured on 2026-09-03, 0.7.14 through 0.7.22 refuse today's
/// document over `disk_cleanup.cleaners`, 0.13.24 refuses it over the
/// `disk_cleanup` key set, and 0.13.46 onward accept it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRegistrySkew {
    pub host: String,
    /// The build that answered, or that would have had to. `None` on a host
    /// this process cannot ask, because the version there is not knowable
    /// from here either.
    pub build: Option<String>,
    pub verdict: BuildVerdict,
}

impl BuildRegistrySkew {
    /// Stable machine-readable category, in `registry doctor`'s vocabulary.
    pub fn kind(&self) -> &'static str {
        match self.verdict {
            BuildVerdict::Refused(_) => "build-refuses-registry",
            BuildVerdict::Unmeasured { .. } => "unread-build-verdict",
        }
    }

    /// The row an operator reads. It names the host, the build and the
    /// validator's own words, because "incompatible" sends somebody back to
    /// run the validator by hand to re-derive the one fact the check already
    /// holds.
    pub fn sentence(&self) -> String {
        match &self.verdict {
            BuildVerdict::Refused(refusal) => format!(
                "the build installed on {} ({}) refuses the registry this control plane publishes \
                 ({}): {refusal}. The document is not the fault — the age of that binary is, and \
                 every policy this build resolves out of the registry fails while it runs. \
                 Upgrading the build on {} is what clears it; republishing the registry does not",
                self.host,
                self.build.as_deref().unwrap_or("an unread version"),
                refusal.kind(),
                self.host,
            ),
            BuildVerdict::Unmeasured { reason } => format!(
                "whether the build installed on {} accepts the registry this control plane \
                 publishes could not be measured, and is NOT reported as acceptance: {reason}",
                self.host
            ),
        }
    }
}

/// The version of the build running this process, which is the only build any
/// process can answer for.
pub fn running_build() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Whether THIS build accepts `document`, the registry the authority is
/// publishing right now.
///
/// The same gate [`store_last_good`] holds the recovery copy to, reported
/// through the same [`LastGoodRefusal`] vocabulary. It is a direct call and
/// not a read of [`last_good_refusal`] on purpose: that record exists only if
/// this process happened to take the uncached authority path, and a check
/// that fires only when the cache was refreshed cannot fire in the case it
/// was written for.
///
/// `None` means this build accepts the document. Nothing is recorded here —
/// judging a document is not the same as caching one, and this must be safe
/// to call from a reporting surface.
pub fn build_refusal(document: &Value) -> Option<LastGoodRefusal> {
    validate_registry(document)
        .err()
        .map(|error| LastGoodRefusal::RejectedByThisBuild {
            detail: error.to_string(),
        })
}

/// Every declared machine held against the published document, worst first
/// in the only order that is knowable: this host measured, every other host
/// unmeasured.
///
/// LOCAL ONLY, and the signature says so — for the reason
/// [`crate::deploy::service::observe_unit_images`] states about pids, applied
/// to builds. Which document a build accepts is a question only that build
/// can answer: the beacon publishes one `state` word per unit and carries no
/// version, and nothing in the store records a host's installed build against
/// a registry generation. So `local_host` is the name of the host this
/// process runs on, and every other machine gets one row saying its build was
/// not asked.
///
/// A host that accepts yields nothing. Dispatcher pools are the caller's to
/// skip: `kind="gcp"` and `kind="vast"` name no machine with a build on it.
pub fn builds_refusing_registry(
    host: &str,
    document: &Value,
    local_host: Option<&str>,
) -> Vec<BuildRegistrySkew> {
    if local_host != Some(host) {
        return vec![BuildRegistrySkew {
            host: host.to_string(),
            build: None,
            verdict: BuildVerdict::Unmeasured {
                reason: format!(
                    "which registry document a build accepts is readable only by running that \
                     build, and this command is running {} on {}; {host} is unmeasured until \
                     `stado registry doctor` runs there",
                    running_build(),
                    local_host.unwrap_or("a host no registry target names"),
                ),
            },
        }];
    }
    build_refusal(document)
        .map(|refusal| BuildRegistrySkew {
            host: host.to_string(),
            build: Some(running_build().to_string()),
            verdict: BuildVerdict::Refused(refusal),
        })
        .into_iter()
        .collect()
}
