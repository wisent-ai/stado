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
pub fn serve<R: BufRead, W: Write>(reader: R, writer: &mut W) {
    for raw in reader.lines() {
        let Ok(raw) = raw else { break };
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(_) => {
                let _ = writeln!(
                    writer,
                    "{}",
                    frame(&error_response(
                        &Value::Null,
                        CODE_PARSE_ERROR,
                        "parse error"
                    ))
                );
                let _ = writer.flush();
                continue;
            }
        };
        if !request.is_object() {
            let _ = writeln!(
                writer,
                "{}",
                frame(&error_response(
                    &Value::Null,
                    CODE_PARSE_ERROR,
                    "request must be a JSON object"
                ))
            );
            let _ = writer.flush();
            continue;
        }
        if let Some(response) = handle(&request) {
            let _ = writeln!(writer, "{}", frame(&response));
            let _ = writer.flush();
        }
    }
}
