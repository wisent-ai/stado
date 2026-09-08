//! `stado release logs` — the candidate's own account of why it stopped,
//! fetched from the exact file the release agent writes it to.

use clap::{Args, ValueEnum};
use serde_json::{json, Value};

use crate::cli::release_quarantine::{
    canonical_control, compute_target, remote_read_head, remote_read_tail, resolve_target,
};
use crate::cli::CmdError;
use crate::release_agent::host_log_path;

use super::constants::{STREAM_EMPTY, STREAM_MISSING, STREAM_READ};

/// The tail every operator wanted in the incident: enough to carry a panic
/// and its backtrace's first frames, short enough to read in a terminal.
const DEFAULT_LINES: usize = 40;

/// Which of a candidate's two logs to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StreamArg {
    Out,
    Err,
    Both,
}

impl StreamArg {
    /// The file extensions [`crate::release_agent`] writes, in the order an
    /// operator reads them: stderr first. In the incident the answer was in
    /// `.err` and `.out` was empty, and printing stdout first buries it.
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Out => &["out"],
            Self::Err => &["err"],
            Self::Both => &["err", "out"],
        }
    }
}

#[derive(Args)]
pub struct ReleaseLogsArgs {
    pub product: String,
    /// Registry target whose host holds the logs.
    #[arg(long)]
    target: String,
    /// Release version to read logs for. Defaults to the desired version,
    /// which is the version any candidate on the host is running.
    #[arg(long)]
    version: Option<String>,
    #[arg(long, value_enum, default_value_t = StreamArg::Both)]
    stream: StreamArg,
    #[arg(long, default_value_t = DEFAULT_LINES)]
    lines: usize,
    /// Read from the start of each log instead of its tail.
    #[arg(long)]
    head: bool,
    #[arg(long)]
    json: bool,
}

/// One release log as the report carries it.
struct StreamReport {
    stream: &'static str,
    path: String,
    bytes: Option<u64>,
    lines: Vec<String>,
    state: &'static str,
}

impl StreamReport {
    fn to_value(&self) -> Value {
        json!({
            "stream": self.stream,
            "path": self.path,
            "bytes": self.bytes,
            "lines": self.lines,
            "state": self.state,
        })
    }
}

/// Turn what the host said about one log into the report's own account of it.
///
/// Split from the read so the three-way distinction can be exercised without
/// a host: `missing`, `empty` and `read` are the whole point of the command,
/// and folding two of them together is the regression worth a test.
fn classify(path: String, extension: &'static str, read: Option<(String, u64)>) -> StreamReport {
    match read {
        None => StreamReport {
            stream: extension,
            path,
            bytes: None,
            lines: Vec::new(),
            state: STREAM_MISSING,
        },
        Some((_, 0)) => StreamReport {
            stream: extension,
            path,
            bytes: Some(0),
            lines: Vec::new(),
            state: STREAM_EMPTY,
        },
        Some((tail, bytes)) => StreamReport {
            stream: extension,
            path,
            bytes: Some(bytes),
            lines: tail.lines().map(str::to_string).collect(),
            state: STREAM_READ,
        },
    }
}

/// Read one release log from the selected edge of the host file.
///
/// The path is the one [`crate::release_agent`]'s `release_log` opens,
/// spelled by `release_agent::host_log_path` itself rather than retyped, so
/// the reader cannot look somewhere the writer does not write.
async fn stream_report(
    target: &crate::targets::ComputeTarget,
    logs_root: &str,
    product: &str,
    version: &str,
    extension: &'static str,
    lines: usize,
    head: bool,
) -> Result<StreamReport, CmdError> {
    let path = host_log_path(logs_root, product, version, extension);
    let read = if head {
        remote_read_head(target, &path, lines).await?
    } else {
        remote_read_tail(target, &path, lines).await?
    };
    Ok(classify(path, extension, read))
}

pub(super) async fn logs(args: &ReleaseLogsArgs) -> Result<(), CmdError> {
    if args.lines == 0 {
        return Err(CmdError::usage("--lines must be at least 1"));
    }
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, Some(args.target.as_str()))?;
    // The desired version is the version a candidate on this host is
    // running: the agent only ever stages what the registry desires. An
    // operator chasing a version that has since been rolled back names it
    // with `--version`.
    let version = match args.version.clone() {
        Some(version) => version,
        None => policy
            .desired
            .as_ref()
            .map(|desired| desired.version.clone())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} declares no desired release; name the version with --version",
                    args.product
                ))
            })?,
    };
    // `state_dir` and `logs_root` are absolute by registry contract
    // (`release_control::safe_absolute` refuses anything else), so there is
    // nothing to expand here.
    let logs_root = target_policy.logs_root.clone();
    let compute = compute_target(&target_name).await?;
    let mut streams = Vec::new();
    for extension in args.stream.extensions() {
        streams.push(
            stream_report(
                &compute,
                &logs_root,
                &args.product,
                &version,
                extension,
                args.lines,
                args.head,
            )
            .await?,
        );
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "version": version,
        "selection": if args.head { "head" } else { "tail" },
        "streams": streams.iter().map(StreamReport::to_value).collect::<Vec<Value>>(),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for stream in &streams {
        match stream.state {
            STREAM_MISSING => println!("--- {} ({}): no such file", stream.stream, stream.path),
            STREAM_EMPTY => println!(
                "--- {} ({}): present and empty — the agent opened it and the product \
                 wrote nothing",
                stream.stream, stream.path
            ),
            _ => {
                let position = if args.head { "first" } else { "last" };
                println!(
                    "--- {} ({}): {position} {} lines of {} bytes",
                    stream.stream,
                    stream.path,
                    stream.lines.len(),
                    stream.bytes.unwrap_or_default()
                );
                for line in &stream.lines {
                    println!("{line}");
                }
            }
        }
    }
    Ok(())
}
