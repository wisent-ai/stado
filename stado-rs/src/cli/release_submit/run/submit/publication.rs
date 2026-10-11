//! The second half of one release pass: a run whose build passed and that no
//! newer run replaced publishes each platform, delivers, and promotes.

use std::collections::BTreeMap;

use crate::cli::release_cmd;
use crate::cli::release_submit::deliver::deliveries::{run_deliveries, Deliveries};
use crate::cli::release_submit::publish::artifact::publish;
use crate::cli::release_submit::publish::promotion::{reconcile, Promotion};
use crate::cli::release_submit::publish::signing::signing;
use crate::cli::release_submit::run::state::{persist_failure, save};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    Delivery, PlatformRunState, ReleasePipelineManifest, ReleaseRun, ReleaseRunState,
};

/// How a run's `failure` says that delivery is waiting, not failed, so the pass
/// that converges removes exactly that note and keeps any other.
const DEFERRED: &str = "delivery waits: ";

/// Publish `submitted` platforms (reading back the ones already published),
/// deliver them and promote the release. `enqueue_failure` is the refusal of
/// a platform the pass could not queue: fatal when a required platform is
/// left without an artifact, recorded otherwise.
#[allow(clippy::too_many_arguments)]
pub(in crate::cli::release_submit::run) async fn publish_and_deliver(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    store: &JobStorage,
    deliveries: &[Delivery],
    submitted: &[String],
    already_published: bool,
    enqueue_failure: Option<CmdError>,
    json: bool,
) -> Result<(), CmdError> {
    let mut signing_material = None;
    if !already_published {
        run.state = ReleaseRunState::Publishing;
        save(run).await?;
    }
    let mut artifacts = BTreeMap::new();
    for p in submitted {
        let result = if run.platforms[p].state == PlatformRunState::Published {
            let verified = release_cmd::verified_artifact_for_submit(&run.product, &run.version, p);
            verified.await
        } else {
            // Verification and delivery of already signed bytes need no private
            // signing grant on the machine resuming the release.
            if signing_material.is_none() {
                signing_material = Some(match signing(&run.product).await {
                    Ok(material) => material,
                    Err(error) => return Err(persist_failure(run, error).await),
                });
            }
            let (key, private) = signing_material
                .as_ref()
                .expect("unpublished platform needs signing");
            publish(run, m, p, store, key, private).await
        };
        let a = match result {
            Ok(artifact) => artifact,
            Err(error) => {
                let platform = run.platforms.get_mut(p).unwrap();
                platform.state = PlatformRunState::Failed;
                platform.failure = Some(error.to_string());
                if m.platforms[p].required {
                    return Err(persist_failure(run, error).await);
                }
                save(run).await?;
                continue;
            }
        };
        save(run).await?;
        artifacts.insert(p.clone(), a);
    }
    // A refusal that left only optional platforms unbuilt is recorded, not
    // fatal: the manifest declared them skippable. Failing here threw away
    // skarbiec 0.4.5's published darwin build on every resume because its
    // optional linux-amd64 had no builder allowed its signing secret.
    if let Some(error) = enqueue_failure {
        let required_unqueued = m
            .platforms
            .iter()
            .any(|(platform, recipe)| recipe.required && !artifacts.contains_key(platform));
        if required_unqueued {
            return Err(persist_failure(run, error).await);
        }
        run.failure = Some(format!("optional platform not built: {error}"));
    }
    run.state = ReleaseRunState::Delivering;
    save(run).await?;
    match run_deliveries(run, m, &artifacts, deliveries).await {
        Ok(Deliveries::Complete) => {}
        // A delivery still queued or running on its host is judged by the
        // pass that finds it ended; this one leaves the run delivering, and
        // the release agent's next tick, or `stado release resume`, reads it
        // again.
        Ok(Deliveries::Pending) => {
            save(run).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&*run)?)
            } else {
                println!(
                    "release run {} product={} version={} state={:?}: deliveries queued on their hosts; the next release agent tick or `stado release resume {}` collects their verdicts",
                    run.run_id, run.product, run.version, run.state, run.run_id
                )
            }
            return Ok(());
        }
        Err(error) => return Err(persist_failure(run, error).await),
    }
    if m.promotion.reconcile {
        let promotion = release_cmd::promote_for_submit(&run.product, &run.version, run.channel);
        if let Err(error) = promotion.await {
            return Err(persist_failure(run, error).await);
        }
        match reconcile(run).await {
            Ok(Promotion::Converged) => {}
            // The unit was not touched; the run stays delivering, says why,
            // and the release agent's next tick or `stado release resume`
            // reads the service again.
            Ok(Promotion::Deferred(why)) => {
                run.failure = Some(format!("{DEFERRED}{why}"));
                save(run).await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&*run)?)
                } else {
                    println!(
                        "release run {} product={} version={} state={:?}: {why}; the next release agent tick or `stado release resume {}` reads it again",
                        run.run_id, run.product, run.version, run.state, run.run_id
                    )
                }
                return Ok(());
            }
            Err(error) => return Err(persist_failure(run, error).await),
        }
        if run
            .failure
            .as_deref()
            .is_some_and(|said| said.starts_with(DEFERRED))
        {
            run.failure = None;
        }
        run.state = ReleaseRunState::Reconciled
    } else {
        run.state = ReleaseRunState::Completed
    }
    save(run).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&*run)?)
    } else {
        println!(
            "release run {} product={} version={} state={:?}",
            run.run_id, run.product, run.version, run.state
        )
    }
    Ok(())
}
