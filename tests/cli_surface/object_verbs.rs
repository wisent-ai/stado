//! Every host-side half is a visible verb of the object it acts on, and the
//! hyphenated names it replaced no longer parse. The real `stado` binary of
//! this checkout runs in an isolated home: `host local port` hands out a port
//! this host binds, the help of each object lists its verbs, and each retired
//! name is refused by the parser with the word it did not recognise.
mod harness;

use harness::{help_lists, refused_by_parser, Run};

#[test]
fn every_host_side_half_is_a_visible_verb_and_no_hyphenated_name_parses() {
    let mut run = Run::start();

    let port = run.stado(&["host", "local", "port"]);
    let printed = String::from_utf8_lossy(&port.stdout);
    assert!(
        port.status.success(),
        "host local port: {}",
        String::from_utf8_lossy(&port.stderr)
    );
    let port: u16 = match printed.trim().parse() {
        Ok(port) => port,
        Err(error) => panic!("host local port printed {printed:?}: {error}"),
    };
    if let Err(error) = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)) {
        panic!("the port {port} this host handed out cannot be bound: {error}");
    }

    help_lists(
        &mut run,
        &["host", "local"],
        &["run", "api", "repair", "audit", "port", "reconcile"],
    );
    help_lists(
        &mut run,
        &["host", "local", "api"],
        &["paths", "plist", "boundary", "route", "skarbiec"],
    );
    help_lists(
        &mut run,
        &["host", "local", "api", "plist"],
        &["render", "equal"],
    );
    help_lists(&mut run, &["host", "local", "api", "boundary"], &["ready"]);
    help_lists(
        &mut run,
        &["host", "local", "api", "skarbiec"],
        &["plan", "ownership", "upstream", "proxy"],
    );
    help_lists(
        &mut run,
        &["host", "local", "reconcile", "step"],
        &["launch", "unit", "listener", "object", "store"],
    );
    help_lists(
        &mut run,
        &["host", "local", "reconcile", "step", "unit"],
        &["snapshot", "restore"],
    );
    help_lists(
        &mut run,
        &["host", "local", "reconcile"],
        &["phase", "step", "worker"],
    );
    help_lists(
        &mut run,
        &["release", "local"],
        &["install", "restore", "list", "converge"],
    );
    help_lists(&mut run, &["release", "active"], &["binary", "dir"]);
    help_lists(&mut run, &["release", "delivery"], &["worker"]);
    help_lists(&mut run, &["credentials", "item"], &["summarize"]);
    help_lists(
        &mut run,
        &["credentials", "token"],
        &["register", "custody"],
    );
    help_lists(&mut run, &["space", "file", "local"], &["retire", "remove"]);
    help_lists(&mut run, &["service"], &["roles"]);
    help_lists(&mut run, &["service", "env"], &["write"]);
    help_lists(&mut run, &["service", "image"], &["refresh"]);
    help_lists(&mut run, &["host", "gpu"], &["power"]);
    help_lists(&mut run, &["host", "gpu", "power"], &["set", "unset"]);
    help_lists(&mut run, &["host", "port"], &["owner"]);
    help_lists(&mut run, &["host", "document"], &["render"]);
    help_lists(&mut run, &["host", "compiler"], &["cache"]);
    help_lists(
        &mut run,
        &["release", "gate"],
        &[
            "surface",
            "baseline",
            "decide",
            "conformance",
            "semver",
            "modules",
            "app",
        ],
    );
    help_lists(&mut run, &["release", "gate", "semver"], &["reaches"]);
    help_lists(&mut run, &["release", "gate", "modules"], &["unreachable"]);
    help_lists(
        &mut run,
        &["release", "gate", "app"],
        &["surface", "baseline", "check"],
    );

    refused_by_parser(&mut run, &["host", "free-port-local"], "free-port-local");
    refused_by_parser(&mut run, &["host", "gpu-power-limit"], "gpu-power-limit");
    refused_by_parser(&mut run, &["host", "port-owner"], "port-owner");
    refused_by_parser(
        &mut run,
        &["host", "render-public-document"],
        "render-public-document",
    );
    refused_by_parser(&mut run, &["host", "compiler-cache"], "compiler-cache");
    refused_by_parser(&mut run, &["release", "version-gate"], "version-gate");
    refused_by_parser(
        &mut run,
        &["release", "gate", "semver-at-least"],
        "semver-at-least",
    );
    refused_by_parser(
        &mut run,
        &["release", "gate", "unreachable-modules"],
        "unreachable-modules",
    );
    refused_by_parser(&mut run, &["release", "gate", "app-check"], "app-check");
    refused_by_parser(&mut run, &["host", "run-locked"], "run-locked");
    refused_by_parser(&mut run, &["host", "object-api-local"], "object-api-local");
    refused_by_parser(
        &mut run,
        &["host", "release-store-repair-local"],
        "release-store-repair-local",
    );
    refused_by_parser(
        &mut run,
        &["host", "backup-audit-local"],
        "backup-audit-local",
    );
    refused_by_parser(
        &mut run,
        &["host", "storage-root-reconcile-local"],
        "storage-root-reconcile-local",
    );
    refused_by_parser(
        &mut run,
        &["host", "storage-root-reconcile-host"],
        "storage-root-reconcile-host",
    );
    refused_by_parser(
        &mut run,
        &["host", "storage-root-reconcile-worker"],
        "storage-root-reconcile-worker",
    );
    refused_by_parser(&mut run, &["release", "install-local"], "install-local");
    refused_by_parser(&mut run, &["release", "restore-local"], "restore-local");
    refused_by_parser(
        &mut run,
        &["host", "local", "api", "render-plist"],
        "render-plist",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "api", "plist-equal"],
        "plist-equal",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "api", "boundary-ready"],
        "boundary-ready",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "api", "skarbiec-plan"],
        "skarbiec-plan",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "api", "skarbiec-proxy-match"],
        "skarbiec-proxy-match",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "reconcile", "step", "unit-snapshot"],
        "unit-snapshot",
    );
    refused_by_parser(
        &mut run,
        &["host", "local", "reconcile", "step", "served-store"],
        "served-store",
    );
    refused_by_parser(
        &mut run,
        &["release", "converge-local-readers"],
        "converge-local-readers",
    );
    refused_by_parser(&mut run, &["release", "active-binary"], "active-binary");
    refused_by_parser(&mut run, &["release", "active-dir"], "active-dir");
    refused_by_parser(&mut run, &["release", "delivery-worker"], "delivery-worker");
    refused_by_parser(
        &mut run,
        &["credentials", "item", "summarize-local"],
        "summarize-local",
    );
    refused_by_parser(
        &mut run,
        &["credentials", "token", "register-item-local"],
        "register-item-local",
    );
    refused_by_parser(
        &mut run,
        &["credentials", "token", "custody-local"],
        "custody-local",
    );
    refused_by_parser(&mut run, &["space", "file", "retire-local"], "retire-local");
    refused_by_parser(&mut run, &["space", "file", "remove-local"], "remove-local");
    refused_by_parser(&mut run, &["service", "serve-roles"], "serve-roles");
    refused_by_parser(&mut run, &["service", "refresh-image"], "refresh-image");
    refused_by_parser(&mut run, &["service", "unit-env-local"], "unit-env-local");

    run.passed();
}
