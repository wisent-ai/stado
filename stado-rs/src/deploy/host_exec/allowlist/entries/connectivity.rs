//! What a connectivity gap leaves behind, and the provider sign-in repairs —
//! the only entries in this table that change anything.

use super::super::arguments::{LINUX_TAILSCALE_LOG_READ, MACOS_TAILSCALE_LOG_READ};
use super::super::programs::{BRAMA_LAUNCHER, KIMI_CLI, TAILSCALE_PROGRAM};
use super::super::ApprovedCommand;

pub const CONNECTIVITY_AND_SIGN_IN: &[ApprovedCommand] = &[
    // The four reads a connectivity gap needs, added 2026-08-19. Between
    // 18:29 and 18:35 UTC control-host answered no ping and no ssh, then
    // came back on a direct path; every fact about that gap — when the host
    // slept and woke, whether its path was direct or relayed and to which
    // endpoint, whether its own view of the tailnet was degraded, and which
    // interface had dropped — was read by an operator over a private ssh
    // session, eleven times, because no sanctioned path existed. These are
    // that path.
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
              the public ones every node publishes. This is where `direct 10.0.0.253:41641` \
              comes from, the line that said the 2026-08-19 gap had ended",
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
              on 2026-08-24 left Jeden reading bare 404s from Brama while every beacon said \
              active, and could only be diagnosed from outside the host",
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
    // The Linux half of the interface read, added 2026-09-02.
    // `stado host exec ubuntu-server-rtx-pro-6000 -- ifconfig -a` fails with
    // `/sbin/ifconfig: No such file or directory`, because Ubuntu ships
    // iproute2 and not net-tools, so the entry above answers for the macOS
    // hosts and for no other kind of machine in the fleet.
    ApprovedCommand {
        argv: &["/usr/bin/ip", "addr"],
        why: "lists every network interface on a Linux host with the addresses it carries — the \
              same fact the `ifconfig -a` entry above reads, on the hosts where that entry \
              cannot run. Ubuntu ships iproute2 and not net-tools, so \
              `host exec ubuntu-server-rtx-pro-6000 -- ifconfig -a` answers \
              `/sbin/ifconfig: No such file or directory` and the fleet's one approved way to \
              read a host's interfaces was a macOS-only read; the address of the fleet's only \
              Linux host had to be inferred from `tailscale netcheck` instead, which reports \
              the reflexive address a relay observed and not one word about what the \
              interfaces on the machine actually hold. `addr` with no object and no operand is \
              iproute2's read-only listing form: every form that changes an address takes \
              `add`, `del`, `change`, `replace` or `flush` after it, none of which is in this \
              table and none of which can be appended, because the allowlist matches an entry \
              exactly and never appends operator words. What it prints are the addresses the \
              registry already holds",
    },
    // The three sign-in repairs, added 2026-09-02. These are the only entries
    // in this table that change anything, and they are here because the thing
    // they change cannot be reached any other way: a provider grant the vendor
    // has disowned is replaced by one browser sign-in, that sign-in belongs to
    // Brama's own CLI on the host whose vault the gateway reads, and the vault
    // that matters is never this control plane's. `brama-sub-wisent-app-codex-primary`
    // was recorded `needs_reauthorization` on 2026-08-27 with the provider's own
    // sentence -- "Your session has ended. Please log in again." -- and from that
    // moment every model call the fleet routed through that gateway had one live
    // provider and no way for an operator to repair it without a private ssh
    // session outside the registry-authorized channel. Each entry names one
    // provider, one exact Weles sign-in row, and its own fixed reason. The row
    // is named rather than inferred because Weles holds seven codex accounts
    // and two claude ones, and the cost of getting that wrong is one real
    // sign-in into somebody else's account; the reason is fixed because it is
    // recorded in Brama's journal beside the verdict and an operator-supplied
    // one would be an operator-supplied argument.
    //
    // The login budget is deliberately above the ten minutes the reauth
    // trajectory's own `login.mjs` allows itself. Setting the two equal, which
    // this table did first, meant the outer kill landed in the same second as
    // the inner cap: Weles answered 502 with no run detail and the trajectory
    // was SIGKILLed before it could write the page it was stuck on, which is
    // the one artifact the operator actually needs. The inner cap must be the
    // one that fires.
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "subscription",
            "sign-in",
            "codex",
            "--login-item",
            "codex-wisent-google-sso",
            "--reason",
            "codex-grant-disowned-2026-08-27-gateway-has-one-live-provider",
            "--login-timeout-ms",
            "900000",
            "--json",
        ],
        why: "asks Weles to sign the codex account in on the host that holds the vault, then \
              proves the repair by Brama's own refresh. The row is the one Weles declares \
              primary for codex and maps to `brama-sub-wisent-app-codex-primary`, which is \
              the subscription the provider disowned; it is also the row Brama's own renewal \
              sweep already drives, so this entry cannot reach an account that sweep would \
              not. It changes exactly one thing: that subscription's stored provider \
              credential. It cannot spend money, because a sign-in buys nothing. No \
              credential reaches this command: Weles writes what it mints into the vault \
              directly, the admission bearer is acquired on the host, and the verdict this \
              prints carries a result, a reason and a login row and never a secret",
    },
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "subscription",
            "sign-in",
            "claude-code",
            "--login-item",
            "claude-wisent-google-sso",
            "--reason",
            "claude-code-vault-row-yields-no-credential-second-live-provider",
            "--login-timeout-ms",
            "900000",
            "--json",
        ],
        why: "the same repair for claude-code, whose stored document is account metadata \
              carrying no credential material: its pool contributes no model at all, and a \
              sign-in is what would put a credential there. A gateway with one live provider \
              is a gateway that stops serving at the next lapsed session, which is the state \
              this fleet was in on 2026-08-27. The row is Weles's declared primary for \
              claude, mapped to `brama-sub-wisent-app-claude-primary`. Same guarantees as \
              the codex entry: no argument, no purchase, no secret in argv or output",
    },
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "subscription",
            "sign-in",
            "kimi",
            "--login-item",
            "kimi-lukasz-google-sso",
            "--reason",
            "kimi-vault-row-yields-no-credential-second-live-provider",
            "--login-timeout-ms",
            "900000",
            "--json",
        ],
        why: "the same repair for kimi, in the same state as claude-code: a stored document \
              with no credential material and a pool that contributes no model. The row is \
              Weles's only kimi account and its declared primary, mapped to \
              `brama-sub-wisent-app-kimi-primary`. Same guarantees as the codex entry",
    },
    // What the installed Kimi CLI actually accepts, added 2026-09-02. Weles's
    // kimi login trajectory spawns `kimi login --json` and the CLI on
    // charless-mac-mini answers `error: unknown option '--json'`, so the run
    // never reaches an authorize URL and kimi has renewed nothing. Fixing a
    // trajectory against a flag list guessed from a pinned version is how that
    // mismatch happened; these three reads are how it gets fixed against the
    // binary that is really there.
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
    // The proof the sign-in entries above are judged by, added 2026-09-02. A
    // repaired credential that redeems is not a repaired gateway: the vault can
    // yield a value the provider then refuses, which is exactly the state
    // 2026-08-27 left, and only a real completion separates the two. It runs
    // through the same subscription dispatch a caller reaches, on the host, so
    // no bearer of any kind crosses this channel.
    ApprovedCommand {
        argv: &[
            BRAMA_LAUNCHER,
            "test",
            "--model",
            "codex/gpt-5.3-codex-spark",
            "--agent-id",
            "wisent-app",
            "--allow-provider-cost",
        ],
        why: "sends one fixed prompt through Brama's subscription dispatch and prints the \
              model, the token counts and the latency. The route is the provider's own \
              cheapest codex model -- the one its plan-probe table already names -- and the \
              request is covered by the subscription the account already holds, so it buys \
              nothing, renews nothing and raises no limit. `--allow-provider-cost` is \
              Brama's own acknowledgement flag and is fixed here because this entry exists \
              to spend exactly one completion; the prompt and the agent are compile-time \
              constants, and the answer carries no credential",
    },
];
