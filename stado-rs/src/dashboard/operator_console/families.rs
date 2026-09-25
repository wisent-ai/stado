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
    "azure",
    "billing",
    "blast-radius",
    "bootstrap",
    "build",
    "cancel",
    "capabilities",
    "cloudflare",
    "config",
    "cost",
    "credentials",
    "disk-cleanup",
    "doctor",
    "fleet",
    "host",
    "identity",
    "inference",
    "install-disk-cleanup",
    "instances",
    "job",
    "machine",
    "mail",
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
    "secrets",
    "service",
    "space",
    "status",
    "storage",
    "submit",
    "vast",
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
    if family == "workload" {
        return matches!(operation, "list" | "status");
    }
    if family == "repair" {
        return !args.iter().any(|arg| arg == "--apply");
    }
    if family == "route" {
        return matches!(operation, "list" | "capability");
    }
    if family == "scratch" {
        return matches!(operation, "profiles" | "hosts" | "list")
            || (operation == "reap" && !args.iter().any(|arg| arg == "--apply"));
    }
    if family == "credentials" {
        return operation == "vaults"
            || operation == "seed-freshness"
            || (operation == "item"
                && (detail == "show"
                    || (detail == "retag" && !args.iter().any(|arg| arg == "--tags"))))
            || (operation == "grant" && detail == "show")
            || (operation == "vault"
                && (detail.is_empty()
                    || (detail == "sync" && args.iter().any(|arg| arg == "--check"))))
            || (operation == "backup"
                && detail == "audit"
                && !args.iter().any(|arg| arg == "--apply"));
    }
    if family == "release" {
        return matches!(
            operation,
            "status" | "provenance" | "logs" | "doctor" | "active-binary"
        ) || (operation == "host-state" && !args.iter().any(|arg| arg == "--apply"))
            || (operation == "catalog" && detail == "audit");
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
    if family == "azure" && operation == "unusual-activity" {
        return detail == "diagnose";
    }
    if family == "host" && operation == "exec" {
        return is_retained_log_request(args);
    }
    // `web origin` reads and writes under one operation word, so the third
    // word decides. `converge` is mutating even without `--apply`, because
    // the flag is the difference between a plan and a change and a Desktop
    // that could run the plan unconfirmed would be one flag away from running
    // the change: the confirmation belongs to the verb, not to the flag.
    if family == "web" && operation == "origin" {
        return matches!(detail, "list" | "status");
    }
    // The bridge's preview reads the queue and calls no Vast endpoint, but
    // only when it is bounded: a daemon cannot answer a request, so the
    // graphical surface asks for exactly one evaluation.
    if family == "vast" && operation == "auto-list" {
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
                "health" | "inventory" | "uptime" | "ping" | "vaults"
            )
            | ("identity", "list" | "verify")
            | (
                "inference",
                "list" | "status" | "logs" | "plan-logs" | "doctor" | "verify" | "blockers"
            )
            | ("instances", "list")
            | ("machine", "status" | "logs" | "artifacts")
            | ("optimize", "status" | "explain")
            | ("placement", "relief")
            | ("queue", "status")
            | ("quota", "show" | "catalog" | "requests" | "azure-replies")
            | (
                "registry",
                "validate" | "pull" | "self" | "doctor" | "beacon-age"
            )
            | ("resources", "show" | "verify" | "operations")
            | (
                "runner",
                "list" | "status" | "report" | "credential" | "diagnostics"
            )
            | ("schedule", "list" | "show")
            | ("secrets", "ls" | "doctor" | "inspect-vault")
            | (
                "service",
                "directory"
                    | "list"
                    | "onboarding-catalog"
                    | "status"
                    | "show"
                    | "logs"
                    | "env"
                    | "auth-check"
            )
            | (
                "storage",
                "ls" | "stat" | "cat" | "verify" | "objects" | "url"
            )
            | ("vast", "status" | "readiness" | "monitor")
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
        "schedule" => !flag("--install") && !flag("--remove"),
        "sync" => flag("--dry-run"),
        "create" => flag("--status"),
        _ => false,
    }
}
