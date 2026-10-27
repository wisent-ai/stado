//! Finite authority commands share native SSH authentication, not a local process.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use russh::ChannelMsg;

use crate::deploy::host_access::native;

pub(crate) struct Output {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_status: Option<u32>,
}

/// The wait lines (`crate::wait`) the authority's command has opened and not
/// yet closed, by the id each carries, as they are relayed. A caller that
/// gives up on the command reads them to say what the authority itself stood
/// on at that moment, instead of only that it answered nothing.
#[derive(Clone, Default)]
pub(crate) struct OpenWaits(Arc<Mutex<BTreeMap<String, String>>>);

/// The prefix of a wait's opening line, and of the lines that end it.
const OPENED: &str = "czekam: ";
const ENDED: &[&str] = &["koniec czekania: ", "blad czekania: "];
/// The field every wait line ends with.
const ID_FIELD: &str = "; id: ";

impl OpenWaits {
    /// Keep `line` when it opens a wait, drop the wait it ends when it ends
    /// one; any other line says nothing about waits.
    fn note(&self, line: &str) -> Result<(), String> {
        let Some((_, id)) = line.rsplit_once(ID_FIELD) else {
            return Ok(());
        };
        // One insert or remove per line: a holder that panicked left the map
        // whole, so it is taken back rather than failing the command's run.
        let mut open = crate::primitives::lock::held(&self.0);
        if line.starts_with(OPENED) {
            open.insert(id.to_owned(), line.to_owned());
        } else if ENDED.iter().any(|prefix| line.starts_with(prefix)) {
            open.remove(id);
        }
        Ok(())
    }

    /// What the authority's command still waited on: its open wait lines, or
    /// that it had opened none.
    pub(crate) fn said(&self) -> String {
        let open = crate::primitives::lock::held(&self.0);
        if open.is_empty() {
            "the authority's command had no wait of its own open".to_string()
        } else {
            format!(
                "the authority's command still waited on: {}",
                open.values().cloned().collect::<Vec<_>>().join(" | ")
            )
        }
    }
}

/// Run one command over the authority's SSH channel, saying so at both ends
/// (`crate::wait`). The command ends when the channel does; a channel that
/// closes without an exit status is reported as such by the caller. Both
/// streams are kept whole: the 1 MiB ceiling once here would have refused
/// the registry once it grew past it. The command's own wait lines are kept
/// in `open` while it runs.
pub(crate) async fn execute(destination: &str, command: &str, open: &OpenWaits) -> Result<Output> {
    announced(destination, command, None, open).await
}

/// [`execute`] with `input` written to the command's standard input, which
/// is then closed: how a document reaches a command on the authority without
/// a file there.
pub(crate) async fn execute_with_input(
    destination: &str,
    command: &str,
    input: &[u8],
) -> Result<Output> {
    announced(destination, command, Some(input), &OpenWaits::default()).await
}

async fn announced(
    destination: &str,
    command: &str,
    input: Option<&[u8]>,
    open: &OpenWaits,
) -> Result<Output> {
    // A failed step names itself and its cause, the way the wait's failure
    // line and the caller's refusal read it.
    let failed = |step: &str, cause: String| std::io::Error::other(format!("{step}: {cause}"));
    // The authority is a host this process waits to reach: recorded in the
    // wait-for graph, so a wait whose recovery needs this process (an
    // install holding the lock the authority's own repair waits on) is
    // refused with the cycle named instead of standing.
    let authority = crate::wait::Resource::new(crate::wait::ResourceKind::Host, destination)
        .map_err(|cause| {
            anyhow::anyhow!("the authority host cannot be named in the wait-for graph: {cause}")
        })?;
    crate::wait::until_on(
        crate::wait::Kind::Host,
        command,
        format!("SSH to {destination}"),
        &authority,
        async move {
            let session = native::connect(destination)
                .await
                .map_err(|error| failed("connect to the authority", format!("{error:#}")))?;
            let mut channel = session
                .channel_open_session()
                .await
                .map_err(|error| failed("open authority command channel", error.to_string()))?;
            channel.exec(true, command).await.map_err(|error| {
                failed("request authority command execution", error.to_string())
            })?;
            if let Some(input) = input {
                channel.data(input).await.map_err(|error| {
                    failed(
                        "write the authority command's standard input",
                        error.to_string(),
                    )
                })?;
                channel.eof().await.map_err(|error| {
                    failed(
                        "close the authority command's standard input",
                        error.to_string(),
                    )
                })?;
            }
            let mut output = Output {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_status: None,
            };
            // The command's stderr not yet passed on to this process's
            // stderr. The authority's own wait lines say what it stands on
            // while the caller waits minutes; kept until the command failed,
            // they named nothing while it stood and nothing once it succeeded.
            let mut pending: Vec<u8> = Vec::new();
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => {
                        output.stderr.extend_from_slice(&data);
                        pending.extend_from_slice(&data);
                        let passed = relay_lines(destination, &pending, open)
                            .map_err(|cause| failed("keep the authority's wait lines", cause))?;
                        pending.drain(..passed);
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        output.exit_status = Some(exit_status)
                    }
                    ChannelMsg::Failure => {
                        return Err(failed(
                            "run the authority command",
                            "the SSH server refused it".to_string(),
                        ))
                    }
                    _ => {}
                }
            }
            let rest = String::from_utf8_lossy(&pending);
            if !rest.trim().is_empty() {
                eprintln!("{destination}: {}", rest.trim_end());
            }
            Ok(output)
        },
    )
    .await
    .map_err(|cycle| {
        anyhow::anyhow!("waiting on the authority {destination} would close a wait cycle: {cycle}")
    })?
    .map_err(anyhow::Error::from)
}

/// Write each whole line of `pending` to this process's stderr under the
/// destination's name, keep each wait it opens or ends in `open`, and answer
/// how many bytes those lines held; a line whose end has not arrived yet
/// waits for the next chunk.
fn relay_lines(destination: &str, pending: &[u8], open: &OpenWaits) -> Result<usize, String> {
    pending
        .split_inclusive(|byte| *byte == b'\n')
        .take_while(|line| line.ends_with(b"\n"))
        .map(|line| {
            let text = String::from_utf8_lossy(line);
            let text = text.trim_end();
            eprintln!("{destination}: {text}");
            open.note(text).map(|()| line.len())
        })
        .sum()
}
