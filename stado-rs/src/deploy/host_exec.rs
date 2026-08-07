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
use super::{py_str_repr, DeployError, Runner};

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
}

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
    let output = host_channel::run_program(&target, approved.argv, runner).await?;

    let mut report = host_channel::base_report(&target);
    report.insert("command".to_string(), json!(approved.display()));
    report.insert("argv".to_string(), json!(approved.argv));
    report.insert("stdout".to_string(), json!(output.stdout));
    report.insert("stderr".to_string(), json!(output.stderr));
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    #[test]
    fn process_limit_and_owner_entries_are_approved_exactly() {
        let sysctl = approve(&words("sysctl -n kern.maxproc kern.maxprocperuid")).unwrap();
        assert_eq!(
            sysctl.argv,
            &[
                "/usr/sbin/sysctl",
                "-n",
                "kern.maxproc",
                "kern.maxprocperuid"
            ]
        );
        let owners = approve(&words("ps ax -o user -o pid -o comm")).unwrap();
        assert_eq!(
            owners.argv,
            &["/bin/ps", "ax", "-o", "user", "-o", "pid", "-o", "comm"]
        );
    }

    /// The entries match as whole spellings, not as prefixes and not with
    /// extra words appended. `sysctl -n kern.maxproc` alone is a DIFFERENT
    /// command from the approved one, and it is refused rather than run as
    /// the nearest entry.
    #[test]
    fn neither_entry_matches_a_prefix_or_takes_extra_words() {
        for spelling in [
            "sysctl -n kern.maxproc",
            "sysctl -n kern.maxproc kern.maxprocperuid kern.hostname",
            "sysctl",
            "ps ax -o user -o pid",
            "ps ax -o user -o pid -o comm -o etime",
        ] {
            let error = approve(&words(spelling)).unwrap_err();
            assert!(
                error.0.contains("is not an approved host-exec command"),
                "{spelling} was not refused: {}",
                error.0
            );
        }
    }

    /// `-o command` prints the full argv of every process, which is where
    /// tokens and passwords are passed. It is not in the table, and the
    /// exact-match rule means the `-o comm` entry cannot be talked into
    /// serving it — not by substitution, not by appending.
    #[test]
    fn the_full_argv_spelling_is_refused() {
        for spelling in [
            "ps ax -o user -o pid -o command",
            "ps ax -o user -o pid -o comm -o command",
            "ps ax -o command",
        ] {
            let error = approve(&words(spelling)).unwrap_err();
            assert!(
                error.0.contains("is not an approved host-exec command"),
                "{spelling} was not refused: {}",
                error.0
            );
        }
        assert!(
            !APPROVED_COMMANDS
                .iter()
                .any(|entry| entry.argv.contains(&"command")),
            "no approved entry may ask ps for the full command line"
        );
    }
}
