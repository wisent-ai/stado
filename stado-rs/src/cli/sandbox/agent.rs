//! The `stado sandbox` verbs an agent uses beyond a sandbox's life: rename
//! it or change when it stops, hand its coding agent a prompt and follow the
//! run, interrupt it, read its events, allow an SSH key, download a file,
//! give a port a public URL, and keep its disk as a named snapshot. Each is
//! one Boat Public API v1 call through the provider's client.

use clap::ValueEnum;
use serde_json::{json, Value};

use crate::providers::r#box::{BoxPromptRun, TtlUpdate};
use crate::providers::BoxProvider;

use super::snapshots::write_new;
use super::verbs::{failure, print, sandbox};
use super::{
    CmdError, DownloadArgs, EventsArgs, HostArgs, ProcessArgs, PromptArgs, PromptRunId,
    SnapshotSaveArgs, SshKeyArgs, SteerArgs, UpdateArgs,
};

/// An optional argument as the client takes it: the stated value, or no
/// value, which the client leaves out of the request. A stated empty value
/// is refused naming its option, so it is never sent as if it were absent.
fn stated<'a>(value: Option<&'a str>, option: &str) -> Result<&'a str, CmdError> {
    match value {
        Some("") => Err(CmdError::usage(format!("{option} is given with no value"))),
        Some(value) => Ok(value),
        None => Ok(""),
    }
}

fn run(prompt_run: &BoxPromptRun) -> Value {
    json!({
        "promptId": prompt_run.prompt_id,
        "status": prompt_run.status.to_string(),
        "done": prompt_run.done,
        "run": Value::Object(prompt_run.raw.clone()),
    })
}

pub(super) async fn update(provider: &BoxProvider, args: &UpdateArgs) -> Result<(), CmdError> {
    let auto_stop = match (args.ttl_seconds, args.no_auto_stop) {
        (Some(stated), _) if !stated.is_positive() => {
            return Err(CmdError::usage(format!(
                "--ttl-seconds {stated} is not a positive number of seconds a sandbox can live"
            )))
        }
        (Some(stated), _) => TtlUpdate::Set(stated),
        (None, true) => TtlUpdate::Clear,
        (None, false) => TtlUpdate::NotProvided,
    };
    let name = match args.name.as_deref() {
        Some("") => return Err(CmdError::usage("--name is given with no value")),
        name => name,
    };
    let info = provider
        .client
        .update_box(&args.id, name, auto_stop)
        .await
        .map_err(failure)?;
    print(&sandbox(&info))
}

pub(super) async fn prompt(provider: &BoxProvider, args: &PromptArgs) -> Result<(), CmdError> {
    if args.prompt.is_empty() {
        return Err(CmdError::usage("the prompt is empty"));
    }
    if args.provider.is_empty() {
        return Err(CmdError::usage("--provider is given with no value"));
    }
    let model = stated(args.model.as_deref(), "--model")?;
    let effort = stated(args.reasoning_effort.as_deref(), "--reasoning-effort")?;
    let submitted = provider
        .client
        .prompt(&args.id, &args.prompt, &args.provider, model, effort)
        .await
        .map_err(failure)?;
    print(&run(&submitted))
}

pub(super) async fn prompt_status(
    provider: &BoxProvider,
    args: &PromptRunId,
) -> Result<(), CmdError> {
    let status = provider
        .client
        .prompt_status(&args.id, &args.prompt_id)
        .await
        .map_err(failure)?;
    print(&run(&status))
}

pub(super) async fn interrupt(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.interrupt(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn events(provider: &BoxProvider, args: &EventsArgs) -> Result<(), CmdError> {
    let order = args.sort.to_possible_value().ok_or_else(|| {
        CmdError::usage(format!(
            "--sort {:?} names no order Boat lists events in",
            args.sort
        ))
    })?;
    let cursor = stated(args.cursor.as_deref(), "--cursor")?;
    let event_type = stated(args.event_type.as_deref(), "--type")?;
    let page = provider
        .client
        .list_events(&args.id, cursor, order.get_name(), event_type)
        .await
        .map_err(failure)?;
    let events: Vec<Value> = page.events.into_iter().map(Value::Object).collect();
    print(&json!({
        "events": events,
        "hasMore": page.has_more,
        "nextCursor": page.next_cursor,
    }))
}

pub(super) async fn ssh_key(provider: &BoxProvider, args: &SshKeyArgs) -> Result<(), CmdError> {
    let key = std::fs::read_to_string(&args.public_key_file).map_err(|error| {
        CmdError::click(format!(
            "cannot read the public key {} for sandbox {}: {error}",
            args.public_key_file.display(),
            args.id
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let answer = provider
        .client
        .configure_ssh_key(&args.id, key.trim())
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn download(provider: &BoxProvider, args: &DownloadArgs) -> Result<(), CmdError> {
    let bytes = provider
        .client
        .download_artifact(&args.id, &args.path)
        .await
        .map_err(failure)?;
    write_new(
        &args.to,
        &bytes,
        &format!("{} of sandbox {}", args.path, args.id),
    )?;
    print(&json!({
        "sandbox": args.id,
        "path": args.path,
        "to": args.to.display().to_string(),
        "bytes": bytes.len(),
    }))
}

pub(super) async fn host(provider: &BoxProvider, args: &HostArgs) -> Result<(), CmdError> {
    let answer = provider
        .client
        .host_port(&args.id, args.port, args.public, args.localhost)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn process_status(
    provider: &BoxProvider,
    args: &ProcessArgs,
) -> Result<(), CmdError> {
    let answer = provider
        .client
        .command_status(&args.id, args.process)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn process_stop(
    provider: &BoxProvider,
    args: &ProcessArgs,
) -> Result<(), CmdError> {
    let answer = provider
        .client
        .stop_command(&args.id, args.process)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn usage(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.usage(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn steer(provider: &BoxProvider, args: &SteerArgs) -> Result<(), CmdError> {
    if args.conversation.as_deref() == Some("") {
        return Err(CmdError::usage("--conversation is given with no value"));
    }
    let answer = provider
        .client
        .steer(&args.id, &args.message, args.conversation.as_deref())
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn desktop(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.desktop_url(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn snapshot_save(
    provider: &BoxProvider,
    args: &SnapshotSaveArgs,
) -> Result<(), CmdError> {
    let answer = provider
        .client
        .save_named_snapshot(&args.id, &args.name)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn snapshot_list(provider: &BoxProvider) -> Result<(), CmdError> {
    let answer = provider.client.named_snapshots().await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn snapshot_show(provider: &BoxProvider, name: &str) -> Result<(), CmdError> {
    let answer = provider
        .client
        .named_snapshot(name)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn snapshot_delete(provider: &BoxProvider, name: &str) -> Result<(), CmdError> {
    let answer = provider
        .client
        .delete_named_snapshot(name)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}
