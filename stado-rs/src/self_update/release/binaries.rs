//! What a release publishes and which release triple this host reads: the
//! published binary set and the platform segment of the object coordinate.

use super::error::SelfUpdateError;

/// Binaries a signed Stado release carries. The self-update replaces the
/// running binary with the release's own.
pub const RELEASE_BINARIES: &[&str] = &["stado"];

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
