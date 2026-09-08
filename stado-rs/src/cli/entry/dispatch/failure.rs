//! The two low-cardinality keys a failed command is logged under: which
//! command it was, and which dependency axis an operator should reason about.

/// The dotted id of the command that failed, built from the declared
/// subcommand names only — `cli.host.user.create`, never an argument value,
/// so the field stays a low-cardinality key a log query can group by.
pub(super) fn failure_point(matches: &clap::ArgMatches) -> String {
    let mut point = String::from("cli");
    let mut node = matches;
    while let Some((name, sub)) = node.subcommand() {
        point.push('.');
        point.push_str(name);
        node = sub;
    }
    point
}

/// The dependency axis an operator reasons about, coarser than the command
/// tree: when the queue's storage is down, `submit`, `status` and `storage ls`
/// are one incident, not three.
pub(super) fn failure_service(matches: &clap::ArgMatches) -> &'static str {
    match matches.subcommand_name().unwrap_or_default() {
        "submit" | "status" | "cancel" | "results" | "job" | "machine" | "queue" | "storage"
        | "artifact" => "queue",
        "fleet"
        | "host"
        | "space"
        | "registry"
        | "builds"
        | "service"
        | "instances"
        | "resources"
        | "recovery"
        | "bootstrap"
        | "doctor"
        | "disk-cleanup"
        | "install-disk-cleanup" => "fleet",
        "secrets" => "credentials",
        "billing" | "cost" | "quota" => "billing",
        "mail" => "mail",
        "azure" | "cloudflare" | "vast" | "blast-radius" => "provider",
        "coordinator"
        | "resolver"
        | "release"
        | "database"
        | "schedule"
        | "agent"
        | "local-control-plane"
        | "cloud-control-plane" => "control-plane",
        _ => "stado",
    }
}
