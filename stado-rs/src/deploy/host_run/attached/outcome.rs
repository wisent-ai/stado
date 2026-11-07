//! What an attached run answers, and how its end is judged: by the program's
//! own status when the wrapper reported it, and otherwise by who ended the
//! run before it could, the host's wrapper refusing or the channel failing.
//! The channel's own status alone told a failed ssh (255) and a program that
//! refused apart from nothing.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AttachedOutcome {
    pub target: String,
    pub program: String,
    pub arguments: Vec<String>,
    /// `exited` when the program reported status 0, else `failed`.
    pub status: &'static str,
    /// The program's own exit status, as the wrapper reported it the moment
    /// the program ended; `None` when the run ended before that report, so
    /// whether the program ran is not known.
    pub program_exit: Option<i32>,
    /// The status the wrapper reported as it exited; `None` when the run
    /// ended before it could say it.
    pub wrapper_exit: Option<i32>,
    /// The channel's exit code; `None` when a signal ended it, which
    /// `ended` names, so no code is invented for a run that returned none.
    pub exit_code: Option<i32>,
    /// How the channel ended, as the operating system states it.
    pub ended: String,
    pub forwarded_signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

/// Who ended a run that did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The program ran and exited with a status other than 0: its own answer.
    Program,
    /// The host's wrapper exited before the program's status was reported:
    /// it refused to start or watch the program, in its own words.
    Host,
    /// The channel ended before the program's status was reported.
    Channel,
}

/// Why a run did not succeed, said after the program's name and host.
#[derive(Debug, Clone)]
pub struct Failure {
    pub ending: Ending,
    pub said: String,
}

/// ssh(1): "ssh exits with the exit status of the remote command or with 255
/// if an error occurred"; 255 is the largest status a process can exit with.
fn ssh_failed() -> i32 {
    i32::from(u8::MAX)
}

impl AttachedOutcome {
    /// `Ok` when the program reported status 0, whatever the channel did
    /// after (which [`Self::channel_after`] says); otherwise who ended the
    /// run, with the program's lines when they were kept.
    pub fn judged(&self) -> Result<(), Failure> {
        let said = self.said();
        let quoted = |text: String| {
            if said.is_empty() {
                text
            } else {
                format!("{text}: {said}")
            }
        };
        let (ending, text) = match self.program_exit {
            Some(code) if code == nix::libc::EXIT_SUCCESS => return Ok(()),
            Some(code) => (
                Ending::Program,
                match self.channel_after() {
                    Some(after) => format!("{}; {after}", quoted(format!("exited {code}"))),
                    None => quoted(format!("exited {code}")),
                },
            ),
            None => match self
                .wrapper_exit
                .or(self.exit_code.filter(|code| *code != ssh_failed()))
            {
                Some(code) => (
                    Ending::Host,
                    quoted(format!(
                        "did not report its exit: the host's wrapper exited {code} first, \
                         refusing to start or watch it"
                    )),
                ),
                None => (
                    Ending::Channel,
                    quoted(format!(
                        "did not report its exit: the host channel to {} ended ({}) first, so \
                         whether it ran is not known; read its effect on {} before running it \
                         again",
                        self.target, self.ended, self.target
                    )),
                ),
            },
        };
        Err(Failure { ending, said: text })
    }

    /// How the channel ended after the program reported its status, when
    /// not with the wrapper's status as it should: a channel cut or broken
    /// after the program's answer was already in.
    pub fn channel_after(&self) -> Option<String> {
        let program = self.program_exit?;
        let reported = self.wrapper_exit.unwrap_or(program);
        (self.exit_code != Some(reported)).then(|| {
            format!(
                "the host channel to {} then ended ({}), not with the wrapper's status {reported}",
                self.target, self.ended
            )
        })
    }

    /// The run's non-empty lines, stdout first, when they were kept.
    fn said(&self) -> String {
        let lines: Vec<&str> = [self.stdout.as_deref(), self.stderr.as_deref()]
            .into_iter()
            .flatten()
            .flat_map(str::lines)
            .filter(|line| !line.trim().is_empty())
            .collect();
        lines.join(" | ")
    }
}
