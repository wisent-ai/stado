//! Do all the pointers to one service agree, and which one is the odd one out?
//!
//! NO Python original. This module exists because of 2026-09-03, when the
//! whole fleet answered `503 object authorization unavailable` for hours and
//! the cause was arithmetic nobody could see: five independent places name the
//! port of the vault on `charless-mac-mini`, four of them said `8895`, one said
//! `18895`, and no command compared them.
//!
//! The five places, all real, all live at once:
//!
//! 1. `release_control.products.<service>.targets.<host>.stable_bind` — where
//!    the product is declared to serve once promoted.
//! 2. `service_directory.services.<service>.endpoints.<host>.url` — where
//!    every consumer is told to dial.
//! 3. `placement_profiles[].hosts.<host>.probes[].url` — where the fleet's own
//!    health probe looks.
//! 4. the launchd unit's own argument vector in `targets[].services[].args` —
//!    where the process actually binds.
//! 5. every vault/service coordinate in the HOST's effective configuration —
//!    what each long-lived process on that host dials, which on 2026-09-03 was
//!    two different answers inside ONE process: the object API's per-namespace
//!    verifier read `object_api.skarbiec.url` (8895) and its authorization
//!    boundary read `secrets.skarbiec.url` (18895).
//!
//! Two properties are deliberate.
//!
//! **The verdict names the outlier, not the disagreement.** "These five
//! disagree" is the fact an operator can already see once they have collected
//! all five, and collecting all five is the hours-long part. The answer that
//! saves the time is "four say 8895 and `unit args` says 18895", so the report
//! is built around the minority, and a tie names every value rather than
//! inventing a winner.
//!
//! **A candidate port in a live pointer is its own finding.** A release
//! qualification binds a candidate to
//! `release_control.products.<service>.targets.<host>.candidate_ports` on
//! purpose, and promotion is what retires it. Any other ending — quarantine,
//! rollback, an abandoned run — leaves that port behind, and the leftover looks
//! exactly like a normal declaration. That is what happened here: the skarbiec
//! rollout ended `observed=quarantined` and left the always-on unit bound to
//! `18895`, a port listed two lines away in the same document as a CANDIDATE
//! port. So a live unit on a candidate bind, or a host-config pointer naming a
//! candidate port, is reported by name — [`Finding::CandidateLeftover`] — and
//! never folded into the majority vote, because a leftover can win a vote.

use std::collections::BTreeMap;

use serde_json::Value;

/// One place that names a port for this service, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    /// Where this answer was read, in the words of the document it came from,
    /// so the report points straight at the field to change.
    pub source: String,
    pub port: u16,
}

/// What is wrong with the pointer set, named rather than merely counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The pointers do not all name one port. `agreed` is the majority answer
    /// when there is one; `outliers` is what disagrees with it.
    Disagreement {
        agreed: Option<u16>,
        outliers: Vec<Pointer>,
    },
    /// A pointer names a port this product's own declaration lists as a
    /// release CANDIDATE port, and the rollout is not promoted onto it.
    CandidateLeftover {
        pointer: Pointer,
        rollout_state: String,
    },
    /// No pointer names a port at all, so nothing was compared. Reported
    /// because "they all agree" and "there was nothing to compare" look
    /// identical in an empty list, and only one of them is a pass.
    NoPointers,
}

impl Finding {
    /// One sentence an operator can act on without reading this module.
    pub fn sentence(&self, service: &str, host: &str) -> String {
        match self {
            Finding::Disagreement { agreed, outliers } => {
                let odd = outliers
                    .iter()
                    .map(|pointer| format!("{} says {}", pointer.source, pointer.port))
                    .collect::<Vec<String>>()
                    .join(", ");
                match agreed {
                    Some(port) => format!(
                        "{service} on {host}: the pointers disagree — most name {port}, and {odd}"
                    ),
                    None => format!(
                        "{service} on {host}: the pointers disagree with no majority — {odd}"
                    ),
                }
            }
            Finding::CandidateLeftover {
                pointer,
                rollout_state,
            } => format!(
                "{service} on {host}: {} names {}, which this product declares as a release \
                 CANDIDATE port, and its rollout is {rollout_state}, not promoted — a \
                 qualification that ended any way other than promotion left it behind",
                pointer.source, pointer.port
            ),
            Finding::NoPointers => format!(
                "{service} on {host}: no pointer names a port, so nothing could be compared"
            ),
        }
    }
}

