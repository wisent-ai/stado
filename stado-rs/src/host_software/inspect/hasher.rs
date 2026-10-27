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

    /// A file's digest, or `None` when the read failed or printed none. Never
    /// an empty string: two failed reads would compare equal and an
    /// unreadable program would match an unreadable release copy.
    pub(super) async fn digest(
        &self,
        target: &ComputeTarget,
        path: &str,
        runner: &Runner,
    ) -> Result<Option<String>, DeployError> {
        let output = match self {
            Self::Shasum => {
                host_channel::run_program(target, &["/usr/bin/shasum", "-a", "256", path], runner)
                    .await?
            }
            Self::Sha256sum(program) => {
                host_channel::run_program(target, &[program.as_str(), path], runner).await?
            }
        };
        if !output.succeeded() {
            return Ok(None);
        }
        Ok(output.stdout.split_whitespace().next().map(str::to_string))
    }
}
