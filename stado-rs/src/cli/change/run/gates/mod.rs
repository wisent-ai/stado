//! The candidate's declared gates, as the release manifest of its rebased
//! commit declares them: the locks, the module declarations and every
//! `format` gate checked here, compiling nothing (`stado quality check`'s
//! formatting selection), then every `lint` and `test` gate of every
//! declared platform as fleet host shards ([`shard`]: a Cargo gate as `stado
//! host cargo` runs it, any other as declared) on the hosts of that platform
//! ([`pool`]), each host running one shard at a time and the hosts at
//! once. A shard the action cache ([`cache`]) holds a pass for
//! is answered without running. A host that cannot run its shard (it is
//! unreachable, its disk is full) leaves the shard to the platform's other
//! hosts; a platform left with no host ends the pass unchecked, never with
//! a verdict on the candidate. A direct push to `main` runs the same
//! compiling shards, its `lint` gates only ([`PUSHED`]), through
//! [`compiling`] (`crate::cli::change::push`).

mod cache;
mod pool;
mod remote;
mod shard;
mod targets;

pub use shard::Planned;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use crate::cli::change::git::git;
use crate::cli::change::record::Shard;
use crate::cli::quality::{check_revision_for, Report, Selection};
use crate::cli::CmdError;
use crate::release_pipeline::{
    parse_product_manifest, GateKind, ProductManifest, PRODUCT_MANIFEST,
};

/// How much of a shard's report its record keeps: the end, where Cargo and
/// test harnesses put the verdict.
const REPORT_LINES: usize = 60;

/// What the manifest of a commit declares the queue runs.
pub struct Plan {
    pub product: String,
    /// The version the commit's release would cut, as its manifest's
    /// version source states it: the `WISENT_VERSION` a declared gate reads.
    pub version: String,
    pub compiling: Vec<Planned>,
}

impl Plan {
    /// Each compiling gate once, as `name (kind) for platform`.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .compiling
            .iter()
            .map(|planned| {
                format!(
                    "{} ({}) for {}",
                    planned.gate, planned.kind, planned.platform
                )
            })
            .collect();
        names.dedup();
        names
    }
}

/// The compiling gate kinds a queued candidate runs on the fleet: every
/// `lint` and `test` gate, since what it lands is what the release builds.
pub const QUEUED: &[GateKind] = &[GateKind::Lint, GateKind::Test];

/// The compiling gate kinds a push to `main` the queue did not make runs on
/// the fleet (`stado quality check --pre-push`): the `lint` gates, which
/// compile the pushed tree without running it. Its `test` gates stay the
/// queue's and the release's, as a push waits for its gates.
pub const PUSHED: &[GateKind] = &[GateKind::Lint];

/// The plan of `commit`, its compiling gates of the `kinds` named; a
/// declaration the queue cannot run is the candidate's refusal (`Err` of
/// the inner result).
pub async fn plan(
    checkout: &Path,
    commit: &str,
    kinds: &[GateKind],
) -> Result<Result<Plan, String>, CmdError> {
    // Only an absent manifest is the candidate's refusal; a Git that cannot
    // read the commit leaves the candidate queued.
    let listed = git(
        checkout,
        &["ls-tree", "--name-only", commit, "--", PRODUCT_MANIFEST],
    )
    .await?;
    if listed.is_empty() {
        return Ok(Err(format!(
            "{commit} holds no {PRODUCT_MANIFEST}, so it declares no product and no gate the \
             queue could run before landing it"
        )));
    }
    let shown = format!("{commit}:{PRODUCT_MANIFEST}");
    let text = git(checkout, &["show", &shown]).await?;
    let manifest = match parse_product_manifest(text.as_bytes()) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(Err(format!(
                "{PRODUCT_MANIFEST} of {commit} is refused: {error}"
            )))
        }
    };
    let (product, platforms, source) = match manifest {
        ProductManifest::Release(manifest) => (
            manifest.product.clone(),
            manifest.platforms,
            manifest.version_source,
        ),
        ProductManifest::NonRelease(manifest) => {
            return Ok(Ok(Plan {
                product: manifest.product,
                version: String::new(),
                compiling: Vec::new(),
            }))
        }
    };
    let path = crate::release_pipeline::version_path(&source);
    let stated = git(checkout, &["show", &format!("{commit}:{path}")]).await;
    let version = match crate::release_pipeline::declared_version(&source, |_| {
        stated
            .map(String::into_bytes)
            .map_err(|error| error.to_string())
    }) {
        Ok(version) => version,
        Err(why) => {
            return Ok(Err(format!(
                "the version {commit} would release cannot be read from its version source: \
                 {why}"
            )))
        }
    };
    let mut compiling = Vec::new();
    for (platform, recipe) in &platforms {
        for gate in recipe
            .quality
            .iter()
            .filter(|gate| kinds.contains(&gate.kind))
        {
            if !gate.inputs.is_empty() {
                return Ok(Err(format!(
                    "gate {} ({}) for {platform} reads the release inputs {}, which the submit \
                     queue does not stage on a fleet host; run `stado quality check --revision \
                     {commit}` where they are staged",
                    gate.name,
                    gate.kind,
                    gate.inputs.join(", ")
                )));
            }
            let planned = shard::placed(gate, platform);
            if let Err(why) = shard::runnable(checkout, commit, &planned).await? {
                return Ok(Err(why));
            }
            match targets::sharded(checkout, commit, planned).await? {
                Ok(shards) => compiling.extend(shards),
                Err(why) => return Ok(Err(why)),
            }
        }
    }
    Ok(Ok(Plan {
        product,
        version,
        compiling,
    }))
}