/// Every pointer to one service on one host, and the findings over them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agreement {
    pub service: String,
    pub host: String,
    pub pointers: Vec<Pointer>,
    pub candidate_ports: Vec<u16>,
    pub rollout_state: String,
    pub findings: Vec<Finding>,
}

impl Agreement {
    /// True when every pointer names one port and none of them is a candidate
    /// leftover. An empty pointer set is NOT agreement.
    pub fn agrees(&self) -> bool {
        self.findings.is_empty()
    }
}

/// The port of a `host:port` bind or an `http(s)://host:port/...` URL.
fn port_of(raw: &str) -> Option<u16> {
    let raw = raw.trim();
    let authority = match raw.split_once("://") {
        Some((_, rest)) => rest.split(['/', '?', '#']).next().unwrap_or(rest),
        None => raw,
    };
    // Only the last colon-separated component is a port, and only when every
    // byte of it is a digit: `[::1]:8895` and a bare `8895` both land here.
    let (_, port) = authority.rsplit_once(':')?;
    port.parse().ok()
}

fn string_at<'a>(root: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = root;
    for key in path {
        cursor = cursor.get(key)?;
    }
    cursor.as_str()
}

/// Collect every pointer to `service` on `host`.
///
/// `registry` is the canonical registry document and `host_config` the host's
/// own effective configuration (`stado host config-show`), or `None` when it
/// could not be read — an unread host configuration is reported as one fewer
/// pointer, never as agreement.
pub fn collect(
    registry: &Value,
    host_config: Option<&Value>,
    service: &str,
    host: &str,
) -> Agreement {
    let mut pointers: Vec<Pointer> = Vec::new();
    let product = ["release_control", "products", service, "targets", host];
    let mut candidate_ports: Vec<u16> = Vec::new();
    let rollout_state = string_at(registry, &[product[0], product[1], service, "state"])
        .or_else(|| {
            string_at(
                registry,
                &["release_control", "products", service, "targets", host],
            )
        })
        .unwrap_or("unknown")
        .to_string();

    let target_product = registry
        .get("release_control")
        .and_then(|value| value.get("products"))
        .and_then(|value| value.get(service))
        .and_then(|value| value.get("targets"))
        .and_then(|value| value.get(host));
    if let Some(product) = target_product {
        if let Some(port) = product.get("stable_bind").and_then(Value::as_str).and_then(port_of) {
            pointers.push(Pointer {
                source: format!("registry release_control.products.{service}.targets.{host}.stable_bind"),
                port,
            });
        }
        if let Some(list) = product.get("candidate_ports").and_then(Value::as_array) {
            for entry in list {
                if let Some(port) = entry
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok())
                    .or_else(|| entry.as_str().and_then(port_of))
                {
                    candidate_ports.push(port);
                }
            }
        }
    }

    if let Some(url) = string_at(
        registry,
        &[
            "service_directory",
            "services",
            service,
            "endpoints",
            host,
            "url",
        ],
    ) {
        if let Some(port) = port_of(url) {
            pointers.push(Pointer {
                source: format!("registry service_directory.services.{service}.endpoints.{host}.url"),
                port,
            });
        }
    }

    // A host runs probes for several services, and a probe carries no service
    // name. Attributing one by port is exactly right here and nowhere else: a
    // probe is this service's only if it already names a port this service's
    // own product declaration named first.
    let known: Vec<u16> = pointers
        .iter()
        .map(|pointer| pointer.port)
        .chain(candidate_ports.iter().copied())
        .collect();
    if let Some(profiles) = registry.get("placement_profiles").and_then(Value::as_array) {
        for profile in profiles {
            let probes = profile
                .get("hosts")
                .and_then(|value| value.get(host))
                .and_then(|value| value.get("probes"))
                .and_then(Value::as_array);
            for probe in probes.into_iter().flatten() {
                let Some(port) = probe.get("url").and_then(Value::as_str).and_then(port_of) else {
                    continue;
                };
                if known.contains(&port) {
                    pointers.push(Pointer {
                        source: format!("registry placement probe for {host}"),
                        port,
                    });
                }
            }
        }
    }

    // The unit's own argument vector: what the process binds, as opposed to
    // what anything says it should bind.
    if let Some(targets) = registry.get("targets").and_then(Value::as_array) {
        for target in targets {
            if target.get("name").and_then(Value::as_str) != Some(host) {
                continue;
            }
            let services = target.get("services").and_then(Value::as_array);
            for declared in services.into_iter().flatten() {
                let names_service = ["name", "label"].iter().any(|key| {
                    declared
                        .get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|value| value == service || value.ends_with(service))
                });
                if !names_service {
                    continue;
                }
                let args = declared.get("args").and_then(Value::as_array);
                let args: Vec<&str> = args
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if let Some(port) = port_from_args(&args) {
                    pointers.push(Pointer {
                        source: format!(
                            "registry unit args of {}",
                            declared
                                .get("label")
                                .and_then(Value::as_str)
                                .unwrap_or(service)
                        ),
                        port,
                    });
                }
            }
        }
    }

    // Every loopback coordinate in the host's own effective configuration that
    // names a port this service already claims. Generic on purpose: the key
    // that was wrong on 2026-09-03 (`secrets.skarbiec.url`) is not special,
    // and a check that only knew the keys someone remembered to list would
    // have missed exactly the one nobody listed.
    if let Some(config) = host_config {
        let known: Vec<u16> = pointers
            .iter()
            .map(|pointer| pointer.port)
            .chain(candidate_ports.iter().copied())
            .collect();
        for (key, port) in config_ports(config) {
            if known.contains(&port) {
                pointers.push(Pointer {
                    source: format!("host config {key}"),
                    port,
                });
            }
        }
    }

    let findings = judge(&pointers, &candidate_ports, &rollout_state);
    Agreement {
        service: service.to_string(),
        host: host.to_string(),
        pointers,
        candidate_ports,
        rollout_state,
        findings,
    }
}

