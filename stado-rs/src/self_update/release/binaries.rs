//! What a release publishes and which release triple this host reads: the
//! checksum manifest name, the published binary set, and the platform
//! segment of the object coordinate.

use super::error::SelfUpdateError;

/// Legacy pointer name retained only by offline compatibility tests. Runtime
/// release resolution never fetches it.
/// The checksum manifest inside each `<version>/<platform>/` directory.
pub const SHA256SUMS_NAME: &str = "SHA256SUMS";

/// Binaries published per release/platform. The self-update replaces the
/// running binary plus the same-dir siblings among these names that exist.
pub const RELEASE_BINARIES: &[&str] = &[
    "stado",
    "wc",
    "stado-coverage",
    "stado-fix",
    "stado-watchdog",
    "stado-mcp",
];

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Ok("linux-amd64")
}

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Ok("darwin-arm64")
}

/// Release triple for the platforms the release pipeline publishes.
/// Other OS/arch combinations are a hard [`SelfUpdateError::UnsupportedPlatform`].
#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
pub fn platform_triple_short() -> Result<&'static str, SelfUpdateError> {
    Err(SelfUpdateError::UnsupportedPlatform {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
    })
}
