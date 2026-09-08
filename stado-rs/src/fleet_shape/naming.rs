//! One rule, three populations: a label carries this fleet's minted prefix
//! exactly once and a systemd unit name carries its suffix exactly once,
//! whether launchd holds the name, the registry declares it, or a placement
//! profile does.

use super::{Finding, PREFIX_CHECK, SUFFIX_CHECK};
use crate::deploy::{local_install, service};
use crate::targets::{ComputeTarget, Registry};

/// The label prefix this fleet's own installer mints.
///
/// A label that carries it twice was built by applying it to a name that
/// already had it. That is not cosmetic: the doubled label is a DIFFERENT
/// label, so it is declared nowhere, every ownership reader calls it
/// undeclared, and launchd runs it anyway.
const MINTED_PREFIX: &str = "com.wisent.compute.service.";

/// A label carries this fleet's minted prefix exactly once.
///
/// `label()` used to prefix a name that already carried the prefix, and the
/// function was fixed early. Nobody went looking for the jobs it had already
/// minted, and on 2026-09-01 eight of them were loaded on charless-mac-mini —
/// one of them a system LaunchDaemon with `KeepAlive` running
/// `stado agent --target charless-mac-mini`, which recreated an undeclared
/// queue agent for days. Three separate sessions hunted it as a rogue script.
///
/// It is a string comparison. It would have ended that hunt on the first
/// sweep, and it is here because the absence of this one line cost more than
/// every check above it put together.
pub(in crate::fleet_shape) fn doubled_prefix(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for unit in loaded {
        *measured += 1;
        let Some(rest) = unit.label.strip_prefix(MINTED_PREFIX) else {
            continue;
        };
        if !rest.starts_with(MINTED_PREFIX) && !rest.starts_with("com.wisent.") {
            continue;
        }
        out.push(Finding {
            check: PREFIX_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: format!("one {MINTED_PREFIX} prefix per label"),
            observed: format!(
                "the label already carried a fleet prefix, so it was minted onto one: the real name is {rest}"
            ),
            command: format!(
                "stado service label-print {} --host {} to see what it holds, then stado service bootout {} --host {} --domain <the domain it is loaded in>",
                unit.label, target.name, unit.label, target.name
            ),
        });
    }
}

/// A systemd unit name carries its `.service` suffix once.
///
/// [`doubled_prefix`] reads what launchd HOLDS; this reads what the registry
/// DECLARES, because the population that matters here is the declaration: a
/// doubled name is written once and then read by everything.
///
/// On 2026-09-03 a resolver deploy recorded
/// `com.wisent.compute.service.stado-resolver.service.service` on
/// ubuntu-server-rtx-pro-6000. The unit is real and active — `systemctl --user
/// is-active` answers yes and `service ensure` restarts it in place — so the
/// cost is not a dead service; it is that the fleet now carries a name nothing
/// else in it agrees with, and the registry's `services` array validated only
/// that a service's `host_heuristic` matched its target's, so the name itself
/// was never checked by anything.
///
/// The remediation is deliberately not `retire`: that command refuses a unit
/// that is still running, correctly, and this unit is running. What closes it
/// is asserting the correct name and then removing the old one.
pub(in crate::fleet_shape) fn doubled_suffix(
    target: &ComputeTarget,
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for service in service::declared_services(target) {
        if service.kind != service::KIND_SYSTEMD {
            continue;
        }
        *measured += 1;
        let Some(stem) = service.unit.strip_suffix(local_install::SYSTEMD_SUFFIX) else {
            continue;
        };
        if !stem.ends_with(local_install::SYSTEMD_SUFFIX) {
            continue;
        }
        out.push(Finding {
            check: SUFFIX_CHECK,
            subject: format!("{}:{}", target.name, service.unit),
            declared: format!(
                "one {} suffix per unit name",
                local_install::SYSTEMD_SUFFIX
            ),
            observed: format!(
                "the name already ended in the suffix, so it was appended twice: the real unit is {stem}"
            ),
            command: format!(
                "stado service ensure {name} --host {host} --reason 'name the unit once' installs the correct name and starts it, then stado service remove {unit} --host {host} drops the doubled one; retire refuses it while it runs",
                name = service.name,
                host = target.name,
                unit = service.unit
            ),
        });
    }
}

