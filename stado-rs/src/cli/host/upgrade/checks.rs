//! What must still answer after the host's Stado is replaced: the fleet's
//! registry, the host's own reading of it, the vault the host keeps, and
//! every service the registry declares on the host at its declared port.
//! Each is read before the install and again after it; one that answered
//! before and no longer does is the reason the kept previous program goes
//! back.

use serde::Serialize;
use serde_json::Value;

use crate::cli::service::lifecycle::release::awaited::{until_serving, Served};
use crate::deploy::service::ManagedService;
use crate::deploy::{service_serving, Runner};
use crate::targets::ComputeTarget;

/// One check and what it found.
#[derive(Clone, Debug, Serialize)]
pub(super) struct Check {
    pub name: String,
    pub answered: bool,
    pub detail: String,
}

/// What a check reads.
enum Kind {
    /// The fleet registry answers from its authority and declares the host.
    Registry,
    /// The host's own Stado reads the registry (`stado registry doctor` on
    /// it, an allowlisted read).
    HostRegistry,
    /// The vault the host keeps answers its own diagnosis (`skarbiec
    /// doctor` on it).
    Vault,
    /// A declared service serves its declared port.
    Service(Box<ManagedService>),
}

fn name(kind: &Kind, host: &str) -> String {
    match kind {
        Kind::Registry => "registry".to_string(),
        Kind::HostRegistry => format!("registry read on {host}"),
        Kind::Vault => format!("vault on {host}"),
        Kind::Service(service) => format!("service {}", service.name),
    }
}

/// The checks the host declares, in the order they are read.
fn kinds(target: &ComputeTarget) -> Vec<Kind> {
    let mut kinds = vec![Kind::Registry, Kind::HostRegistry, Kind::Vault];
    kinds.extend(
        crate::deploy::service::declared_services(target)
            .into_iter()
            .map(|service| Kind::Service(Box::new(service))),
    );
    kinds
}

/// The host-exec words of an approved fixed command: the program's
/// basename, then its arguments, as `stado host exec` takes them.
pub(super) fn exec_words(argv: &[&str]) -> Vec<String> {
    argv.iter()
        .enumerate()
        .map(|(index, word)| match (index, word.rsplit_once('/')) {
            (0, Some((_, basename))) => basename.to_string(),
            _ => word.to_string(),
        })
        .collect()
}

/// Run one approved read on the host: whether it exited 0, and the last
/// line it said itself (an announced wait's line is not one) or its refusal.
pub(super) async fn exec(host: &str, argv: &[&str], runner: &Runner) -> (bool, String, Value) {
    match crate::deploy::host_exec::exec_host(host, &exec_words(argv), runner).await {
        Ok(receipt) => {
            let ok = receipt["status"] == crate::deploy::host_exec::OK_STATUS;
            // A stream the receipt does not carry printed nothing.
            let said = |field: &str| match receipt[field].as_str() {
                Some(text) => text.trim().to_string(),
                None => String::new(),
            };
            let (stdout, stderr) = (said("stdout"), said("stderr"));
            // A JSON report says itself in its top-level counts and words; a
            // text report in its last line that is not a wait's.
            let summary = match serde_json::from_str::<Value>(&stdout) {
                Ok(Value::Object(fields)) => Some(
                    fields
                        .iter()
                        .filter(|(_, value)| !value.is_object() && !value.is_array())
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                _ => stdout
                    .lines()
                    .chain(stderr.lines())
                    .rfind(|line| !line.trim().is_empty() && !crate::wait::line::announced(line))
                    .map(str::to_string),
            };
            let detail = match (ok, summary, receipt["error"].as_str()) {
                (false, _, Some(error)) => error.to_string(),
                (_, Some(summary), _) => summary,
                (_, None, _) => format!("exited {}", receipt["exit_code"]),
            };
            (ok, detail, receipt)
        }
        Err(refusal) => (false, refusal.message, Value::Null),
    }
}

async fn serving_once(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> (bool, String) {
    let unit = service.unit_id();
    let port = match crate::cli::service::directory_port(unit, &service.host).await {
        Ok(Some(port)) => port,
        Ok(None) => {
            return (
                true,
                format!("{unit}: the service directory declares no port on this host; not read"),
            )
        }
        Err(error) => return (false, error.to_string()),
    };
    match service_serving::read_serving(target, unit, &service.path, &[port], runner).await {
        Ok(report) => {
            let verdicts = service_serving::port_verdicts(&report);
            let verdict = service_serving::verdict(&report, &verdicts);
            let serving = verdict == service_serving::SERVING_YES;
            let detail = match (
                serving,
                service_serving::failure(&service.host, &report, &verdicts),
            ) {
                (true, _) => format!("{unit} serves port {port}"),
                (false, Some(why)) => why,
                (false, None) => format!("{unit}: port {port} is {verdict}"),
            };
            (serving, detail)
        }
        Err(error) => (false, error.to_string()),
    }
}

/// The words of `stado registry doctor`'s verdict when it read the registry
/// and the host and found them apart: `N divergence(s) between <REGISTRY>
/// and live host state`.
const DOCTOR_VERDICT: &str = "divergence(s) between";

async fn read(kind: &Kind, target: &ComputeTarget, after: bool, runner: &Runner) -> (bool, String) {
    match kind {
        Kind::Registry => match crate::targets::fetch_registry_remote().await {
            Ok(registry) if registry.lookup(&target.name).is_some() => (
                true,
                format!(
                    "the registry authority answers and declares {}",
                    target.name
                ),
            ),
            Ok(_) => (
                false,
                format!(
                    "the registry authority answers but declares no {}",
                    target.name
                ),
            ),
            Err(error) => (false, error.to_string()),
        },
        Kind::HostRegistry => {
            let (ok, detail, _) = exec(
                &target.name,
                &[crate::deploy::host_exec::STADO_CLI, "registry", "doctor"],
                runner,
            )
            .await;
            // The doctor exits 1 for a verdict that names divergences: it
            // read the registry and the host, which is what this asks.
            (ok || detail.contains(DOCTOR_VERDICT), detail)
        }
        Kind::Vault => {
            let (ok, detail, _) = exec(
                &target.name,
                &[crate::deploy::host_exec::SKARBIEC_CLI, "doctor"],
                runner,
            )
            .await;
            (ok, detail)
        }
        // After the install a restarted unit is read until it serves or
        // shows why it cannot, as an install's own activation reads it.
        Kind::Service(service) if after => match until_serving(target, service, runner).await {
            Ok(Served::Serving) => (
                true,
                format!("{} serves its declared port", service.unit_id()),
            ),
            Ok(Served::NoDeclaredPort) => (
                true,
                format!(
                    "{}: the service directory declares no port on this host; not read",
                    service.unit_id()
                ),
            ),
            Ok(Served::Failed(why)) => (false, why),
            Err(error) => (false, error.to_string()),
        },
        Kind::Service(service) => serving_once(target, service, runner).await,
    }
}

/// Read every check once.
pub(super) async fn run(target: &ComputeTarget, after: bool, runner: &Runner) -> Vec<Check> {
    let mut checks = Vec::new();
    for kind in kinds(target) {
        let (answered, detail) = read(&kind, target, after, runner).await;
        checks.push(Check {
            name: name(&kind, &target.name),
            answered,
            detail,
        });
    }
    checks
}

/// The checks that answered before and do not answer now.
pub(super) fn regressed<'a>(before: &[Check], after: &'a [Check]) -> Vec<&'a Check> {
    after
        .iter()
        .filter(|now| {
            !now.answered
                && before
                    .iter()
                    .any(|then| then.name == now.name && then.answered)
        })
        .collect()
}
