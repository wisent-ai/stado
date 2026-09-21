//! Which host may run a job that names a platform.
//!
//! This table lived beside the build recipes until they were removed on
//! 2026-09-21, and it went with them — but it was never about recipes. The
//! claiming agent reads it on every job (`providers::local::helpers::claims`)
//! and the managed-service model reads it to decide whether a target carries
//! the macOS agents, so removing it left the product unable to compile at all.
//! It lives on its own now, where nothing that is deleted can take it along.

/// One release platform's job-routing coordinates: every spelling of its
/// operating system and architecture the fleet writes down, canonical first.
///
/// Two spellings for one machine is not a hypothetical: enrollment reads
/// `uname` (`Darwin`/`arm64`), Rust reads `std::env::consts` (`macos`/
/// `aarch64`), the sandbox box declares `x86_64` and the cloud optimizer
/// accepts `amd64`. A router that knows one of them refuses jobs that name
/// the same host by another word, so every spelling this repository writes
/// belongs in one table rather than at each comparison.
struct PlatformRouting {
    platform: &'static str,
    /// `Job::platform_os` spellings; the first is what a job declares.
    os: &'static [&'static str],
    /// `Job::architecture` spellings; the first is what a job declares.
    arch: &'static [&'static str],
}

/// The platform words of [`crate::deploy::products::PLATFORMS`], each with
/// the job fields that route work to it.
const PLATFORM_ROUTING: [PlatformRouting; 2] = [
    PlatformRouting {
        platform: "darwin-arm64",
        os: &["darwin", "macos"],
        arch: &["arm64", "aarch64"],
    },
    PlatformRouting {
        platform: "linux-amd64",
        os: &["linux"],
        arch: &["amd64", "x86_64"],
    },
];

/// A platform the installer knows but nothing can route work to is a job no
/// host will ever claim. Fail the build of whoever adds the platform word
/// instead.
const _: () = assert!(PLATFORM_ROUTING.len() == crate::deploy::products::PLATFORMS.len());

fn platform_routing(platform: &str) -> Option<&'static PlatformRouting> {
    PLATFORM_ROUTING
        .iter()
        .find(|entry| entry.platform == platform)
}

/// The `(platform_os, architecture)` a job must declare to be routed to
/// `platform`, or `None` when the word is not a release platform.
pub fn platform_job_os_arch(platform: &str) -> Option<(&'static str, &'static str)> {
    let routing = platform_routing(platform)?;
    Some((routing.os[0], routing.arch[0]))
}

/// Whether a host running `platform` may run a job declaring `platform_os`
/// and `architecture`.
///
/// An empty job field is no constraint — every job submitted before platform
/// routing existed carries two of them, and they stay claimable everywhere.
/// An unknown `platform` accepts nothing constrained: a host the release
/// pipeline does not publish for cannot be the intended target of a job that
/// names a platform.
pub fn platform_accepts_job(platform: &str, platform_os: &str, architecture: &str) -> bool {
    if platform_os.is_empty() && architecture.is_empty() {
        return true;
    }
    let Some(routing) = platform_routing(platform) else {
        return false;
    };
    let names = |spellings: &[&str], value: &str| {
        value.is_empty()
            || spellings
                .iter()
                .any(|spelling| spelling.eq_ignore_ascii_case(value))
    };
    names(routing.os, platform_os) && names(routing.arch, architecture)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same machine is written down four ways across this repository, and
    /// a host must claim its own work under every one of them.
    #[test]
    fn a_host_claims_its_own_work_under_every_spelling() {
        assert!(platform_accepts_job("darwin-arm64", "Darwin", "arm64"));
        assert!(platform_accepts_job("darwin-arm64", "macos", "aarch64"));
        assert!(platform_accepts_job("linux-amd64", "linux", "x86_64"));
        assert!(platform_accepts_job("linux-amd64", "Linux", "amd64"));
    }

    /// A job that names another platform, or a platform nothing publishes
    /// for, is not this host's work. A job that names neither is everyone's.
    #[test]
    fn work_for_another_platform_is_refused_and_unconstrained_work_is_not() {
        assert!(!platform_accepts_job("darwin-arm64", "linux", "x86_64"));
        assert!(!platform_accepts_job("windows-amd64", "linux", "x86_64"));
        assert!(platform_accepts_job("darwin-arm64", "", ""));
        assert_eq!(
            platform_job_os_arch("darwin-arm64"),
            Some(("darwin", "arm64"))
        );
        assert_eq!(platform_job_os_arch("windows-amd64"), None);
    }
}
