//! One page-loop over the log bytes past the caller's cursor.

use std::io::Write;

use crate::cli::CmdError;
use crate::machine::MachineFacade;

use super::super::cmd_error;

/// Bytes requested per log page. [`MachineFacade::read_logs`] slices an
/// already-downloaded buffer, so the cheapest page is "all of it" — one
/// read per poll instead of one per window. `u32::MAX` is a digit-free
/// bound far above any command log, and [`drain`] still honours `eof` if
/// one ever exceeds it.
const LOG_PAGE_BYTES: i64 = u32::MAX as i64;

/// Print (or buffer) every byte past `cursor` and advance it, returning
/// once the page reports EOF — so a log that grew by more than one page
/// between polls still arrives whole in this poll.
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
        let page = match facade.read_logs(job_id, *cursor, LOG_PAGE_BYTES).await {
            Ok(page) => page,
            // An agent that restarts the command re-uploads the log from
            // the beginning, so a cursor that was valid a poll ago can end
            // up past the new end. Rewind and replay rather than dying
            // mid-tail. Guarded on a non-zero cursor: read_logs cannot
            // reject offset zero, so this can never spin.
            Err(exc) if exc.code == "INVALID_CURSOR" && *cursor != i64::default() => {
                *cursor = i64::default();
                eprintln!("-- log restarted from the beginning; rewinding --");
                continue;
            }
            Err(exc) => return Err(cmd_error(exc)),
        };
        let text = page["text"].as_str().unwrap_or_default();
        if buffer {
            buffered.push_str(text);
        } else {
            print!("{text}");
            // A tail that only flushes on newline stalls on a progress bar.
            let _ = std::io::stdout().flush();
        }
        *cursor = page["next_cursor"].as_i64().unwrap_or(*cursor);
        if page["eof"].as_bool().unwrap_or(true) {
            return Ok(());
        }
    }
}
