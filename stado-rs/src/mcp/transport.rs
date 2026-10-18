//! Transport: newline-delimited framing and the stdin serve loop.

use std::io::{BufRead, Write};

use serde_json::Value;

use super::dispatch::handle;
use super::protocol::{error_response, CODE_PARSE_ERROR};

/// Serialize one response frame. Python uses `json.dumps` defaults
/// (", " / ": " separators); keep them for byte parity.
fn frame(message: &Value) -> String {
    crate::queue::python_json_dumps(message).expect("JSON serialization is infallible")
}

/// Run the stdin loop until EOF (Python `serve`). Owns `writer`
/// exclusively (frames only); diagnostics go to stderr via `tracing`.
/// A line that cannot be read and a frame that cannot be written end the
/// loop with that error: read as EOF, a broken stdin would look like a client
/// that hung up, and an unwritten answer would leave the client waiting while
/// the server went on reading.
pub fn serve<R: BufRead, W: Write>(reader: R, writer: &mut W) -> std::io::Result<()> {
    for raw in reader.lines() {
        let raw = raw?;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(error) => {
                send(
                    writer,
                    &error_response(
                        &Value::Null,
                        CODE_PARSE_ERROR,
                        &format!("parse error: {error}"),
                    ),
                )?;
                continue;
            }
        };
        if !request.is_object() {
            send(
                writer,
                &error_response(
                    &Value::Null,
                    CODE_PARSE_ERROR,
                    "request must be a JSON object",
                ),
            )?;
            continue;
        }
        if let Some(response) = handle(&request) {
            send(writer, &response)?;
        }
    }
    Ok(())
}

/// Write one frame and flush it to the client.
fn send<W: Write>(writer: &mut W, message: &Value) -> std::io::Result<()> {
    writeln!(writer, "{}", frame(message))?;
    writer.flush()
}
