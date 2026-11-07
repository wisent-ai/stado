//! Attached execution of one confined program: the host-side signal marker,
//! the forwarding of HUP/INT/TERM to the remote child, and the outcome the
//! caller sees. The scripts run on the host are in [`script`]; the read of
//! who still holds the program's output once it exited is in [`holders`];
//! what the channel's stderr says and the waits it marks are in [`channel`];
//! the outcome and how its end is judged are in [`outcome`].

mod channel;
mod holders;
mod outcome;
mod script;

use std::process::Stdio;

use tokio::io::AsyncReadExt;

use crate::deploy::{
    host_access::ssh_key, host_channel, production_runner, shlex_quote, CommandSpec, DeployError,
};
use crate::targets::ComputeTarget;

pub use outcome::{AttachedOutcome, Ending};

async fn forward_signal(
    connection: Option<&str>,
    key: Option<&ssh_key::KeyFile>,
    token: &str,
    signal: &str,
) -> Result<(), DeployError> {
    let script = script::signal(token, signal);
    let argv = match (connection, key) {
        (None, None) => vec!["/bin/bash".to_string(), "-s".to_string()],
        (Some(destination), Some(key)) => {
            ssh_key::add_identity(host_channel::ssh_script_argv(destination), key)?
        }
        _ => {
            return Err(
                DeployError("attached host channel is incomplete".to_string())
                    .stating(crate::primitives::failure::FailureCode::Config),
            )
        }
    };
    let output = production_runner()(CommandSpec {
        argv,
        stdin: Some(script),
    })
    .await
    .map_err(DeployError::unreachable)?;
    if output.succeeded() {
        Ok(())
    } else {
        Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the attached signal was not delivered",
        )))
    }
}

pub async fn run_attached(
    target: &ComputeTarget,
    program: &str,
    arguments: &[String],
    capture_output: bool,
) -> Result<AttachedOutcome, DeployError> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let script = script::attached(program, arguments, &token);
    let runner = production_runner();
    let (argv, key, connection) = if host_channel::target_is_this_host(target) {
        (
            vec!["/bin/bash".to_string(), "-c".to_string(), script],
            None,
            None,
        )
    } else {
        let connection = host_channel::select_ssh_connection(target, &runner).await?;
        let key = ssh_key::materialize(target.channel_key()).await?;
        let options = host_channel::ssh_options(connection.destination);
        let Some((ssh, rest)) = options.split_first() else {
            return Err(DeployError("the ssh channel options are empty".to_string())
                .stating(crate::primitives::failure::FailureCode::Config));
        };
        let mut argv = vec![ssh.clone(), "-T".to_string()];
        argv.extend_from_slice(rest);
        // One quoted word for the login shell, and the script itself under the
        // same interpreter the local branch above uses. Pushing the script
        // bare made the account's login shell the interpreter, and this is the
        // only channel that did: every other one sends `/bin/bash -s` or
        // `/bin/sh -c` and keeps the script off the login shell's grammar.
        // A run against a zsh account reports `exit_code: 1, status:
        // failed` for an install that succeeded --
        // `status` is read-only in zsh, so the wrapper's own bookkeeping
        // assignment failed on a line the program never reached. Any shell
        // whose reserved names differ from bash's had the same power over
        // this outcome, which is why the repair is the interpreter and not
        // the variable name.
        argv.push(format!("/bin/bash -c {}", shlex_quote(&script)));
        let argv = ssh_key::add_identity(argv, &key)?;
        (argv, Some(key), Some(connection.destination.to_string()))
    };
    let (command, arguments_argv) = argv.split_first().ok_or_else(|| {
        DeployError("attached host channel is empty".to_string())
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut command = tokio::process::Command::new(command);
    command
        .args(arguments_argv)
        .stdin(Stdio::inherit())
        .kill_on_drop(true);
    if capture_output {
        command.stdout(Stdio::piped());
    } else {
        command.stdout(Stdio::inherit());
    }
    // Stderr is read here in both forms: the wrapper reports the program's
    // status and the stages after it there ([`channel`]); without capture
    // every chunk goes on to this process's stderr as it arrives.
    command.stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(DeployError::from)?;
    let mut stages = channel::Stages::begin(&target.name);
    let (exited, reported) = tokio::sync::oneshot::channel();
    let stdout_reader = child.stdout.take().map(|mut stdout| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        })
    });
    let stderr_reader = child.stderr.take().map(|stderr| {
        tokio::spawn(channel::relay(
            stderr,
            capture_output,
            holders::wait(&target.name),
            target.name.clone(),
            exited,
        ))
    });
    let mut forwarded_signals = Vec::new();

    #[cfg(unix)]
    let status = {
        use tokio::signal::unix::{signal, SignalKind};
        let mut hangup = signal(SignalKind::hangup()).map_err(DeployError::from)?;
        let mut interrupt = signal(SignalKind::interrupt()).map_err(DeployError::from)?;
        let mut terminate = signal(SignalKind::terminate()).map_err(DeployError::from)?;
        let waited = child.wait();
        tokio::pin!(waited);
        tokio::pin!(reported);
        let mut awaiting_report = true;
        loop {
            tokio::select! {
                status = &mut waited => break status.map_err(DeployError::from)?,
                said = &mut reported, if awaiting_report => {
                    awaiting_report = false;
                    if said.is_ok() {
                        stages.program_exited();
                    }
                }
                received = hangup.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "HUP").await?;
                        forwarded_signals.push("SIGHUP".to_string());
                    }
                }
                received = interrupt.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "INT").await?;
                        forwarded_signals.push("SIGINT".to_string());
                    }
                }
                received = terminate.recv() => {
                    if received.is_some() {
                        forward_signal(connection.as_deref(), key.as_ref(), &token, "TERM").await?;
                        forwarded_signals.push("SIGTERM".to_string());
                    }
                }
            }
        }
    };
    #[cfg(not(unix))]
    let status = {
        drop(reported);
        child.wait().await.map_err(DeployError::from)?
    };

    let stdout = match stdout_reader {
        Some(reader) => Some(
            String::from_utf8_lossy(
                &reader
                    .await
                    .map_err(|error| DeployError::unreachable(error.to_string()))?
                    .map_err(DeployError::from)?,
            )
            .into_owned(),
        ),
        None => None,
    };
    let relayed = match stderr_reader {
        Some(reader) => reader
            .await
            .map_err(|error| DeployError::unreachable(error.to_string()))?
            .map_err(DeployError::from)?,
        None => channel::Relayed::default(),
    };
    stages.ended(relayed.program_exit, &status.to_string());
    drop(key);
    Ok(AttachedOutcome {
        target: target.name.clone(),
        program: program.to_string(),
        arguments: arguments.to_vec(),
        status: if relayed.program_exit == Some(nix::libc::EXIT_SUCCESS) {
            "exited"
        } else {
            "failed"
        },
        program_exit: relayed.program_exit,
        wrapper_exit: relayed.wrapper_exit,
        exit_code: status.code(),
        ended: status.to_string(),
        forwarded_signals,
        stdout,
        stderr: capture_output.then(|| String::from_utf8_lossy(&relayed.kept).into_owned()),
    })
}
