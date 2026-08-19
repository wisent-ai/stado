//! `stado host exec TARGET -- CMD…` — run one APPROVED read-only command
//! on a registry host through the shared ssh channel.
//!
//! NO Python original: item six of `docs/missing-commands.md`, whose
//! wording is the whole design — "with an allowlist, not free shell".
//!
//! This is not a remote shell and must never become one. `stado host exec`
//! exists so an operator diagnosing a wedged box does not have to keep a
//! private ssh alias outside the registry-authorized channel; it does not
//! exist to run arbitrary code as the login user of every machine in the
//! fleet.
//!
//! Three independent barriers stand between the operator's words and the
//! host, in this order:
//!
//! 1. **Character rejection.** Every word the operator typed must consist
//!    only of characters no shell treats specially ([`is_shell_safe`]).
//!    Anything carrying `;`, `|`, `&`, `$`, backtick, quote, newline,
//!    redirection or glob is refused by name before anything else happens.
//! 2. **Exact allowlist match.** The words, joined, must equal one entry of
//!    [`APPROVED_COMMANDS`] exactly. There is no prefix match, no
//!    pass-through of extra arguments, and no operator-supplied path — a
//!    command that took a path would be a command that could read
//!    `~/.ssh/id_ed25519`.
//! 3. **Fixed argv.** What actually runs is the matched entry's own
//!    `argv`, a compile-time constant of absolute paths. The operator's
//!    words select an entry; they never become part of the command line.
//!    [`crate::deploy::host_channel::ssh_program_argv`] then shell-quotes
//!    each fixed word for the remote login shell.
//!
//! Barrier 3 alone makes injection impossible, which is precisely why
//! barriers 1 and 2 are worth having: they mean the guarantee does not
//! depend on the table being perfectly curated, and they give the operator
//! a real error instead of a silent mismatch.
//!
//! Every entry is read-only, takes no operator-supplied argument, and
//! carries its own justification in [`ApprovedCommand::why`].

use serde_json::{json, Value};

use super::host_channel;
use super::{py_str_repr, shlex_quote, DeployError, Runner};

/// `status` for a command that ran and exited clean.
pub const OK_STATUS: &str = "ok";

/// The punctuation an operator's word may contain on top of ASCII
/// alphanumerics. Every one of these is inert to `/bin/sh`: no expansion,
/// no word splitting, no redirection, no globbing.
const SAFE_PUNCTUATION: &str = "-_./:";

/// One approved remote program.
#[derive(Debug)]
pub struct ApprovedCommand {
    /// What actually runs: an absolute program path followed by its FIXED
    /// arguments. Nothing the operator types is ever appended to it.
    pub argv: &'static [&'static str],
    /// Why running this unattended, as the registry-managed login user, is
    /// safe. An entry without a defensible answer here does not belong in
    /// the table.
    pub why: &'static str,
}

impl ApprovedCommand {
    /// The spelling an operator types: the program's basename followed by
    /// its fixed arguments. Derived from [`Self::argv`] so the table has
    /// exactly one source of truth.
    pub fn display(&self) -> String {
        let mut words: Vec<&str> = Vec::new();
        if let Some((program, arguments)) = self.argv.split_first() {
            words.push(program.rsplit('/').next().unwrap_or(program));
            words.extend(arguments.iter().copied());
        }
        words.join(" ")
    }

    /// Every absolute path this entry's program may be installed at, in probe
    /// order. A one-element slice — `argv[0]` itself — for every program that
    /// lives in exactly one place.
    pub fn candidates(&self) -> &'static [&'static str] {
        let Some((program, _)) = self.argv.split_first() else {
            return &[];
        };
        PROGRAM_CANDIDATES
            .iter()
            .find(|(named, _)| named == program)
            .map_or(std::slice::from_ref(program), |(_, paths)| paths)
    }
}

/// The tailscale CLI, as the fleet's Linux hosts install it.
///
/// It is `argv[0]` of the two tailscale entries, so [`ApprovedCommand::display`]
/// spells them `tailscale …` — the name of the program, in the case every
/// operator and every script in this repository types it. On macOS the same CLI
/// ships inside the application bundle instead, which is why the entry needs
/// [`PROGRAM_CANDIDATES`]: one program, two install layouts, one spelling.
const TAILSCALE_PROGRAM: &str = "/usr/bin/tailscale";

/// Every absolute path a program in this table is installed at, for the
/// programs whose location differs per platform.
///
/// The order is the one every other reader in this repository already probes
/// (`scripts/diagnose-tailscale-serve-host.sh`,
/// `scripts/reconcile-stado-object-tailnet-route-host.sh`, the enrolment
/// script in `cli/fleet/invite.rs`), so `host exec` cannot disagree with them
/// about which binary is the tailscale CLI on a given host.
///
/// A program absent from this table has exactly one path — its `argv[0]` — and
/// keeps the plain [`host_channel::run_program`] transport.
const PROGRAM_CANDIDATES: &[(&str, &[&str])] = &[(
    TAILSCALE_PROGRAM,
    &[
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        TAILSCALE_PROGRAM,
    ],
)];

