//! Which transport carries an approved entry to the host: a program the
//! managed account owns, a fixed path inside that account's home, or a
//! program this fleet installs at more than one absolute path.

mod candidate;
mod home_rooted;

use crate::deploy::shlex_quote;

use super::allowlist::{BRAMA_LAUNCHER, KIMI_CLI, STADO_CLI};

pub use candidate::candidate_script;
pub use home_rooted::{home_rooted_script, probierz_run_root_script};

/// The prefix that marks a program, or one of its environment values, as
/// living under the login user's home rather than at a system path.
const HOME_RELATIVE: &str = "~/";

/// What an entry whose program the managed account owns needs on top of its
/// fixed argv.
#[derive(Debug)]
pub struct AccountProgram {
    /// `argv[0]` of every entry this describes, exactly as the entry spells
    /// it. Keyed on the program, like
    /// [`super::allowlist::PROGRAM_CANDIDATES`], so one row covers every verb
    /// of the same program.
    program: &'static str,
    /// Every path in the account's home this program is installed at, in probe
    /// order. The first executable one runs.
    pub candidates: &'static [&'static str],
    /// The released product this program belongs to, and the program's path
    /// inside that release, when the host's own release agent records where
    /// the installed bundle is. Resolved before [`Self::candidates`], which
    /// then only covers a host with no recorded rollout.
    released_program: Option<(&'static str, &'static str)>,
    /// Fixed environment for that program, home-relative where a value is a
    /// path. Compile-time constants of this module: an operator's words select
    /// an entry and never become part of this.
    environment: &'static [(&'static str, &'static str)],
    /// The wall-clock budget for the whole run.
    ///
    /// [`crate::deploy::host_channel::remote_timeout`] is two minutes, which
    /// is right for a read and wrong for a repair that walks a real
    /// single-sign-on and a consent screen in a browser on the far side.
    /// Cutting the channel mid-flight would leave the operator unable to tell
    /// a refused sign-in from one still running.
    pub timeout_seconds: u64,
}

/// Where the release agent records each product's rollout, home-relative.
const RELEASE_STATE_DIRECTORY: &str = ".stado/release-state";
/// Prints `<active.release_dir>/<argv[2]>` for the state document in
/// `argv[1]`, or nothing when the document names no active release. Fixed
/// source: the two arguments are compile-time constants of this module.
const RELEASE_DIRECTORY_READER: &str = "import json,sys\n\
    state=json.load(open(sys.argv[1]))\n\
    active=state.get('active') or {}\n\
    directory=active.get('release_dir') or ''\n\
    print(f\"{directory}/{sys.argv[2]}\" if directory else '')\n";

/// Every program in the table that the managed account owns.
pub const ACCOUNT_PROGRAMS: &[AccountProgram] = &[
    AccountProgram {
        program: STADO_CLI,
        candidates: &[STADO_CLI],
        environment: &[],
        timeout_seconds: 180,
        released_program: None,
    },
    AccountProgram {
        program: BRAMA_LAUNCHER,
        // The launcher is part of the release bundle, and the bundle that
        // matters is the one the release agent installed - read from its own
        // state above. These remain as the fallback for a host whose agent has
        // recorded no active release: both platform directory spellings the
        // fleet has shipped, then the standalone copy some accounts keep in
        // `~/.stado/bin`.
        released_program: Some(("brama", "bin/start-with-skarbiec")),
        candidates: &[
            "~/.stado/services/brama/current/darwin-arm64/bin/start-with-skarbiec",
            "~/.stado/services/brama/current/darwin-arm/bin/start-with-skarbiec",
            BRAMA_LAUNCHER,
        ],
        // Two paths this run must not share with the gateway it is repairing.
        //
        // The launcher ends whatever holds its capability-broker socket and then
        // rebinds it. On its stable default path that is the live gateway's own
        // broker, so a CLI run beside a serving Brama would take the service's
        // credential redemption down with it. And the runtime directory is named
        // after the installation, which for a CLI run out of the live bundle is
        // the live one: the launcher rebuilds the subscription manifest and the
        // capability catalog in it at every start, so sharing it would rewrite the
        // serving gateway's own runtime state. Both get a copy of their own under
        // the fleet's scratch area, and the service is untouched.
        environment: &[
            (
                "BRAMA_CAP_SOCKET",
                "~/.stado/work/brama-sign-in/capability.sock",
            ),
            ("BRAMA_RUNTIME_DIR", "~/.stado/work/brama-sign-in/runtime"),
        ],
        timeout_seconds: 1500,
    },
    AccountProgram {
        program: KIMI_CLI,
        // The three places this fleet's hosts have it, in the order the Weles
        // trajectory's own resolver probes them, so `host exec` and the
        // trajectory cannot disagree about which binary is the Kimi CLI.
        candidates: &["~/.local/bin/kimi", KIMI_CLI, "/opt/homebrew/bin/kimi"],
        // The Kimi CLI is not a Stado-released product; nothing records an
        // installed bundle for it.
        released_program: None,
        // Nothing. Its help is a read; giving it an environment would be
        // giving it a home and a session it has no business reading here.
        environment: &[],
        timeout_seconds: 60,
    },
];

