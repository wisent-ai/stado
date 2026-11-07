//! The register's entries.

use super::{Base, Entry, Kind, Reclaim};
use crate::providers::local::{space_reclaim, work_base};

/// The trees a runner keeps beside its install: the job checkouts and build
/// output of `_work`, what its jobs stage in `.tmp`, and the listener's and
/// workers' diagnostic logs in `_diag`. The `runner_work_trees` stage of
/// `stado space reclaim` takes each one while no runner job is in flight.
pub const RUNNER_SCRATCH: &[&str] = &["_work", ".tmp", "_diag"];

/// The `stado space reclaim` stage that takes a runner's scratch.
const RUNNER_STAGE: &str = "runner_work_trees";

/// The `stado space reclaim` stage that takes a managed run tree.
pub const MANAGED_RUNS_STAGE: &str = "managed_runs";

/// The managed run area and its signal markers, below the home: the paths
/// `stado host run` and every delivery confine their programs to.
pub const RUN_AREA: &str = ".stado/work/runs";
pub const SIGNAL_AREA: &str = ".stado/work/run-signals";

/// What Stado keeps under `~/.stado` for itself — its programs, state,
/// caches, logs and services — each written and read by Stado alone and
/// kept: a pass removes from inside them what a cleaner names, never them.
const STADO_STATE: &[&str] = &[
    "actions-runner-brama", "apple-developer-id", "attestations", "audit", "auth-check",
    "backups", "bin", "box_diagnostics", "cache", "change-queue", "cron-backups",
    "database-backups", "databases", "drain", "files", "forwards", "host-keys", "host-store",
    "inference", "libexec", "local-backup", "local-storage", "logs", "market", "marketplace",
    "placement", "placement-history", "products", "provenance", "recovery", "registered-runner",
    "release-state", "releases", "role-handoffs", "roles", "run", "schedules", "scratch",
    "services", "skarbiec-unlock", "space", "test-runs", "waits", "web", "web-edge",
    "weles-detached-runs",
];

/// The account's own trees and the tools the host was provisioned with,
/// at the top of the home of a host the fleet owns.
const ACCOUNT: &[&str] = &[
    ".bun", ".cache", ".cargo", ".claude", ".codex", ".config", ".docker", ".factory",
    ".gnupg", ".jeden", ".kimi-code", ".local", ".npm", ".omp", ".oko", ".pki", ".rustup",
    ".ssh", ".tama", "Library", "snap",
];

/// What a runner's install keeps beside its scratch: its program, the
/// tools it ships and its Kronika signing identity.
const RUNNER_INSTALL: &[&str] = &["bin", "externals", ".stado"];

/// Every runner root this build's runner profiles declare, on either
/// platform: `/opt/wisent/<slug>-runner` and `/Users/Shared/<slug>-runner`.
/// A declaration that does not parse declares none, and says why.
pub fn runner_roots() -> Vec<String> {
    use crate::deploy::host_precheck_runner::{runner_declaration, Platform};
    match runner_declaration() {
        Ok(declaration) => declaration
            .profiles
            .iter()
            .flat_map(|profile| {
                [Platform::LinuxAmd64, Platform::DarwinArm64]
                    .map(|platform| platform.runner_root(profile))
            })
            .collect(),
        Err(error) => {
            eprintln!("stado: no runner root is known: {error}");
            Vec::new()
        }
    }
}

/// One entry the fleet's own verbs write.
fn writer(
    name: &'static str,
    base: Base,
    path: &str,
    writers: &'static [&'static str],
    readers: &'static [&'static str],
    reclaim: Reclaim,
) -> Entry {
    Entry {
        name,
        base,
        path: path.to_string(),
        kind: Kind::Writer,
        writers,
        readers,
        reclaim,
    }
}

