//! What the host channel of an attached run says on stderr, as this process
//! reads it: the bytes go on as they arrive, and the wrapper's lines mark the
//! stages of the run, each a wait of this process's own, so `stado waits` and
//! the `czekam` lines name the stage a run stands in: the program running,
//! the read of who still holds its output, and the channel closing after the
//! program exited.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::wait::{Kind, Waiting};

use super::holders::{HOLDERS_READ, HOLDERS_READING};

/// The words before the program's own exit status, on the line the wrapper
/// writes the moment the program ended.
pub(super) const PROGRAM_EXITED: &str = "stado host run attach: program exited";
/// The words before the wrapper's own status, on the line it writes as it
/// exits.
pub(super) const WRAPPER_EXITING: &str = "stado host run attach: wrapper exiting";
/// What the wait on the running program is.
const PROGRAM_WAIT: &str = "the attached remote program to exit";
/// What the wait between the program's exit and the channel's end is.
const CLOSING_WAIT: &str = "the host channel of an attached program to close after it exited";

/// What the channel's stderr carried, once it ended.
#[derive(Default)]
pub(super) struct Relayed {
    /// Every byte but the wrapper's two status lines, when kept.
    pub kept: Vec<u8>,
    pub program_exit: Option<i32>,
    pub wrapper_exit: Option<i32>,
}

/// The waits of one run's stages, on `target`.
pub(super) struct Stages {
    target: String,
    program: Option<Waiting>,
    closing: Option<Waiting>,
}

impl Stages {
    pub fn begin(target: &str) -> Self {
        Self {
            target: target.to_string(),
            program: Some(crate::wait::begin(Kind::Host, PROGRAM_WAIT, target)),
            closing: None,
        }
    }

    /// The program reported its exit: its wait is over, and the channel's
    /// closing is waited on.
    pub fn program_exited(&mut self) {
        if let Some(waiting) = self.program.take() {
            waiting.done();
        }
        if self.closing.is_none() {
            self.closing = Some(crate::wait::begin(Kind::Host, CLOSING_WAIT, &self.target));
        }
    }

    /// The channel ended `how`, and its stderr said `program_exit`.
    pub fn ended(mut self, program_exit: Option<i32>, how: &str) {
        if program_exit.is_some() {
            self.program_exited();
        } else if let Some(waiting) = self.program.take() {
            waiting.failed(format!(
                "the host channel to {} ended ({how}) before the program reported its exit",
                self.target
            ));
        }
        if let Some(waiting) = self.closing.take() {
            waiting.done();
        }
    }
}

/// The channel's stderr, chunk by chunk: kept when `keep`, else written on to
/// this process's stderr as each chunk arrives, a partial line included.
/// Complete lines are read for the wrapper's: its two status lines are
/// recorded and left out of what is kept, `exited` is told the program's
/// status the moment it arrives, and the holder read's lines open the wait
/// `what` at `place` and end it; a channel that ends while the read runs
/// ends that wait as cut short.
pub(super) async fn relay(
    stderr: tokio::process::ChildStderr,
    keep: bool,
    what: String,
    place: String,
    exited: tokio::sync::oneshot::Sender<i32>,
) -> std::io::Result<Relayed> {
    let mut source = tokio::io::BufReader::new(stderr);
    let mut through = tokio::io::stderr();
    let mut reader = Reader {
        keep,
        what,
        place,
        exited: Some(exited),
        reading: None,
        relayed: Relayed::default(),
    };
    let mut line = Vec::new();
    loop {
        let chunk = source.fill_buf().await?;
        if chunk.is_empty() {
            if !line.is_empty() {
                reader.line(&line);
            }
            return Ok(reader.relayed);
        }
        if !keep {
            through.write_all(chunk).await?;
            through.flush().await?;
        }
        for piece in chunk.split_inclusive(|byte| *byte == b'\n') {
            line.extend_from_slice(piece);
            if piece.ends_with(b"\n") {
                reader.line(&line);
                line.clear();
            }
        }
        let read = chunk.len();
        source.consume(read);
    }
}

struct Reader {
    keep: bool,
    what: String,
    place: String,
    exited: Option<tokio::sync::oneshot::Sender<i32>>,
    reading: Option<Waiting>,
    relayed: Relayed,
}

impl Reader {
    fn line(&mut self, line: &[u8]) {
        if let Some((before, status)) = reported(line, PROGRAM_EXITED) {
            self.relayed.program_exit = Some(status);
            if let Some(exited) = self.exited.take() {
                // The run's own loop may have ended already; the status is
                // kept in what this relay answers either way.
                let _ = exited.send(status);
            }
            self.keep_part(before);
            return;
        }
        if let Some((before, status)) = reported(line, WRAPPER_EXITING) {
            self.relayed.wrapper_exit = Some(status);
            self.keep_part(before);
            return;
        }
        if self.keep {
            self.relayed.kept.extend_from_slice(line);
        }
        let said = String::from_utf8_lossy(line);
        if said.contains(HOLDERS_READING) {
            self.reading = Some(crate::wait::begin(Kind::Host, &self.what, &self.place));
        } else if said.contains(HOLDERS_READ) {
            if let Some(waiting) = self.reading.take() {
                waiting.done();
            }
        }
    }

    /// The program's bytes a status line followed with no newline between.
    fn keep_part(&mut self, before: &[u8]) {
        if self.keep && !before.is_empty() {
            self.relayed.kept.extend_from_slice(before);
            self.relayed.kept.push(b'\n');
        }
    }
}

/// The bytes before `marker` in `line` and the status after it, when `line`
/// carries the marker and a status that reads as one.
fn reported<'a>(line: &'a [u8], marker: &str) -> Option<(&'a [u8], i32)> {
    let marker = marker.as_bytes();
    let at = line
        .windows(marker.len())
        .position(|window| window == marker)?;
    let (before, rest) = line.split_at(at);
    let status = std::str::from_utf8(rest.get(marker.len()..)?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some((before, status))
}