/// The account-owned program behind an entry, if this is one.
pub fn account_program(program: &str) -> Option<&'static AccountProgram> {
    ACCOUNT_PROGRAMS
        .iter()
        .find(|account| account.program == program)
}

/// A home-relative word as the remote shell should read it: its own `$HOME`
/// followed by the quoted remainder. A word that is already absolute is just
/// quoted.
fn home_anchored(word: &str) -> String {
    match word.strip_prefix(HOME_RELATIVE) {
        Some(rest) => format!("\"$HOME\"/{}", shlex_quote(rest)),
        None => shlex_quote(word),
    }
}

/// The remote script for an account-owned entry: find the installed copy,
/// export the entry's fixed environment, then become it.
///
/// Every word is a compile-time constant of this module and is quoted for the
/// remote shell; the only thing that expands on the host is its own `$HOME`.
/// The operator's words selected the entry and reach the host in nothing else,
/// so barrier three holds exactly as it does on the
/// [`crate::deploy::host_channel::run_program`] path.
pub fn account_script(account: &AccountProgram, arguments: &[&str]) -> String {
    let mut script = String::from("set -eu\nprogram=\n");
    if let Some((product, relative)) = account.released_program {
        // The launcher the repair must run is the one the release agent
        // actually installed, and the agent records where that is: the
        // `active.release_dir` of `$HOME/.stado/release-state/<product>.json`.
        // The static candidates below name a `current/<platform>/bin` layout
        // the agent never writes, so on 2026-09-19 every `subscription
        // sign-in` on charless-mac-mini fell through to the standalone copy in
        // `~/.stado/bin` - a bundle from 8 September, whose sign-in refuses
        // every Weles release with "does not advertise the login_item
        // selector", a gate the served release removed on 9 September.
        script.push_str(&format!(
            "state=\"$HOME\"/{state}\n\
             if [ -r \"$state\" ]; then\n\
             \x20 released=$(/usr/bin/python3 -S -c {reader} \"$state\" {relative} || true)\n\
             \x20 [ -z \"$released\" ] || [ ! -x \"$released\" ] || program=\"$released\"\n\
             fi\n",
            state = shlex_quote(&format!("{RELEASE_STATE_DIRECTORY}/{product}.json")),
            reader = shlex_quote(RELEASE_DIRECTORY_READER),
            relative = shlex_quote(relative),
        ));
    }
    for candidate in account.candidates {
        script.push_str(&format!(
            "[ -n \"$program\" ] || [ ! -x {candidate} ] || program={candidate}\n",
            candidate = home_anchored(candidate)
        ));
    }
    script.push_str(&format!(
        "[ -n \"$program\" ] || {{ printf '%s\\n' {} >&2; exit 127; }}\n",
        shlex_quote(&format!(
            "this program is installed at none of its approved paths in the managed \
             account's home on this host: {}",
            account.candidates.join(", ")
        ))
    ));
    for (name, value) in account.environment {
        script.push_str(&format!("{name}={}\nexport {name}\n", home_anchored(value)));
    }
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    script.push_str(&format!("exec \"$program\" {fixed}\n"));
    script
}
