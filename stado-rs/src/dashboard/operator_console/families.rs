//! Which command families the Desktop API may reach, and which of their
//! operations only read.
//!
//! Split out of `mod.rs` so the table is a file an operator can read whole.
//! The distinction it draws is load-bearing: a command that only reads runs
//! on the Desktop's own request, and anything else needs the explicit
//! `RUN_MUTATION` confirmation the review dialog supplies.

pub(super) const ALLOWED_FAMILIES: &[&str] = &[
    "alerts",
    "artifact",
    "billing",
    "blast-radius",
    "bootstrap",
    "build",
    "cancel",
    "capabilities",
    "cloud",
    "config",
    "cost",
    "credentials",
    "database",
    "disk-cleanup",
    "doctor",
    "fleet",
    "host",
    "identity",
    "inference",
    "instances",
    "job",
    "machine",
    "mail",
    "market",
    "optimize",
    "overview",
    "placement",
    "product",
    "profiles",
    "quota",
    "queue",
    "recovery",
    "registry",
    "release",
    "resources",
    "repair",
    "results",
    "runner",
    "route",
    "schedule",
    "scratch",
    "service",
    "space",
    "status",
    "storage",
    "submit",
    "tunnel",
    "web",
    "workdirs",
    "workload",
];

pub(super) fn is_retained_log_request(args: &[String]) -> bool {
    args.first().is_some_and(|arg| arg == "host")
        && args.get(1).is_some_and(|arg| arg == "exec")
        && args
            .iter()
            .position(|arg| arg == "--")
            .is_some_and(|separator| {
                crate::deploy::host_exec::is_retained_log_read(&args[separator + 1..])
            })
}

