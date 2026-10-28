//! The service directory read against release control — a consumer sent to
//! a port the next rollout moves — and release control against its own
//! targets: an install root under one host's home.

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
    // until the next release: a directory naming a product's candidate port
    // while the policy declares its stable bind has every rollout take
    // product chat down the moment the rollout moves to the other candidate
    // port.
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
                // The directory endpoint is a URL; its port is the one it
                // sends consumers to. A URL this check cannot read, or one
                // without a port, cannot be compared and is reported as such,
                // since the next rollout may still move it.
                let named = match url::Url::parse(&endpoint.url).map(|url| url.port()) {
                    Ok(Some(port)) => port,
                    Ok(None) => {
                        findings.push(Finding::new(
                            "directory-url-without-port",
                            host,
                            format!(
                                "service directory sends {} consumers to {}, which names no \
                                 port, so it cannot be checked against release-controlled \
                                 product {product}'s stable bind {}",
                                policy.service, endpoint.url, serving.stable_bind
                            ),
                        ));
                        continue;
                    }
                    Err(error) => {
                        findings.push(Finding::new(
                            "directory-url-unreadable",
                            host,
                            format!(
                                "service directory sends {} consumers to {}, which is not a \
                                 URL ({error}), so it cannot be checked against \
                                 release-controlled product {product}'s stable bind {}",
                                policy.service, endpoint.url, serving.stable_bind
                            ),
                        ));
                        continue;
                    }
                };
                // The stable bind is `<address>:<port>`; a bind whose port
                // does not read as one declares no comparable port.
                let declared_port = serving.stable_bind.rsplit_once(':').and_then(|(_, port)| {
                    match port.parse::<u16>() {
                        Ok(port) => Some(port),
                        Err(_not_a_port) => None,
                    }
                });
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

/// Every release-controlled product whose `install_root` is an absolute path
/// under one target's home while another target's home differs
/// ([`crate::release_control::foreign_targets`]): release control expands
/// only `{home}`, so that target's agent installs under a home that is not
/// its own. Reported here for documents written before `release policy
/// apply` refused such a root; the document stays readable.
pub(in crate::cli::registry::doctor) fn foreign_install_roots(
    release_control: Option<&crate::release_control::ReleaseControl>,
    findings: &mut Vec<Finding>,
) {
    let Some(control) = release_control else {
        return;
    };
    for (product, policy) in &control.products {
        for foreign in crate::release_control::foreign_targets(policy) {
            findings.push(Finding::new(
                "foreign-install-root",
                &foreign.host,
                format!(
                    "release-controlled product {product} installs every target at {}, under \
                     {}'s home, so {} (home {}) installs where it is not at home; write it as {} \
                     with stado release policy root repair {product}",
                    policy.install_root,
                    foreign.owner,
                    foreign.host,
                    foreign.home,
                    foreign.portable_root
                ),
            ));
        }
    }
}