/// `--port N` / `--port=N` / `serve N` anywhere in an argument vector.
fn port_from_args(args: &[&str]) -> Option<u16> {
    let mut iterator = args.iter().peekable();
    while let Some(argument) = iterator.next() {
        if let Some(value) = argument.strip_prefix("--port=") {
            if let Ok(port) = value.parse() {
                return Some(port);
            }
        }
        if *argument == "--port" || *argument == "-p" {
            if let Some(value) = iterator.peek() {
                if let Ok(port) = value.parse() {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Loopback ports named anywhere in a flat or nested configuration document,
/// keyed by the dotted path they were found at.
fn config_ports(config: &Value) -> Vec<(String, u16)> {
    let mut found = Vec::new();
    walk_config(config, &mut String::new(), &mut found);
    found
}

fn walk_config(node: &Value, path: &mut String, found: &mut Vec<(String, u16)>) {
    match node {
        Value::Object(map) => {
            for (key, value) in map {
                let mark = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                walk_config(value, path, found);
                path.truncate(mark);
            }
        }
        Value::Array(items) => {
            for (index, value) in items.iter().enumerate() {
                let mark = path.len();
                path.push_str(&format!("[{index}]"));
                walk_config(value, path, found);
                path.truncate(mark);
            }
        }
        Value::String(raw) => {
            if raw.contains("127.0.0.1") || raw.contains("localhost") || raw.contains("[::1]") {
                if let Some(port) = port_of(raw) {
                    found.push((path.clone(), port));
                }
            }
        }
        _ => {}
    }
}

/// A rollout has retired its candidate ports only when it is promoted onto the
/// stable bind. Every other ending leaves them live, which is the whole point
/// of [`Finding::CandidateLeftover`].
fn promoted(rollout_state: &str) -> bool {
    matches!(rollout_state, "promoted" | "stable" | "rolled_out")
}

fn judge(pointers: &[Pointer], candidate_ports: &[u16], rollout_state: &str) -> Vec<Finding> {
    if pointers.is_empty() {
        return vec![Finding::NoPointers];
    }
    let mut findings = Vec::new();
    if !promoted(rollout_state) {
        for pointer in pointers {
            if candidate_ports.contains(&pointer.port) {
                findings.push(Finding::CandidateLeftover {
                    pointer: pointer.clone(),
                    rollout_state: rollout_state.to_string(),
                });
            }
        }
    }
    // A candidate leftover is excluded from the vote: it is already a named
    // defect, and letting it vote lets the defect become the majority.
    let voting: Vec<&Pointer> = pointers
        .iter()
        .filter(|pointer| promoted(rollout_state) || !candidate_ports.contains(&pointer.port))
        .collect();
    let mut tally: BTreeMap<u16, usize> = BTreeMap::new();
    for pointer in &voting {
        *tally.entry(pointer.port).or_insert(0) += 1;
    }
    if tally.len() > 1 || (!findings.is_empty() && !voting.is_empty()) {
        let best = tally.values().copied().max().unwrap_or(0);
        let leaders: Vec<u16> = tally
            .iter()
            .filter(|(_, count)| **count == best)
            .map(|(port, _)| *port)
            .collect();
        let agreed = (leaders.len() == 1).then(|| leaders[0]);
        let outliers: Vec<Pointer> = pointers
            .iter()
            .filter(|pointer| agreed != Some(pointer.port))
            .cloned()
            .collect();
        if !outliers.is_empty() {
            findings.push(Finding::Disagreement { agreed, outliers });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registry() -> Value {
        json!({
            "release_control": {"products": {"skarbiec": {"targets": {"charless-mac-mini": {
                "stable_bind": "127.0.0.1:8895",
                "candidate_ports": [18895, 18896]
            }}}}},
            "service_directory": {"services": {"skarbiec": {"endpoints": {"charless-mac-mini": {
                "url": "http://127.0.0.1:8895"
            }}}}},
            "placement_profiles": [{"hosts": {"charless-mac-mini": {"probes": [
                {"url": "http://127.0.0.1:8895/health"},
                {"url": "http://127.0.0.1:8080/health"}
            ]}}}],
            "targets": [{"name": "charless-mac-mini", "services": [{
                "label": "com.wisent.always-on.skarbiec",
                "name": "skarbiec",
                "args": ["serve", "--port", "18895"]
            }]}]
        })
    }

    #[test]
    fn port_is_read_from_binds_and_urls() {
        assert_eq!(port_of("127.0.0.1:8895"), Some(8895));
        assert_eq!(port_of("http://127.0.0.1:8895/health"), Some(8895));
        assert_eq!(port_of("https://example.test/health"), None);
        assert_eq!(port_of("8895"), None);
    }

    #[test]
    fn the_live_unit_on_a_candidate_port_is_named_not_outvoted() {
        let report = collect(&registry(), None, "skarbiec", "charless-mac-mini");
        assert!(!report.agrees());
        let leftover = report
            .findings
            .iter()
            .find(|finding| matches!(finding, Finding::CandidateLeftover { .. }))
            .expect("the unit's candidate bind is a named finding");
        let sentence = leftover.sentence("skarbiec", "charless-mac-mini");
        assert!(sentence.contains("18895"), "{sentence}");
        assert!(sentence.contains("CANDIDATE"), "{sentence}");
    }

    #[test]
    fn the_report_names_the_minority_pointer() {
        let report = collect(&registry(), None, "skarbiec", "charless-mac-mini");
        let disagreement = report
            .findings
            .iter()
            .find_map(|finding| match finding {
                Finding::Disagreement { agreed, outliers } => Some((agreed, outliers)),
                _ => None,
            })
            .expect("four pointers against one is a disagreement");
        assert_eq!(*disagreement.0, Some(8895));
        assert_eq!(disagreement.1.len(), 1);
        assert_eq!(disagreement.1[0].port, 18895);
        assert!(disagreement.1[0].source.contains("unit args"));
    }

    #[test]
    fn a_host_config_pointer_on_the_candidate_port_is_found_generically() {
        // The exact 2026-09-03 shape: three scoped pointers right, the base
        // one left on the candidate port, under a key this module never names.
        let config = json!({
            "object_skarbiec_url": "http://127.0.0.1:8895",
            "release_skarbiec_url": "http://127.0.0.1:8895",
            "skarbiec_url": "http://127.0.0.1:18895"
        });
        let report = collect(
            &registry(),
            Some(&config),
            "skarbiec",
            "charless-mac-mini",
        );
        let sources: Vec<&str> = report
            .findings
            .iter()
            .filter_map(|finding| match finding {
                Finding::CandidateLeftover { pointer, .. } => Some(pointer.source.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            sources.iter().any(|source| source.contains("skarbiec_url")),
            "{sources:?}"
        );
    }

    #[test]
    fn agreement_is_silent_and_an_empty_set_is_not_agreement() {
        let mut document = registry();
        document["targets"][0]["services"][0]["args"] =
            json!(["serve", "--port", "8895"]);
        let report = collect(&document, None, "skarbiec", "charless-mac-mini");
        assert!(report.agrees(), "{:?}", report.findings);

        let empty = collect(&json!({}), None, "skarbiec", "charless-mac-mini");
        assert!(!empty.agrees());
        assert_eq!(empty.findings, vec![Finding::NoPointers]);
    }
}
