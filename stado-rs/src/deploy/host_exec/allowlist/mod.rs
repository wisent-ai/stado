//! The allowlist: the shape of one approved command, the barriers an
//! operator's words pass, and the table those words are matched against.

mod arguments;
mod candidates;
mod entries;
mod programs;

use crate::deploy::py_str_repr;

use super::refusal::ExecRefusal;
use arguments::{LINUX_TAILSCALE_LOG_READ, MACOS_TAILSCALE_LOG_READ};

pub use arguments::{home_rooted, PROBIERZ_RUN_ROOT_CREATE};
pub use candidates::{cargo_candidates, program_candidates, PROGRAM_CANDIDATES};
pub use entries::APPROVED_COMMANDS;
pub use programs::{
    ADB_PROGRAM, APPIUM_PROGRAM, BRAMA_LAUNCHER, GIT_PROGRAM, KIMI_CLI, NODE_PROGRAM, STADO_CLI,
    TMUX_PROGRAM,
};

/// The punctuation an operator's word may contain on top of ASCII
/// alphanumerics. Every one of these is inert to `/bin/sh`: no expansion,
/// no word splitting, no redirection, no globbing.
const SAFE_PUNCTUATION: &str = "-_./:%+";

/// One approved remote program.
#[derive(Clone, Copy, Debug)]
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

/// Every approved spelling, comma-separated, for help and error text.
pub fn allowlist() -> String {
    APPROVED_COMMANDS
        .iter()
        .map(ApprovedCommand::display)
        .collect::<Vec<String>>()
        .join(", ")
}

/// These exact retained-log reads need no mutation confirmation in Desktop.
/// Other host-exec operations, including provider sign-in, keep their existing
/// confirmation requirement.
pub(crate) fn is_retained_log_read(words: &[String]) -> bool {
    approve(words).is_ok_and(|entry| {
        entry.argv == MACOS_TAILSCALE_LOG_READ || entry.argv == LINUX_TAILSCALE_LOG_READ
    })
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
/// Every refusal here is stated, not guessed: it is the one place that knows
/// the words matched nothing, so it says so with its own code (see
/// [`ExecRefusal`]). The approved spellings still reach the operator — an
/// operator who guessed wrong should not have to go read the source — but
/// they ride [`ExecRefusal::help`] rather than the sentence, because a
/// refusal that quotes its own help text is a refusal that can be
/// misclassified by it.
pub fn approve(words: &[String]) -> Result<&'static ApprovedCommand, ExecRefusal> {
    if words.is_empty() {
        return Err(ExecRefusal::unapproved("no command given".to_string()));
    }
    for word in words {
        if !is_shell_safe(word) {
            return Err(ExecRefusal::unapproved(format!(
                "argument {} contains a character a shell would interpret; \
                 host exec is an allowlist, not a shell",
                py_str_repr(word),
            )));
        }
    }
    let requested = words.join(" ");
    APPROVED_COMMANDS
        .iter()
        .find(|candidate| candidate.display() == requested)
        .ok_or_else(|| {
            ExecRefusal::unapproved(format!(
                "{} is not an approved host-exec command",
                py_str_repr(&requested),
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry must be reachable by the spelling it advertises.
    ///
    /// This is the trap the home-rooted reads were written around: an entry
    /// carrying a `~/…` argument advertises a spelling barrier one refuses,
    /// so it would sit in the table forever, listed in every refusal, and
    /// never run.
    #[test]
    fn every_advertised_spelling_selects_its_own_entry() {
        for entry in APPROVED_COMMANDS {
            let words: Vec<String> = entry.display().split(' ').map(str::to_string).collect();
            for word in &words {
                assert!(
                    is_shell_safe(word),
                    "{}: the word {word:?} an operator must type is refused by barrier one",
                    entry.display()
                );
            }
            let selected = approve(&words).expect("its own spelling selects it");
            assert_eq!(selected.argv, entry.argv, "{}", entry.display());
        }
    }

    /// A path an operator supplies is a path that can be a private key.
    #[test]
    fn no_entry_can_be_pointed_at_a_home_dotfile() {
        for entry in APPROVED_COMMANDS {
            for word in entry.argv {
                assert!(
                    !word.contains(".ssh"),
                    "{}: reads inside .ssh are not approvable",
                    entry.display()
                );
            }
        }
        assert!(approve(&["cat".into(), ".ssh/id_ed25519".into()]).is_err());
        assert!(approve(&["readlink".into(), ".stado/services/brama/current".into()]).is_err());
    }
}