pub(super) fn is_read_only(args: &[String]) -> bool {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return true;
    }
    let family = args.first().map(String::as_str).unwrap_or("");
    let operation = args.get(1).map(String::as_str).unwrap_or("");
    let detail = args.get(2).map(String::as_str).unwrap_or("");
    if family == "bootstrap" {
        // Printing the immutable installer does not provision a host.
        return args.len() == 2 && operation == "--print-install-script";
    }
    if family == "workload" {
        return matches!(operation, "list" | "status");
    }
    if family == "repair" {
        return !args.iter().any(|arg| arg == "--apply");
    }
    if family == "route" {
        return matches!(operation, "list" | "capability");
    }
    if family == "database" {
        return operation == "list";
    }
    if family == "tunnel" {
        return matches!(operation, "list" | "status");
    }
    if family == "registry" && operation == "host" && detail == "path" {
        return args.get(3).is_some_and(|value| value == "list");
    }
    if family == "alerts" && operation == "preferences" {
        // `stado alerts preferences` with no action shows the choice.
        return matches!(detail, "" | "show");
    }
    if family == "scratch" {
        return matches!(operation, "profiles" | "hosts" | "list")
            || (operation == "reap" && !args.iter().any(|arg| arg == "--apply"));
    }
    if family == "credentials" {
        return operation == "get"
            || (operation == "seed" && detail == "list")
            || (operation == "item"
                && (detail == "show"
                    || (detail == "retag" && !args.iter().any(|arg| arg == "--tags"))))
            || (operation == "grant" && detail == "show")
            || (operation == "vault"
                && (matches!(detail, "show" | "list" | "items")
                    || (detail == "sync" && args.iter().any(|arg| arg == "--check"))))
            || (operation == "backup"
                && detail == "audit"
                && !args.iter().any(|arg| arg == "--apply"));
    }
    if family == "release" {
        return matches!(
            operation,
            "status" | "provenance" | "logs" | "doctor" | "active-binary"
        ) || (operation == "version" && detail == "show")
            || (operation == "catalog" && detail == "audit")
            || (operation == "quarantine" && detail == "list")
            || (operation == "destinations" && matches!(detail, "list" | "show"))
            || (operation == "policy" && matches!(detail, "list" | "show"));
    }
    if family == "build" {
        return matches!(operation, "status" | "list")
            || (operation == "newest" && args.iter().any(|arg| arg == "--plan"));
    }
    if family == "workdirs" {
        return !args.iter().any(|arg| arg == "--apply");
    }
    if family == "product" {
        return product_read_only(args);
    }
    if family == "space" {
        return operation == "report"
            || (operation == "cleaners" && detail == "list")
            || (matches!(operation, "reclaim" | "relocate")
                && !args.iter().any(|arg| arg == "--apply"))
            || (operation == "file"
                && detail == "retire"
                && args.iter().any(|arg| arg == "--dry-run"));
    }
    if family == "host" && operation == "exec" {
        return is_retained_log_request(args);
    }
    if family == "host" && operation == "beacon" {
        return detail == "list";
    }
    if family == "host" && operation == "config" {
        return detail == "show";
    }
    // A ticket reply posts to the provider's support desk unless it only
    // prints what it would send.
    if family == "quota" && matches!(operation, "request" | "ticket") {
        return (operation == "request" && detail == "list")
            || (operation == "ticket" && args.iter().any(|arg| arg == "--dry-run"));
    }
    if family == "service" && operation == "env" {
        return matches!(detail, "show" | "check");
    }
    // `auth check --repair` synchronizes the secret and restarts the unit.
    if family == "service" && operation == "auth" {
        return detail == "check" && !args.iter().any(|arg| arg == "--repair");
    }
    if family == "service" && operation == "grant" {
        return detail == "show";
    }
    // `web origin` reads and writes under one operation word, so the third
    // word decides. `converge` is mutating even without `--apply`, because
    // the flag is the difference between a plan and a change and a Desktop
    // that could run the plan unconfirmed would be one flag away from running
    // the change: the confirmation belongs to the verb, not to the flag.
    if family == "web" && operation == "origin" {
        return matches!(detail, "list" | "status");
    }
    // The listing preview reads the queue and calls no marketplace, but only
    // when it is bounded: a loop cannot answer a request, so the graphical
    // surface asks for exactly one evaluation.
    if family == "market" && operation == "auto-list" {
        return args.iter().any(|arg| arg == "--dry-run") && args.iter().any(|arg| arg == "--once");
    }
    if matches!(
        family,
        "capabilities"
            | "overview"
            | "profiles"
            | "status"
            | "doctor"
            | "results"
            | "cost"
            | "blast-radius"
    ) {
        return true;
    }
    matches!(
        (family, operation),
        (
            "artifact",
            "list" | "show" | "resolve" | "verify" | "lineage"
        ) | ("billing", "show")
            | (
                "fleet",
                "list" | "status" | "catalog" | "doctor" | "pending" | "methods"
            )
            | (
                "host",
                "health"
                    | "inventory"
                    | "uptime"
                    | "ping"
                    | "gates"
                    | "link"
            )
            | ("identity", "list" | "verify")
            | (
                "inference",
                "list" | "status" | "logs" | "plan-logs" | "doctor" | "verify" | "blockers"
            )
            | ("instances", "list")
            | ("machine", "status" | "logs" | "artifacts")
            | ("optimize", "status" | "explain")
            | ("queue", "status")
            | ("quota", "show" | "catalog")
            | (
                "registry",
                "validate" | "pull" | "self" | "doctor"
            )
            | ("resources", "show" | "verify" | "operations")
            | (
                "runner",
                "list" | "status" | "report" | "credential" | "diagnostics"
            )
            | ("schedule", "list" | "show")
            | ("credentials", "ls" | "doctor")
            | (
                "service",
                "directory"
                    | "list"
                    | "catalog"
                    | "onboarding-catalog"
                    | "status"
                    | "show"
                    | "logs"
            )
            | (
                "storage",
                "ls" | "stat" | "cat" | "verify" | "objects" | "url"
            )
            | ("market", "status" | "readiness" | "monitor")
            | ("web", "status")
            | ("alerts", "channels")
    )
}

/// `stado product` operations that only read. The global `--catalog PATH` may
/// precede the operation, so it is skipped before the operation is named.
fn product_read_only(args: &[String]) -> bool {
    let mut words = Vec::new();
    let mut rest = args.iter().skip(1);
    while let Some(word) = rest.next() {
        if word == "--catalog" {
            rest.next();
        } else if !word.starts_with("--catalog=") {
            words.push(word.as_str());
        }
    }
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    match words.first().copied().unwrap_or("") {
        "catalog" => !flag("--output"),
        "status" | "paths" | "documentation" => true,
        "signing" => matches!(
            words.get(1).copied().unwrap_or(""),
            "inspect" | "report" | "residue"
        ),
        "sync" => flag("--dry-run"),
        "create" => flag("--status"),
        _ => false,
    }
}
