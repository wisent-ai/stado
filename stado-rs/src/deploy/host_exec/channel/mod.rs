//! Which transport carries an approved entry to the host: a program the
//! managed account owns, a fixed path inside that account's home, or a
//! program this fleet installs at more than one absolute path.

mod candidate;
mod home_rooted;

use crate::deploy::shlex_quote;

use super::allowlist::{KIMI_CLI, SKARBIEC_CLI, STADO_CLI};

pub use candidate::candidate_script;
pub use home_rooted::{home_rooted_script, probierz_run_root_script};

/// Marks a program as living under the managed account's home.
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
}

/// Every program in the table that the managed account owns.
pub const ACCOUNT_PROGRAMS: &[AccountProgram] = &[
    AccountProgram {
        program: STADO_CLI,
        candidates: &[STADO_CLI],
    },
    AccountProgram {
        program: KIMI_CLI,
        // Match the installed-client locations probed by Weles.
        candidates: &["~/.local/bin/kimi", KIMI_CLI, "/opt/homebrew/bin/kimi"],
    },
    AccountProgram {
        program: SKARBIEC_CLI,
        candidates: &[SKARBIEC_CLI],
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

/// Resolve an account-owned program and execute its fixed approved arguments.
///
/// Every word is a compile-time constant of this module and is quoted for the
/// remote shell; the only thing that expands on the host is its own `$HOME`.
/// The operator's words selected the entry and reach the host in nothing else,
/// so barrier three holds exactly as it does on the
/// [`crate::deploy::host_channel::run_program`] path.
pub fn account_script(account: &AccountProgram, arguments: &[&str]) -> String {
    let mut script = String::from("set -eu\nprogram=\n");
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
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    script.push_str(&format!("exec \"$program\" {fixed}\n"));
    script
}