/// The allowlist.
///
/// Declared as a slice, not an array, so adding an entry never means
/// touching a length. Ordered roughly by how often an operator reaches for
/// it while a box is misbehaving.
pub const APPROVED_COMMANDS: &[ApprovedCommand] = &[
    ApprovedCommand {
        argv: &["/usr/bin/uptime"],
        why: "reads kernel uptime and load counters; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/df", "-h"],
        why: "reads mounted-filesystem statistics; -h is a fixed display unit, and with no \
              path argument it cannot be pointed at anything",
    },
    ApprovedCommand {
        argv: &["/usr/bin/du", "-xk", "-d", "2", "/"],
        why: "attributes a full root filesystem two directory levels deep; -x stays on one \
              filesystem, -k is a fixed unit, the depth and the root are fixed words, and du \
              writes nothing. Added 2026-08-19: the linux builder sat at 100% used and every \
              declared cleaner and reclaim stage measured zero, so the operator had no \
              sanctioned way to even name what was eating the disk",
    },
    ApprovedCommand {
        argv: &["/usr/bin/who"],
        why: "reads the login-session table; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/launchctl", "list"],
        why: "lists the calling user's launchd jobs. `list` without a label is the read-only \
              verb; the mutating verbs (bootout, bootstrap, kickstart, enable) are absent from \
              this table and cannot be reached through it. This is the view the unmanaged \
              weles-api agent shows up in",
    },
    ApprovedCommand {
        argv: &[
            "/bin/ps", "ax", "-o", "pid", "-o", "ppid", "-o", "etime", "-o", "comm",
        ],
        why: "lists process identifiers, elapsed time, and executable names without command \
              arguments or environment values; it is read-only and cannot expose secret argv",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/netstat", "-anv", "-p", "tcp"],
        why: "reads the kernel TCP socket table without connecting to any endpoint; fixed \
              flags expose listeners and owning processes but accept no remote address",
    },
    ApprovedCommand {
        argv: &["/usr/sbin/lsof", "-nP", "-iTCP", "-sTCP:LISTEN"],
        why: "names the process behind every listening TCP port. `netstat -anv -p tcp` above \
              never shows an owner, so the one question the fleet asks most often - which \
              process holds this port - was answered over ssh instead. The flags fix the \
              selection to listeners and take no argument, so it cannot be pointed at a file, \
              a user, or a remote address",
    },
    ApprovedCommand {
        argv: &["/usr/bin/uname", "-a"],
        why: "prints kernel identification; -a only widens the fields, and there is no input \
              to interpret",
    },
    ApprovedCommand {
        argv: &["/usr/bin/sw_vers"],
        why: "prints the macOS product and build version; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/usr/bin/vm_stat"],
        why: "prints Mach virtual-memory statistics; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/hostname", "-f"],
        why: "prints the fully qualified hostname; -f only selects the long form. Reading it \
              is how a registry `hostnames` entry gets checked against the box itself",
    },
    ApprovedCommand {
        argv: &["/usr/bin/id"],
        why: "prints the login user's uid, gid and groups; takes no argument and writes nothing",
    },
    ApprovedCommand {
        argv: &["/bin/date", "-u"],
        why: "prints the current UTC clock; -u only selects the timezone. Clock skew is a real \
              cause of refused ssh keys and failed storage authentication, so it is worth \
              being able to read",
    },
    ApprovedCommand {
        argv: &["/usr/bin/defaults", "read", "MobileMeAccounts"],
        why: "prints which Apple accounts the login user is signed into. Some work runs only \
              on the machine that holds an identity -- a two-factor prompt appears on the \
              trusted device and nowhere else -- so `stado identity verify` must check the \
              binding rather than trust a declaration nothing re-reads. The domain is fixed, \
              `read` is the read-only verb, and the output carries account identifiers, never \
              tokens or passwords",
    },
    ApprovedCommand {
        argv: &["/usr/bin/dscl", ".", "-list", "/Users"],
        why: "lists the local account names on the host. An identity binding may name a user \
              other than the login user, and `defaults read` answers only for whoever the \
              channel logs in as, so such a binding reports unknown forever. Whether that user \
              exists at all is the question that separates a real gap from a declaration \
              nobody can ever satisfy. `.` is the local node, `-list` is the read-only verb, \
              and account names are not secrets",
    },
    ApprovedCommand {
        argv: &[
            "/usr/sbin/sysctl",
            "-n",
            "kern.maxproc",
            "kern.maxprocperuid",
        ],
        why: "reads two named kernel tunables — the system-wide and per-uid process ceilings — \
              and nothing else. `-n` prints values without names, the two keys are compile-time \
              constants rather than an operator-supplied name, and neither carries secret data. \
              This is the pair that says whether a host refusing to fork is out of process slots \
              or actually wedged; without it `host inventory` reporting probe_failed has no \
              follow-up question",
    },
    ApprovedCommand {
        argv: &["/bin/ps", "ax", "-o", "user", "-o", "pid", "-o", "comm"],
        why: "lists which login user owns each process, by executable name only. `-o comm` is \
              the executable's name; `-o command` — the full argv, where tokens and passwords \
              are passed — is deliberately NOT in this table and cannot be reached through it, \
              because the allowlist matches an entry exactly and never appends operator words. \
              Answers 'whose process is holding that port', which the pid-only listing cannot",
    },
    // The four reads a connectivity gap needs, added 2026-08-19. Between
    // 18:29 and 18:35 UTC charless-mac-mini answered no ping and no ssh, then
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
        argv: &["/sbin/ifconfig", "-a"],
        why: "lists every network interface with its addresses and flags. `-a` only widens the \
              selection to interfaces that are not up, which is the whole point: the interface \
              that dropped is the one that is no longer listed by default. There is no \
              interface operand and no address, and every configuring form of ifconfig requires \
              one, so this entry cannot change an address, a route, or an interface's state",
    },
];

