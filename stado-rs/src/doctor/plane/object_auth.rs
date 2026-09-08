//! The object, release, machine, and service gateway authorization boundary.

use crate::doctor::Check;

pub(in crate::doctor) const OBJECT_AUTH_ID: &str = "object-auth";
pub(in crate::doctor) const OBJECT_AUTH_TITLE: &str =
    "Object, release, machine, and service gateway auth";
pub(in crate::doctor) const OBJECT_AUTH_REMEDY: &str =
    "configure object_api.namespaces, release_api.publishers, machine_api.clients, and \
     service_api.deployers; install their distinct owner-only verifier grants and scope each \
     verifier to exactly its mapped items; every mapped item must contain a distinct non-empty \
     token field";

pub(in crate::doctor) async fn check_object_auth() -> Check {
    object_auth_verdict(
        crate::skarbiec::validate_object_verifier().await,
        crate::skarbiec::validate_release_verifier().await,
        crate::skarbiec::validate_machine_verifier().await,
        crate::skarbiec::validate_service_verifier().await,
    )
}

/// The gateway-auth row, from the four verifier results alone.
///
/// Taken as data so the judgement can be exercised without a vault, a store
/// or a host — the same reason [`crate::cli::release_submit::claimability`]
/// is shaped this way.
pub fn object_auth_verdict(
    objects: Result<usize, crate::skarbiec::SkarbiecError>,
    releases: Result<usize, crate::skarbiec::SkarbiecError>,
    machines: Result<usize, crate::skarbiec::SkarbiecError>,
    services: Result<usize, crate::skarbiec::SkarbiecError>,
) -> Check {
    match (objects, releases, machines, services) {
        (
            Ok(namespace_count),
            Ok(publisher_count),
            Ok(machine_client_count),
            Ok(deployer_count),
        ) => Check::pass(
            OBJECT_AUTH_ID,
            OBJECT_AUTH_TITLE,
            format!(
                "product verifier exposes exactly {namespace_count} namespace items, release \
                 verifier exposes exactly {publisher_count} publisher items, machine verifier \
                 exposes exactly {machine_client_count} client items, and service verifier \
                 exposes exactly {deployer_count} deployer items; tokens are present and \
                 distinct; namespace, prefix, client, target, service, and action policy is valid"
            ),
            OBJECT_AUTH_REMEDY,
        ),
        (object_result, release_result, machine_result, service_result) => {
            let mut failures = Vec::new();
            let mut unavailable = Vec::new();
            let mut sort =
                |verifier: &str, result: Result<usize, crate::skarbiec::SkarbiecError>| {
                    if let Err(error) = result {
                        // An unreachable or 5xx vault says nothing about mapping,
                        // grants or tokens, and reporting it as `FAIL` said the
                        // opposite: on 2026-09-04 a wedged keyboxd made this row
                        // read "authorization fails closed because mapping,
                        // verifier grant, or mapped token validation failed" with
                        // `error_code=auth`, for a boundary whose mapping and
                        // grants were exactly right, and it flapped back to PASS
                        // on the next sweep. A row that alternates teaches an
                        // operator to discount the whole table.
                        if error.is_unavailable() {
                            unavailable.push(format!("{verifier}: {error}"));
                        } else {
                            failures.push(format!("{verifier}: {error}"));
                        }
                    }
                };
            sort("product verifier", object_result);
            sort("release verifier", release_result);
            sort("machine verifier", machine_result);
            sort("service verifier", service_result);
            if failures.is_empty() {
                return Check::unmeasured(
                    OBJECT_AUTH_ID,
                    OBJECT_AUTH_TITLE,
                    format!(
                        "not measured: the vault did not answer, so this check says nothing \
                         about the deployment either way: {}",
                        unavailable.join("; ")
                    ),
                    OBJECT_AUTH_REMEDY,
                );
            }
            // A real configuration verdict stands on its own, and any
            // unavailable verifier beside it is named so the reader knows
            // which half of the boundary was measured.
            failures.extend(unavailable);
            Check::fail(
                OBJECT_AUTH_ID,
                OBJECT_AUTH_TITLE,
                format!(
                    "authorization fails closed because mapping, verifier grant, or mapped token \
                     validation failed: {}",
                    failures.join("; ")
                ),
                OBJECT_AUTH_REMEDY,
            )
        }
    }
}
