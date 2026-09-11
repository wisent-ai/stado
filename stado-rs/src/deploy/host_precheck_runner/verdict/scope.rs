//! Where a runner registers, and which declared profile may ask for that door.

use crate::deploy::host_precheck_runner::accounts::github::{repository_name, GITHUB_ORGANIZATION};
use crate::deploy::host_precheck_runner::declaration::{RunnerProfile, DECLARATION_PATH};
use crate::deploy::DeployError;

/// Where a runner registers. GitHub answers a registration token at two
/// addresses and they are not interchangeable: the organization endpoint needs
/// the organization's self-hosted-runner permission, and the repository
/// endpoint needs admin on that one repository. On 2026-08-10 and again on
/// 2026-09-06 the fleet's credential was refused at the first and accepted at
/// the second — proven, not assumed — and five separate diagnoses read that
/// 403 as "runners cannot be managed from here". They can; the door is
/// different, and a runner registered to a repository serves that repository
/// only. Which door was used is part of the answer, so it is recorded on the
/// host and reported by `status`.
#[derive(Debug, Clone)]
pub enum RunnerScope {
    Organization,
    Repository(String),
}

impl RunnerScope {
    /// The URL `config.sh` registers against.
    pub(crate) fn registration_url(&self) -> String {
        match self {
            Self::Organization => format!("https://github.com/{GITHUB_ORGANIZATION}"),
            Self::Repository(repository) => {
                format!("https://github.com/{GITHUB_ORGANIZATION}/{repository}")
            }
        }
    }

    pub(crate) fn group<'a>(&self, profile: &'a RunnerProfile) -> &'a str {
        match self {
            Self::Organization => &profile.github_runner_group,
            Self::Repository(_) => "",
        }
    }

    pub(crate) fn token_endpoint(&self, kind: &str) -> String {
        match self {
            Self::Organization => format!(
                "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runners/{kind}-token"
            ),
            Self::Repository(repository) => format!(
                "https://api.github.com/repos/{GITHUB_ORGANIZATION}/{repository}/actions/runners/{kind}-token"
            ),
        }
    }

    /// Where GitHub lists the runners this scope registered. The two lists are
    /// disjoint: an organization runner never appears under a repository and a
    /// repository runner never appears under the organization, so asking the
    /// wrong one answers "no such runner" about a runner that exists.
    pub(crate) fn runners_endpoint(&self) -> String {
        match self {
            Self::Organization => format!(
                "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runners?per_page=100"
            ),
            Self::Repository(repository) => format!(
                "https://api.github.com/repos/{GITHUB_ORGANIZATION}/{repository}/actions/runners?per_page=100"
            ),
        }
    }

    /// What `status` prints and what the host records.
    pub fn label(&self) -> String {
        match self {
            Self::Organization => format!("organization:{GITHUB_ORGANIZATION}"),
            Self::Repository(repository) => {
                format!("repository:{GITHUB_ORGANIZATION}/{repository}")
            }
        }
    }
}

pub(crate) fn scope_for_profile(
    profile: &RunnerProfile,
    repository: Option<&str>,
) -> Result<RunnerScope, DeployError> {
    let Some(repository) = repository else {
        return Ok(RunnerScope::Organization);
    };
    if !profile.accepts_repository_scope {
        return Err(DeployError(format!(
            "runner profile '{}' declares no repository scope; enable accepts_repository_scope in {DECLARATION_PATH}",
            profile.name
        )));
    }
    Ok(RunnerScope::Repository(
        repository_name(repository)?.to_string(),
    ))
}
