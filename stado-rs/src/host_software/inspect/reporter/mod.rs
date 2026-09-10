//! The per-program readings of one reporting pass over one host.
//!
//! [`classify`] decides what each population path is before anything is read
//! from it; the readings of a program — digest, provenance, version — are
//! here.

mod classify;

use std::time::Duration;

use serde_json::Value;

use crate::deploy::products::{self, Shape};
use crate::deploy::{host_channel, shlex_quote, DeployError};
use crate::host_software::{HostSoftware, RELEASE, UNKNOWN, UNMANAGED};

use super::queries::VersionQuery;
use super::{ProgramInspection, ReleaseMatch, Reporter};

pub(super) use classify::Classification;

/// A supported version command is a tiny read, not a recovery operation.
const VERSION_QUERY_DEADLINE: Duration = Duration::from_secs(5);

impl Reporter<'_> {
    /// The program one declared unit runs, read out of the unit file itself
    /// rather than guessed from its label: a label that merely mentions
    /// "stado" is a guess, and a wrong program in this report is worse than
    /// an admitted absence.
    pub(super) async fn unit_program(
        &self,
        kind: &str,
        path: &str,
    ) -> Result<Option<String>, DeployError> {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{}", self.home, path)
        };
        if !host_channel::remote_test(
            self.target,
            &format!("-f {}", shlex_quote(&path)),
            self.runner,
        )
        .await?
        {
            return Ok(None);
        }
        if kind == "systemd" {
            // `sed -n 's/^ExecStart=//p' | head -n 1 | awk '{print $1}'`.
            let read = host_channel::run_command(
                self.target,
                &format!(
                    "sed -n 's/^ExecStart=//p' {} | head -n 1",
                    shlex_quote(&path)
                ),
                self.runner,
            )
            .await?;
            return Ok(read
                .stdout
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().next())
                .map(str::to_string));
        }
        let extracted = host_channel::run_program(
            self.target,
            &[
                "/usr/bin/plutil",
                "-extract",
                "ProgramArguments.0",
                "raw",
                "-o",
                "-",
                &path,
            ],
            self.runner,
        )
        .await?;
        let program = extracted.stdout.trim();
        Ok((extracted.ok() && !program.is_empty()).then(|| program.to_string()))
    }

    /// A version from one command whose argument and response shape are both
    /// declared in the shipped product catalog.
    ///
    /// Failure is deliberately `None`: the caller still emits the program row
    /// as `version=unknown`. The five-second command deadline is carried by
    /// [`CommandSpec`](crate::deploy::CommandSpec), so a server-style binary
    /// cannot consume the channel's 120-second recovery allowance.
    async fn query_version(&self, path: &str, query: &VersionQuery) -> Option<String> {
        let output = tokio::time::timeout(
            VERSION_QUERY_DEADLINE,
            host_channel::run_program_with_timeout(
                self.target,
                &[path, &query.argument],
                VERSION_QUERY_DEADLINE,
                self.runner,
            ),
        )
        .await
        .ok()?
        .ok()?;
        if !output.ok() {
            return None;
        }
        match query.shape {
            Shape::Plain => host_channel::extract_semver(&output.stdout),
            Shape::Json => {
                let document = serde_json::from_str::<Value>(&output.stdout).ok()?;
                let text = match document.get("version")? {
                    Value::String(version) => version.clone(),
                    Value::Number(version) => version.to_string(),
                    _ => return None,
                };
                host_channel::extract_semver(&text)
            }
        }
    }

    /// Recover a version only from the immutable release coordinate of the
    /// candidate whose bytes matched. A similarly named staged file is not
    /// metadata for these bytes, and an arbitrary directory word is not a
    /// version.
    fn release_version(&self, candidate: &str, base: &str) -> Option<String> {
        let relative = candidate.strip_prefix(&self.releases)?.strip_prefix('/')?;
        let mut components = relative.split('/');
        let product = components.next()?;
        let version = components.next()?;
        let platform = components.next()?;
        if product != base
            || relative.rsplit('/').next() != Some(base)
            || !products::PLATFORMS.contains(&platform)
        {
            return None;
        }
        let parsed = host_channel::extract_semver(version)?;
        (parsed == version).then_some(parsed)
    }

    /// `release` when these exact bytes are also a staged release artefact
    /// under `$HOME/.stado/releases`, else `unmanaged`. When that digest match
    /// sits under a canonical release coordinate, it also supplies an honest
    /// version without executing the active file.
    ///
    /// Matched on the basename as well as the digest: the staging trees are
    /// the only place on the host where a verified published artefact is kept
    /// under its own coordinate, and hashing every file beneath them to
    /// answer one question would turn a status read into a full-tree walk of
    /// every release ever delivered.
    async fn provenance(&self, digest: &str, base: &str) -> Result<ReleaseMatch, DeployError> {
        if digest.is_empty() || !self.releases_present {
            return Ok(ReleaseMatch {
                provenance: UNMANAGED,
                version: None,
            });
        }
        let found = host_channel::run_command(
            self.target,
            &format!(
                "find {} -maxdepth 6 -type f -name {} 2>/dev/null",
                shlex_quote(&self.releases),
                shlex_quote(base),
            ),
            self.runner,
        )
        .await?;
        let Some(hasher) = &self.hasher else {
            return Ok(ReleaseMatch {
                provenance: UNMANAGED,
                version: None,
            });
        };
        let mut matched = false;
        let mut matched_version: Option<String> = None;
        let mut ambiguous_version = false;
        for candidate in found
            .stdout
            .lines()
            .filter(|candidate| !candidate.is_empty())
        {
            if hasher.digest(self.target, candidate, self.runner).await? != digest {
                continue;
            }
            matched = true;
            if let Some(version) = self.release_version(candidate, base) {
                match matched_version.as_deref() {
                    Some(previous) if previous != version.as_str() => ambiguous_version = true,
                    None => matched_version = Some(version),
                    _ => {}
                }
            }
        }
        Ok(if matched {
            ReleaseMatch {
                provenance: RELEASE,
                version: if ambiguous_version {
                    None
                } else {
                    matched_version
                },
            }
        } else {
            ReleaseMatch {
                provenance: UNMANAGED,
                version: None,
            }
        })
    }

    /// Inspect one unique population path without ever executing an
    /// undeclared program.
    pub(super) async fn inspect_program(
        &self,
        path: &str,
    ) -> Result<ProgramInspection, DeployError> {
        let decided = self.classify(path).await?;
        self.inspect_classified(path, decided).await
    }

    /// The readings of one path already classified: digest, provenance and
    /// version for a program; a count for a script; nothing for the rest.
    pub(super) async fn inspect_classified(
        &self,
        path: &str,
        decided: Classification,
    ) -> Result<ProgramInspection, DeployError> {
        match decided {
            Classification::Ignored => return Ok(ProgramInspection::Ignored),
            Classification::Script => return Ok(ProgramInspection::Script),
            Classification::Program => {}
        }
        let base = path.rsplit('/').next().unwrap_or(path);
        let digest = match &self.hasher {
            Some(hasher) => hasher.digest(self.target, path, self.runner).await?,
            None => String::new(),
        };
        let ReleaseMatch {
            provenance,
            version: release_version,
        } = self.provenance(&digest, base).await?;
        // A catalog query and release-coordinate metadata are disjoint safety
        // paths. If a declared query fails, `unknown` keeps that failure
        // visible; only a program we did not execute may take its version from
        // the digest-matched release coordinate.
        let version = match self.version_queries.get(path) {
            Some(query) => self.query_version(path, query).await,
            None => release_version,
        }
        .unwrap_or_else(|| UNKNOWN.to_string());
        Ok(ProgramInspection::Software(HostSoftware {
            name: base.to_string(),
            path: path.to_string(),
            version,
            sha256: if digest.is_empty() {
                UNKNOWN.to_string()
            } else {
                digest
            },
            provenance: provenance.to_string(),
        }))
    }
}