/// How the gates ended: every shard that ran or was answered, and the
/// refusal when one refused.
pub struct Outcome {
    pub shards: Vec<Shard>,
    pub refusal: Option<String>,
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(REPORT_LINES)..].join("\n")
}

/// Run `plan` over `commit` (tree `tree`) of `checkout` on the pool of
/// `hosts`, as work of the agent session `session` that submitted it.
pub async fn run(
    checkout: &Path,
    commit: &str,
    tree: &str,
    plan: &Plan,
    hosts: &[String],
    (purposes, session): (&[&str], Option<&str>),
) -> Result<Outcome, CmdError> {
    let mut shards = Vec::new();
    let gate = "locks, modules and format gates";
    let argv: Vec<String> = ["stado", "quality", "check", "--pre-push"]
        .map(String::from)
        .into();
    let key = cache::key(tree, "any", gate, &argv);
    let local = match cache::hit(&key)? {
        Some(hit) => hit,
        None => {
            let started = Instant::now();
            let checked = check_revision_for(
                checkout,
                commit,
                Report::Stderr,
                Selection::Formatting,
                purposes,
            )
            .await;
            // Whether the check refused is its result, never whether its
            // refusal has words; the refusal is the shard's report.
            let (exit_code, report) = match checked {
                Ok(()) => (0, String::new()),
                Err(error)
                    if error.failure
                        == Some(crate::primitives::failure::FailureCode::InfraDown) =>
                {
                    return Err(error.within("the format check did not run"));
                }
                Err(error) => (1, error.to_string()),
            };
            let shard = Shard {
                gate: gate.to_string(),
                kind: GateKind::Format.to_string(),
                platform: "any".to_string(),
                argv,
                host: "this host".to_string(),
                cached: false,
                exit_code,
                seconds: started.elapsed().as_secs(),
                report,
            };
            cache::keep(&key, &shard)?;
            shard
        }
    };
    let format_refused = local.exit_code != 0;
    let report = local.report.clone();
    shards.push(local);
    if format_refused {
        return Ok(Outcome {
            shards,
            refusal: Some(report),
        });
    }
    let (remote, refusal) =
        compiling(checkout, commit, tree, plan, hosts, (purposes, session)).await?;
    shards.extend(remote);
    Ok(Outcome { shards, refusal })
}

/// Every compiling shard of `plan` over `commit` (tree `tree`) of
/// `checkout`, run or answered by the action cache on the pool of `hosts`
/// once Oko admitted each action they take as work of `session`, and the
/// refusal naming every shard that refused with the end of its report.
pub async fn compiling(
    checkout: &Path,
    commit: &str,
    tree: &str,
    plan: &Plan,
    hosts: &[String],
    (purposes, session): (&[&str], Option<&str>),
) -> Result<(Vec<Shard>, Option<String>), CmdError> {
    // Each action the compiling gates take is asked of Oko once, as the work
    // of the session that asked for the gates (a candidate's submitter, the
    // session that pushed), never as the queue's own.
    let mut asked = BTreeSet::new();
    for planned in &plan.compiling {
        let action = if planned.kind == GateKind::Test {
            crate::cli::release_loop::allowed::TEST
        } else {
            crate::cli::release_loop::allowed::COMPILE
        };
        if !asked.insert(action) {
            continue;
        }
        crate::cli::release_loop::allowed::refusal_as(
            action,
            Some(&plan.product),
            purposes,
            session,
        )
        .await
        .map_err(|refused| {
            CmdError::refused(format!(
                "{refused}; gate {} ({}) for {} was not run",
                planned.gate, planned.kind, planned.platform
            ))
        })?;
    }
    let remote = remote::run(checkout, commit, tree, plan, hosts).await?;
    let refusal = remote
        .iter()
        .filter(|shard| shard.exit_code != 0)
        .map(|shard| {
            format!(
                "gate {} ({}) for {} on {} refused, `{}` exited {}:\n{}",
                shard.gate,
                shard.kind,
                shard.platform,
                shard.host,
                shard.argv.join(" "),
                shard.exit_code,
                shard.report
            )
        })
        .collect::<Vec<_>>();
    Ok((remote, (!refusal.is_empty()).then(|| refusal.join("\n\n"))))
}
