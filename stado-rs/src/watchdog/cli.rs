//! The argparse-compatible command line: the parsed argument shape, the
//! usage/help text, argparse's long-option and `int()` semantics, and the
//! entry point `src/bin/stado_watchdog.rs` calls.

use std::path::Path;
use std::time::Duration;

use super::upload::once;
use super::{DEFAULT_BUCKET, DEFAULT_INTERVAL_S};
use crate::models::py_str_repr;

// ---------------------------------------------------------------------------
// argparse-compatible CLI
// ---------------------------------------------------------------------------

/// Parsed watchdog arguments (Python `argparse.Namespace` equivalent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub bucket: String,
    pub interval_s: i64,
    pub once: bool,
}

/// Non-Ok parse results, mapped to argparse's exit behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// `-h`/`--help`: print [`help_text`] on stdout, exit 0.
    Help,
    /// Usage error: print usage + `{prog}: error: {msg}` on stderr, exit 2.
    Error(String),
}

/// argparse's two-line usage string for this parser (it wraps the option
/// list to 80 columns; the break lands before `[--once]`, continuation
/// aligned past `usage: {prog} `).
pub fn usage_text(prog: &str) -> String {
    let indent = " ".repeat("usage: ".len() + prog.len() + 1);
    format!("usage: {prog} [-h] [--bucket BUCKET] [--interval-s INTERVAL_S]\n{indent}[--once]")
}

/// argparse `--help` output (without the trailing newline `print` adds).
pub fn help_text(prog: &str) -> String {
    format!(
        "{}\n\nUpload workstation diagnostics to GCS.\n\noptions:\n  -h, --help            show this help message and exit\n  --bucket BUCKET\n  --interval-s INTERVAL_S\n  --once",
        usage_text(prog)
    )
}

const LONG_OPTIONS: [&str; 4] = ["--help", "--bucket", "--interval-s", "--once"];

/// The process exit code argparse uses for an argument error, named once
/// here so the usage-error arm of [`cli_main`] carries no bare number.
const USAGE_ERROR_EXIT: i32 = 2;

/// Python `int(str)` for the `--interval-s` argument: surrounding
/// whitespace tolerated, optional sign, `_` digit separators allowed
/// between digits.
fn parse_python_int(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let cleaned: String = trimmed.chars().filter(|&c| c != '_').collect();
    cleaned.parse::<i64>().ok()
}

/// argparse-style long-option resolution with unambiguous prefix
/// abbreviation (`--buck` -> `--bucket`).
fn resolve_long(name: &str) -> Option<&'static str> {
    let matches: Vec<&&str> = LONG_OPTIONS
        .iter()
        .filter(|opt| opt.starts_with(name))
        .collect();
    match matches.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// argparse `parse_args` for the watchdog parser. Byte-reproduces
/// argparse's error strings; exit behavior is described by
/// [`ParseOutcome`].
pub fn parse_args(_prog: &str, args: &[String]) -> Result<ParsedArgs, ParseOutcome> {
    let bucket_env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::Gcs.id(),
        "bucket",
    )
    .expect("GCS bucket binding is missing from the capability catalog");
    let default_bucket = std::env::var(bucket_env).unwrap_or_else(|_| DEFAULT_BUCKET.to_string());
    let mut parsed = ParsedArgs {
        bucket: default_bucket,
        interval_s: DEFAULT_INTERVAL_S,
        once: false,
    };
    let mut extras: Vec<String> = Vec::new();
    let mut positional_only = false;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        if positional_only {
            extras.push(arg.clone());
            continue;
        }
        if arg == "--" {
            positional_only = true;
            continue;
        }
        if arg == "-h" {
            return Err(ParseOutcome::Help);
        }
        if let Some(long) = arg.strip_prefix("--") {
            if long.is_empty() {
                extras.push(arg.clone());
                continue;
            }
            let (name, inline_value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let dashed = format!("--{name}");
            let Some(option) = resolve_long(&dashed) else {
                extras.push(arg.clone());
                continue;
            };
            match option {
                "--help" => return Err(ParseOutcome::Help),
                "--once" => parsed.once = true,
                "--bucket" | "--interval-s" => {
                    let value = match inline_value {
                        Some(value) => value,
                        None => {
                            let Some(next) = args.get(index) else {
                                return Err(ParseOutcome::Error(format!(
                                    "argument {option}: expected one argument"
                                )));
                            };
                            index += 1;
                            next.clone()
                        }
                    };
                    if option == "--bucket" {
                        parsed.bucket = value;
                    } else {
                        match parse_python_int(&value) {
                            Some(interval) => parsed.interval_s = interval,
                            None => {
                                return Err(ParseOutcome::Error(format!(
                                    "argument --interval-s: invalid int value: {}",
                                    py_str_repr(&value)
                                )));
                            }
                        }
                    }
                }
                _ => unreachable!("LONG_OPTIONS is exhaustive"),
            }
            continue;
        }
        extras.push(arg.clone());
    }
    if !extras.is_empty() {
        return Err(ParseOutcome::Error(format!(
            "unrecognized arguments: {}",
            extras.join(" ")
        )));
    }
    Ok(parsed)
}

/// The argparse CLI entry (used by `src/bin/stado_watchdog.rs`).
pub async fn cli_main() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let prog = argv
        .first()
        .and_then(|arg0| Path::new(arg0).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "stado-watchdog".to_string());
    let parsed = match parse_args(&prog, &argv[1..]) {
        Ok(parsed) => parsed,
        Err(ParseOutcome::Help) => {
            println!("{}", help_text(&prog));
            return 0;
        }
        Err(ParseOutcome::Error(message)) => {
            eprintln!("{}", usage_text(&prog));
            eprintln!("{prog}: error: {message}");
            return USAGE_ERROR_EXIT;
        }
    };
    if parsed.once {
        return once(&parsed.bucket).await;
    }
    loop {
        once(&parsed.bucket).await;
        let interval = parsed.interval_s.max(10) as u64;
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}
