//! What this host can actually reach, probed from this host.

use crate::observations::UNVERIFIED;
use crate::targets::Registry;

use crate::cli::service_verify::checks::{endpoint_for, probe_hosts, unsupported};
use crate::cli::service_verify::probe::probe;
use crate::cli::service_verify::Finding;

/// Probe every declaration that names THIS host, from this host, by whatever
/// method each declaration carries.
pub(in crate::cli::service_verify) async fn local_findings(
    registry: &Registry,
    me: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(directory) = registry.service_directory.as_ref() else {
        return findings;
    };
    for (name, service) in &directory.services {
        let descriptor = service.verification();
        if !probe_hosts(service, &descriptor).contains(me) {
            continue;
        }
        let endpoint = endpoint_for(service, me, &descriptor.kind);
        if let Some(detail) = unsupported(name, &descriptor) {
            findings.push(Finding {
                service: name.clone(),
                host: me.to_string(),
                endpoint: endpoint.unwrap_or_else(|| "-".to_string()),
                state: UNVERIFIED,
                detail,
                probed: true,
            });
            continue;
        }
        match endpoint {
            None => findings.push(Finding {
                service: name.clone(),
                host: me.to_string(),
                endpoint: "-".to_string(),
                state: UNVERIFIED,
                detail: "no endpoint declared for this host".to_string(),
                probed: true,
            }),
            Some(url) => {
                let (state, detail) = probe(&descriptor.kind, &url).await;
                findings.push(Finding {
                    service: name.clone(),
                    host: me.to_string(),
                    endpoint: url,
                    state,
                    detail,
                    probed: true,
                });
            }
        }
    }
    findings
}
