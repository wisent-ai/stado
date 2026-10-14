//! One page-loop over the log bytes past the caller's cursor.

use std::io::Write;

use crate::cli::CmdError;
use crate::machine::MachineFacade;

use super::super::cmd_error;

/// Print (or buffer) every byte past `cursor` and advance it, returning
/// once the page reports EOF. [`MachineFacade::read_logs`] slices an
/// already-downloaded buffer, so each read asks for everything to the end.
///
/// The cursor is never reset between polls, which is the point: each read
/// asks for `[cursor, end)` and the stream stays monotone instead of
/// replaying the log every tick.
pub(super) async fn drain(
    facade: &MachineFacade,
    job_id: &str,
    cursor: &mut i64,
    buffered: &mut String,
    buffer: bool,
) -> Result<(), CmdError> {
    loop {
        let page = match facade.read_logs(job_id, *cursor, None).await {
            Ok(page) => page,
            // An agent that restarts the command re-uploads the log from
            // the beginning, so a cursor that was valid a poll ago can end
            // up past the new end. Rewind and replay rather than dying
            // mid-tail. Guarded on a non-zero cursor: read_logs cannot
            // reject offset zero, so this can never spin.
            Err(exc) if exc.code == "INVALID_CURSOR" && *cursor != 0 => {
                *cursor = 0;
                eprintln!("-- log restarted from the beginning; rewinding --");
                continue;
            }
            Err(exc) => return Err(cmd_error(exc)),
        };
        // A page without its text, next cursor or end flag cannot be
        // followed: read as "", the same cursor and "ended", it stopped the
        // tail as if the log were complete.
        let (Some(text), Some(next_cursor), Some(eof)) = (
            page["text"].as_str(),
            page["next_cursor"].as_i64(),
            page["eof"].as_bool(),
        ) else {
            return Err(CmdError::click(format!(
                "{job_id}: the log page at cursor {} names no text, next_cursor or eof: {page}",
                *cursor
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        };
        if buffer {
            buffered.push_str(text);
        } else {
            print!("{text}");
            // A tail that only flushes on newline stalls on a progress bar.
            let _ = std::io::stdout().flush();
        }
        *cursor = next_cursor;
        if eof {
            return Ok(());
        }
    }
}