/// The prefix rule, on what the registry DECLARES.
///
/// [`doubled_prefix`] reads the labels launchd has LOADED and that the registry
/// does not declare, which is the population the #286 respawner lived in. A
/// label the registry DOES declare is in neither that population nor any
/// other, so `com.wisent.compute.service.com.wisent.stado-host-health-api` and
/// its `-forward` sibling sat declared, loaded and unmeasured on lukasz-macbook
/// while the rule that forbids them was already written down. Same comparison,
/// same check id: it is one rule, and which list a name came from does not
/// change whether it carries the prefix twice.
pub(in crate::fleet_shape) fn declared_doubled_prefix(
    target: &ComputeTarget,
    out: &mut Vec<Finding>,
    measured: &mut usize,
) {
    for service in service::declared_services(target) {
        if service.label.is_empty() {
            continue;
        }
        *measured += 1;
        let Some(rest) = service.label.strip_prefix(MINTED_PREFIX) else {
            continue;
        };
        if !rest.starts_with("com.wisent.") {
            continue;
        }
        out.push(Finding {
            check: PREFIX_CHECK,
            subject: format!("{}:{}", target.name, service.label),
            declared: format!("one {MINTED_PREFIX} prefix per label"),
            observed: format!(
                "the registry declares it with the prefix minted onto a name that already carried one: the real name is {rest}"
            ),
            command: format!(
                "stado service ensure {name} --host {host} --reason 'name the unit once' installs the correct label, then stado service remove {label} --host {host} drops the doubled one",
                name = service.name,
                host = target.name,
                label = service.label
            ),
        });
    }
}

/// The same two rules, on the unit names a placement profile declares.
///
/// The third population that declares a managed unit name, and the one both
/// checks above still missed: managed
/// `placement_profiles[].hosts[<host>].units` entries name a label and path per
/// service. Release-controlled entries deliberately have neither and are not
/// part of either naming population. A doubled managed name remains
/// load-bearing because placement lifecycle commands address it directly.
///
/// `stado service handoff-release-control` is the only command that replaces a
/// managed template with a release-controlled one. Other profile corrections
/// still name the exact registry key because no general profile editor exists.
pub(in crate::fleet_shape) fn profile_unit_names(
    registry: &Registry,
    out: &mut Vec<Finding>,
    labels: &mut usize,
    suffixes: &mut usize,
) {
    for profile in &registry.placement_profiles {
        for (host, placement) in &profile.hosts {
            for (service, unit) in &placement.units {
                if unit.release_controlled() {
                    continue;
                }
                let key = format!(
                    "placement_profiles[{}].hosts.{host}.units.{service}",
                    profile.name
                );
                if unit.kind == service::KIND_SYSTEMD {
                    *suffixes += 1;
                    if let Some(stem) = unit.unit.strip_suffix(local_install::SYSTEMD_SUFFIX) {
                        if stem.ends_with(local_install::SYSTEMD_SUFFIX) {
                            out.push(Finding {
                                check: SUFFIX_CHECK,
                                subject: format!("{host}:{}", unit.unit),
                                declared: format!(
                                    "one {} suffix per unit name",
                                    local_install::SYSTEMD_SUFFIX
                                ),
                                observed: format!(
                                    "the profile's name already ended in the suffix, so it was appended twice: the real unit is {stem}"
                                ),
                                command: format!("correct {key}.unit to {stem}"),
                            });
                        }
                    }
                    continue;
                }
                *labels += 1;
                let Some(rest) = unit.unit.strip_prefix(MINTED_PREFIX) else {
                    continue;
                };
                if !rest.starts_with("com.wisent.") {
                    continue;
                }
                out.push(Finding {
                    check: PREFIX_CHECK,
                    subject: format!("{host}:{}", unit.unit),
                    declared: format!("one {MINTED_PREFIX} prefix per label"),
                    observed: format!(
                        "the profile declares it with the prefix minted onto a name that already carried one: the real name is {rest}"
                    ),
                    command: format!(
                        "correct {key}.unit, .name and .path to {rest}; stado service label-print {} --host {host} says whether anything holds the doubled one",
                        unit.unit
                    ),
                });
            }
        }
    }
}
