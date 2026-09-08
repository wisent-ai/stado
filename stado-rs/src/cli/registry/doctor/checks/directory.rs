//! The service directory read against release control: a consumer sent to a
//! port the next rollout moves.

use crate::cli::registry::doctor::findings::Finding;
use crate::targets::Registry;

/// Every service directory endpoint that names a rollout's candidate port
/// instead of the product's stable bind.
pub(in crate::cli::registry::doctor) fn directory_candidate_ports(
    registry: &Registry,
    release_control: Option<&crate::release_control::ReleaseControl>,
    findings: &mut Vec<Finding>,
) {
    // A blue-green product serves consumers on its stable bind; its candidate
    // ports belong to the rollout and change with every version. A service
    // directory that hands consumers a candidate port therefore works only
    // until the next release: on 2026-09-06 the directory named brama's
    // 127.0.0.1:18080 while the policy declared 127.0.0.1:8080, and three
    // rollouts in one evening each took product chat down the moment the
    // rollout moved to the other candidate port.
    if let Some(control) = release_control {
        for (product, policy) in &control.products {
            for (host, target) in &policy.targets {
                let Ok(serving) = target.blue_green_serving() else {
                    continue;
                };
                let Some(route) = registry
                    .service_directory
                    .as_ref()
                    .and_then(|directory| directory.services.get(&policy.service))
                else {
                    continue;
                };
                let Some(endpoint) = route.endpoints.get(host) else {
                    continue;
                };
                let declared_port = serving
                    .stable_bind
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.parse::<u16>().ok());
                let named_port = endpoint
                    .url
                    .rsplit_once(':')
                    .and_then(|(_, port)| port.trim_end_matches('/').parse::<u16>().ok());
                let Some(named) = named_port else {
                    continue;
                };
                if serving.candidate_ports.contains(&named) || declared_port != Some(named) {
                    findings.push(Finding::new(
                        "directory-names-candidate-port",
                        host,
                        format!(
                            "service directory sends {} consumers to {} while release-controlled \
                             product {product} declares its stable bind {}; the next rollout moves \
                             that port and every consumer loses the service",
                            policy.service, endpoint.url, serving.stable_bind
                        ),
                    ));
                }
            }
        }
    }
}
