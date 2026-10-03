//! Fixed connectivity observations and installed-client diagnostics.

use super::super::arguments::{LINUX_TAILSCALE_LOG_READ, MACOS_TAILSCALE_LOG_READ};
use super::super::programs::{KIMI_CLI, TAILSCALE_PROGRAM};
use super::super::ApprovedCommand;

pub const CONNECTIVITY_READS: &[ApprovedCommand] = &[
    // Read the host's own power, interface and transport observations.
    ApprovedCommand {
        argv: &["/usr/bin/pmset", "-g", "log"],
        why: "prints the power-management event log: every sleep, every wake, and the reason \
              the kernel recorded for each. `-g` is pmset's read-only getter and `log` names \
              the log to print; the verbs that change anything (`sleep`, `displaysleepnow`, \
              `schedule`, `repeat`, and the `-a`/`-b`/`-c` setting forms) are absent from this \
              table and cannot be reached through it, because the allowlist matches an entry \
              exactly and never appends operator words. A host that went quiet because it slept \
              is indistinguishable from one that crashed until this log is read",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "status", "--json"],
        why: "prints this node's own view of the tailnet as JSON: for every peer, whether the \
              current path is direct or through a relay, and which endpoint carries it. \
              `status` is the read-only verb and `--json` changes only the rendering; the verbs \
              that change anything (`up`, `down`, `set`, `login`, `logout`, `serve`, `funnel`) \
              are absent from this table. The output carries node names, tailnet addresses and \
              endpoints — the same addresses the registry already holds — and no keys beyond \
              the public ones every node publishes. Direct and relayed paths are reported \
              from the node's current observation",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "netcheck"],
        why: "reports what this host can reach of the relay mesh: UDP reachability, whether a \
              router will map a port, and the latency to each relay region. It sends probe \
              traffic to Tailscale's own relays and writes nothing on the host and nothing to \
              the tailnet, so it is safe to run against a live machine. It answers the question \
              a one-sided ping cannot — whether the degraded path is this host's or the peer's",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "serve", "status", "--json"],
        why: "prints this node's serve and funnel handler table as JSON: which HTTPS port \
              forwards to which loopback origin, and whether funnel is on. `status` is the \
              read-only verb; the forms that change anything (`serve --bg`, `funnel`, `reset`) \
              are absent from this table because the allowlist matches an entry exactly. This \
              is the read that says a published endpoint 404s because its rule was lost, which \
              leaves a consumer reading bare 404s from the gateway while every beacon says \
              active, and can only be diagnosed from outside the host",
    },
    ApprovedCommand {
        argv: &[TAILSCALE_PROGRAM, "funnel", "status"],
        why: "prints whether funnel is enabled and which origins it publishes. `status` is \
              the read-only verb and takes no operand, so nothing here can publish, retract, \
              or alter a rule. It is the postcondition half of the serve-status read: brama's \
              funnel publisher (brama/scripts/brama-funnel-publisher.sh) verifies itself with \
              exactly this command, so an operator can re-check its verdict by hand",
    },
    ApprovedCommand {
        argv: MACOS_TAILSCALE_LOG_READ,
        why: "reads the last hour of retained macOS logs from Tailscale's application and \
              daemon processes. Serve and Funnel status describe configuration, not why \
              a connection failed. This fixed read neither enables logging nor starts \
              probes, changes configuration, or restarts a process",
    },
    ApprovedCommand {
        argv: LINUX_TAILSCALE_LOG_READ,
        why: "reads the last hour of the Linux tailscaled unit's retained journal, including \
              its original timestamps and failure messages. The unit, time window and \
              output format are fixed; this read starts no network probe and changes \
              neither the service nor its logging configuration",
    },
    ApprovedCommand {
        argv: &["/sbin/ifconfig", "-a"],
        why: "lists every network interface with its addresses and flags. `-a` only widens the \
              selection to interfaces that are not up, which is the whole point: the interface \
              that dropped is the one that is no longer listed by default. There is no \
              interface operand and no address, and every configuring form of ifconfig requires \
              one, so this entry cannot change an address, a route, or an interface's state",
    },
    // Linux interface observations use iproute2 rather than requiring net-tools.
    ApprovedCommand {
        argv: &["/usr/bin/ip", "addr"],
        why: "lists a Linux host's network interfaces and their assigned addresses. \
              `addr` with no object is iproute2's read-only listing form. The allowlist \
              accepts neither address-changing verbs nor additional arguments",
    },
    // Read the installed client's actual interface before selecting a login flow.
    ApprovedCommand {
        argv: &[KIMI_CLI, "--version"],
        why: "prints the installed Kimi CLI version. It takes no argument, reads no session \
              and writes nothing; the version is the first thing a trajectory-versus-CLI \
              mismatch has to be judged against",
    },
    ApprovedCommand {
        argv: &[KIMI_CLI, "--help"],
        why: "prints the CLI's own subcommand list. `--help` short-circuits before any \
              subcommand runs, so nothing logs in, nothing is written, and no account state \
              is read",
    },
    ApprovedCommand {
        argv: &[KIMI_CLI, "login", "--help"],
        why: "prints the flags the `login` subcommand accepts. This is the exact question the \
              broken trajectory needs answered -- whether a machine-readable output flag \
              exists and what it is spelled -- and `--help` is answered by the argument \
              parser before the subcommand body, so no login is started and no browser \
              opens",
    },
];