/// Every approved spelling, comma-separated, for help and error text.
pub fn allowlist() -> String {
    APPROVED_COMMANDS
        .iter()
        .map(ApprovedCommand::display)
        .collect::<Vec<String>>()
        .join(", ")
}

/// True when a word contains nothing a shell would act on.
pub fn is_shell_safe(word: &str) -> bool {
    !word.is_empty()
        && word.chars().all(|character| {
            character.is_ascii_alphanumeric() || SAFE_PUNCTUATION.contains(character)
        })
}

/// Resolve the operator's words to an approved entry, or refuse.
///
/// The refusal always carries the full allowlist: an operator who guessed
/// wrong should not have to go read the source to find out what is
/// available.
pub fn approve(words: &[String]) -> Result<&'static ApprovedCommand, DeployError> {
    if words.is_empty() {
        return Err(DeployError(format!(
            "no command given; approved commands: {}",
            allowlist()
        )));
    }
    for word in words {
        if !is_shell_safe(word) {
            return Err(DeployError(format!(
                "argument {} contains a character a shell would interpret; \
                 host exec is an allowlist, not a shell. Approved commands: {}",
                py_str_repr(word),
                allowlist()
            )));
        }
    }
    let requested = words.join(" ");
    APPROVED_COMMANDS
        .iter()
        .find(|candidate| candidate.display() == requested)
        .ok_or_else(|| {
            DeployError(format!(
                "{} is not an approved host-exec command; approved commands: {}",
                py_str_repr(&requested),
                allowlist()
            ))
        })
}

/// The remote program for an entry whose binary is installed at a different
/// absolute path on each platform: run the first candidate that is executable
/// on the host, with the entry's own fixed arguments.
///
/// Every word of this script is a compile-time constant of this module — the
/// candidate paths and the entry's arguments — and each one is quoted for the
/// remote shell anyway. The operator's words selected the entry and reach the
/// host in nothing else, so barrier three of this module holds exactly as it
/// does on the [`host_channel::run_program`] path.
///
/// A host carrying none of the candidates gets the refusal named here rather
/// than a shell's `No such file or directory` against whichever path happened
/// to be listed first, because the second reads as "the fleet installed this
/// wrongly" when the truth is "this program is not on this machine".
fn candidate_script(candidates: &[&str], arguments: &[&str]) -> String {
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    let mut script = String::from("set -eu\n");
    for candidate in candidates {
        let path = shlex_quote(candidate);
        script.push_str(&format!("if [ -x {path} ]; then exec {path} {fixed}; fi\n"));
    }
    script.push_str(&format!(
        "printf '%s\\n' {} >&2\nexit 127\n",
        shlex_quote(&format!(
            "this program is installed at none of its approved paths on this host: {}",
            candidates.join(", ")
        ))
    ));
    script
}

/// Run one approved read-only command on a canonical registry host.
pub async fn exec_host(
    target_name: &str,
    words: &[String],
    runner: &Runner,
) -> Result<Value, DeployError> {
    // Refuse before resolving the target: an operator who typed something
    // outside the allowlist gets the allowlist back immediately, without a
    // registry round-trip and without the host ever being contacted.
    let approved = approve(words)?;
    let target = host_channel::canonical_target(target_name).await?;
    // A program that lives in exactly one place keeps the plain transport: its
    // fixed argv IS the command line, and probing a single path on the host
    // would only replace the remote shell's own report of a missing program
    // with a worse one.
    let candidates = approved.candidates();
    let output = match approved.argv.split_first() {
        Some((_, arguments)) if candidates.len() > usize::from(true) => {
            let script = candidate_script(candidates, arguments);
            host_channel::run_script(&target, &script, runner).await?
        }
        _ => host_channel::run_program(&target, approved.argv, runner).await?,
    };

    let mut report = host_channel::base_report(&target);
    report.insert("command".to_string(), json!(approved.display()));
    report.insert("argv".to_string(), json!(approved.argv));
    // Where this program may live, for an entry that has more than one install
    // path: `argv[0]` is one candidate among several and is not evidence of
    // where the host actually found it.
    if candidates.len() > usize::from(true) {
        report.insert("program_candidates".to_string(), json!(candidates));
    }
    report.insert("stdout".to_string(), json!(output.stdout));
    report.insert("stderr".to_string(), json!(output.stderr));
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}