/// Every entry of the register.
pub fn entries() -> Vec<Entry> {
    let mut entries = vec![
        writer(
            "job_trees",
            Base::WorkBase,
            work_base::JOBS_LEAF,
            &["stado agent (a job's start)"],
            &["stado workload results", "stado release worker", "stado space reclaim"],
            Reclaim::By(space_reclaim::queue_workdirs::CLEANER),
        ),
        writer(
            "session_checkouts",
            Base::WorkRoot,
            work_base::SESSION_CHECKOUTS_LEAF,
            &["stado workload start omp-session", "stado workload start jeden-session", "stado workload attach jeden-session"],
            &["stado space reclaim"],
            Reclaim::By(space_reclaim::queue_workdirs::session_checkouts::CLEANER),
        ),
        writer(
            "jeden_ledgers",
            Base::WorkRoot,
            work_base::JEDEN_SESSIONS_LEAF,
            &["stado workload start jeden-session", "stado workload attach jeden-session"],
            &["stado workload attach jeden-session --resume"],
            Reclaim::Kept("Jeden's session ledgers, read back to resume a session"),
        ),
        writer(
            "job_package_caches",
            Base::WorkRoot,
            work_base::PACKAGE_CACHES_LEAF,
            &["stado agent (a job's package managers)"],
            &["stado agent (a job's package managers)"],
            Reclaim::By(space_reclaim::package_caches::CLEANER),
        ),
        writer(
            "build_cache",
            Base::WorkRoot,
            work_base::BUILD_CACHE_LEAF,
            &["stado release worker"],
            &["stado release worker"],
            Reclaim::By("build_caches"),
        ),
        writer(
            "placement_staging",
            Base::WorkRoot,
            "placement",
            &["stado placement transfer"],
            &["stado placement transfer"],
            Reclaim::Kept("a transfer's staged copy, removed by the transfer that staged it"),
        ),
        writer(
            "managed_runs",
            Base::Home,
            RUN_AREA,
            &["stado host run deliver", "stado host run cargo", "stado workload harness install", "stado host upgrade"],
            &["stado host run attach", "stado host run remove"],
            Reclaim::By(MANAGED_RUNS_STAGE),
        ),
        writer(
            "run_signals",
            Base::Home,
            SIGNAL_AREA,
            &["stado host run attach"],
            &["stado host run signal"],
            Reclaim::By(MANAGED_RUNS_STAGE),
        ),
        Entry {
            kind: Kind::Legacy,
            ..writer(
                "legacy_session_checkouts",
                Base::Home,
                space_reclaim::queue_workdirs::session_checkouts::HOME_ROOT,
                &["stado workload start omp-session (earlier releases)"],
                &["stado space reclaim"],
                Reclaim::By(space_reclaim::queue_workdirs::session_checkouts::CLEANER),
            )
        },
    ];
    for cleaner in space_reclaim::rule::CLEANERS
        .iter()
        .filter(|cleaner| !cleaner.root.is_empty())
    {
        entries.push(writer(
            cleaner.name,
            Base::Home,
            cleaner.root,
            &["the fleet's jobs, deliveries and services"],
            &["stado space reclaim"],
            Reclaim::By(cleaner.name),
        ));
    }
    for name in STADO_STATE {
        entries.push(writer(
            name,
            Base::Home,
            &format!(".stado/{name}"),
            &["stado"],
            &["stado"],
            Reclaim::Kept("Stado's own programs and state; a cleaner takes what it names inside"),
        ));
    }
    for name in ACCOUNT {
        entries.push(Entry {
            kind: Kind::Account,
            ..writer(
                name,
                Base::Home,
                name,
                &["the account and the tools the host was provisioned with"],
                &["the account"],
                Reclaim::Kept("the account's own; a cleaner takes what it names inside"),
            )
        });
    }
    for scratch in RUNNER_SCRATCH {
        entries.push(writer(
            "runner_scratch",
            Base::Runner,
            scratch,
            &["the host's GitHub runners"],
            &["the host's GitHub runners"],
            Reclaim::By(RUNNER_STAGE),
        ));
    }
    for install in RUNNER_INSTALL {
        entries.push(writer(
            "runner_install",
            Base::Runner,
            install,
            &["stado host runner install"],
            &["the host's GitHub runners"],
            Reclaim::Kept("the runner's own program and signing identity"),
        ));
    }
    entries
}
