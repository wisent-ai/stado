//! The digest half of a reading: which SHA-256 tool this host has, if any.

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The host's SHA-256 tool, resolved once per report: `shasum` where macOS
/// keeps it, `sha256sum` where Linux keeps it, and nothing — never a
/// fabricated digest — when the host has neither, because the digest decides
/// provenance and a fabricated one would decide it wrongly.
pub(super) enum Hasher {
    Shasum,
    Sha256sum(String),
}

impl Hasher {
    pub(super) async fn resolve(
        target: &ComputeTarget,
        runner: &Runner,
    ) -> Result<Option<Self>, DeployError> {
        if host_channel::remote_test(target, "-x /usr/bin/shasum", runner).await? {
            return Ok(Some(Self::Shasum));
        }
        let found = host_channel::run_command(target, "command -v sha256sum", runner).await?;
        let path = found.stdout.trim();
        Ok((!path.is_empty()).then(|| Self::Sha256sum(path.to_string())))
    }

    /// A file's SHA-256, or nothing when the read failed.
    pub(super) async fn digest(
        &self,
        target: &ComputeTarget,
        path: &str,
        runner: &Runner,
    ) -> Result<String, DeployError> {
        let output = match self {
            Self::Shasum => {
                host_channel::run_program(target, &["/usr/bin/shasum", "-a", "256", path], runner)
                    .await?
            }
            Self::Sha256sum(program) => {
                host_channel::run_program(target, &[program.as_str(), path], runner).await?
            }
        };
        Ok(if output.ok() {
            output
                .stdout
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            String::new()
        })
    }
}
