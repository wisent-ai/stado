//! Did the thing that answered turn out to be the service that was declared?

use crate::observations::{MISOWNED, OBSERVED};
use crate::targets::Registry;

use crate::cli::service_verify::Finding;

/// Did the thing that answered turn out to be the service that was declared?
///
/// A probe proves a socket is alive and nothing more. That was the whole of
/// [`OBSERVED`]'s evidence, and it is why a wrong declaration can read green
/// indefinitely: on 2026-08-31 the directory put `brama` on
/// `http://127.0.0.1:8080` while brama served 18080, an unrelated FastAPI job
/// held 8080, and every sweep recorded `HTTP 404` as an answer. Seventeen
/// hours of a documentation gate failing on a 404 followed, and no check in
/// this binary contradicted the declaration, because none of them asked who
/// owned the port.
///
/// [`crate::deploy::service_serving`] already answers exactly that, by launchd
/// label and never by argv, so this reuses it rather than growing a second
/// opinion about ownership.
///
/// Two deliberate narrowings:
///
/// * Only [`Service::active_host`](crate::targets::Service::active_host). Every other host holding an endpoint
///   reaches this service through its own resolver adapter, whose loopback
///   socket is owned by the resolver on purpose; judging those would file a
///   foreign-owner row against every healthy consumer, and a report that cries
///   wolf gets read like one.
/// * Only rows that already came back [`OBSERVED`]. Where nothing answered,
///   [`UNREACHABLE`](crate::observations::UNREACHABLE) is the finding and who owns the silence adds nothing.
/// * Only ports that are NOT the rollout's declared stable bind. A blue-green
///   product declares `release_control.products.<p>.targets.<host>.stable_bind`
///   and serves it through the rollout's stable proxy, never through the
///   service's own launchd job, so "the declared unit does not hold this port"
///   is the declared design rather than a fault. Judging it by label reported
///   `misowned` against brama on 2026-09-01 for a declaration that was right,
///   and `verify` exited non-zero on a healthy fleet. The port is still named
///   in the detail, so a squatter sharing it stays visible.
///
/// Ownership that cannot be established leaves the row exactly as it was and
/// says so in the detail. "I could not tell" is not evidence of a foreign
/// owner, and this command's rule is that an unchecked declaration is not a
/// failure.
pub(in crate::cli::service_verify) async fn judge_ownership(
    registry: &Registry,
    findings: &mut [Finding],
) {
    let runner = crate::deploy::production_runner();
    for finding in findings.iter_mut() {
        if finding.state != OBSERVED || !finding.probed {
            continue;
        }
        let Some(service) = registry.service(&finding.service) else {
            continue;
        };
        if service.active_host != finding.host {
            continue;
        }
        let Some(port) = url::Url::parse(&finding.endpoint)
            .ok()
            .and_then(|url| url.port())
        else {
            continue;
        };
        // The stable bind is held by the rollout's proxy by declaration, so
        // ownership-by-label is the wrong question for it. Checked before the
        // remote read: there is no answer worth paying a host round-trip for.
        if declared_stable_port(registry, &finding.service, &finding.host) == Some(port) {
            finding.detail = format!(
                "{}; {port} is the stable bind release_control declares for {} on {}, which the \
                 rollout's stable proxy holds rather than the service's own unit",
                finding.detail, finding.service, finding.host
            );
            continue;
        }
        let Some(unit) = registry.service_unit(&finding.service, &finding.host) else {
            finding.detail = format!(
                "{}; the registry names no unit for it on this host, so the port's owner was \
                 not judged",
                finding.detail
            );
            continue;
        };
        let Some(target) = registry
            .local_targets()
            .into_iter()
            .find(|target| target.name == finding.host)
        else {
            continue;
        };
        let Some(declared) = crate::deploy::service::declared_services(target)
            .into_iter()
            .find(|found| found.unit_id() == unit)
        else {
            finding.detail = format!(
                "{}; {unit} is not declared on this host, so the port's owner was not judged",
                finding.detail
            );
            continue;
        };
        let report = match crate::deploy::service_serving::read_serving(
            target,
            unit,
            &declared.path,
            &[port],
            &runner,
        )
        .await
        {
            Ok(report) => report,
            Err(error) => {
                finding.detail = format!(
                    "{}; the port's owner could not be read: {error}",
                    finding.detail
                );
                continue;
            }
        };
        let verdicts = crate::deploy::service_serving::port_verdicts(&report);
        let Some(taken) = verdicts.iter().find(|verdict| {
            verdict.verdict == crate::deploy::service_serving::PORT_SERVED_BY_OTHER
        }) else {
            // Not foreign, but not established either. Both "not judged"
            // branches above say so; leaving this one silent let an operator
            // read an unanswerable question as a verified answer.
            if let Some(unresolved) = verdicts.iter().find(|verdict| {
                matches!(
                    verdict.verdict,
                    crate::deploy::service_serving::PORT_OWNER_UNKNOWN
                        | crate::deploy::service_serving::PORT_UNKNOWN
                )
            }) {
                finding.detail = format!(
                    "{}; something holds {port} but which launchd job owns it could not be \
                     established, so ownership is unjudged: {}",
                    finding.detail,
                    unresolved.holder_cell()
                );
            }
            continue;
        };
        finding.state = MISOWNED;
        finding.detail = format!(
            "{} answered, but {port} is held by {} and not by {unit}",
            finding.detail,
            taken.holder_cell()
        );
    }
}

/// The loopback port a blue-green rollout declares as this service's stable
/// bind on `host`, or `None` when nothing declares one.
///
/// Read from `release_control`, which is where the fleet states it:
/// `products` is keyed by PRODUCT and carries the logical `service` name, so
/// the lookup is by that field rather than by the product key. A `replace`
/// rollout declares no `stable_bind` and yields `None`, which leaves the
/// ownership judgement exactly as it was.
fn declared_stable_port(registry: &Registry, service: &str, host: &str) -> Option<u16> {
    let control = crate::release_control::control(&registry.to_document()).ok()??;
    let policy = control
        .products
        .values()
        .find(|policy| policy.service == service)?;
    let bind = policy.targets.get(host)?.stable_bind.as_deref()?;
    bind.parse::<std::net::SocketAddr>()
        .ok()
        .map(|address| address.port())
}
