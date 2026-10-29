//! What enrollment asks the machine before anything is written: its
//! hostname and release platform, through Stado's deploy channel.

use crate::cli::fleet::enroll::failures::{
    probe_failure, CHANNEL_KEY, IDENTITY_AMBIGUOUS, PLATFORM_UNSUPPORTED, SSH_UNSTARTABLE,
};
use crate::cli::CmdError;

/// Probe one fixed identity command through Stado's existing deploy channel.
/// Each way the channel or the command fails is the declared cause OpenSSH's
/// answer names ([`probe_failure`]); an answer that is not one line is
/// refused, because the machine is not what enrollment can verify.
async fn probe_identity_field(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
    command: &str,
) -> Result<String, CmdError> {
    let values = [("name", target), ("ssh", destination), ("command", command)];
    let (argv, _key) = crate::cli::fleet::key::channel_argv(target, destination, command)
        .await
        .map_err(|error| CHANNEL_KEY.raise(error, &values))?;
    let output = runner(crate::deploy::CommandSpec::new(argv))
        .await
        .map_err(|exc| {
            CmdError::declared(
                &SSH_UNSTARTABLE,
                &[("ssh", destination), ("answer", &exc.to_string())],
            )
        })?;
    if !output.succeeded() {
        let answer = output.detail().trim().to_string();
        let status = output.code.to_string();
        return Err(CmdError::declared(
            probe_failure(output.code, &answer),
            &[
                ("name", target),
                ("ssh", destination),
                ("command", command),
                ("status", &status),
                ("answer", &answer),
            ],
        ));
    }
    let value = output.stdout.trim();
    if value.is_empty() || value.contains('\n') {
        let answer = format!("{:?}", output.stdout);
        return Err(CmdError::declared(
            &IDENTITY_AMBIGUOUS,
            &[
                ("ssh", destination),
                ("command", command),
                ("answer", &answer),
            ],
        ));
    }
    Ok(value.to_string())
}

/// Verify hostname and immutable-release platform before any registry write.
pub(super) async fn probe_identity(
    runner: &crate::deploy::Runner,
    target: &str,
    destination: &str,
) -> Result<(String, &'static str), CmdError> {
    let raw_hostname = probe_identity_field(runner, target, destination, "hostname").await?;
    let hostname = crate::targets::normalize_hostname(&raw_hostname);
    if hostname.is_empty() {
        let answer = format!("{raw_hostname:?}");
        return Err(CmdError::declared(
            &IDENTITY_AMBIGUOUS,
            &[
                ("ssh", destination),
                ("command", "hostname"),
                ("answer", &answer),
            ],
        ));
    }
    let os = probe_identity_field(runner, target, destination, "uname -s").await?;
    let arch = probe_identity_field(runner, target, destination, "uname -m").await?;
    let platform = crate::cli::fleet::enroll::release_platform(&os, &arch).map_err(|_| {
        CmdError::declared(
            &PLATFORM_UNSUPPORTED,
            &[("name", target), ("os", &os), ("arch", &arch)],
        )
    })?;
    Ok((hostname, platform))
}
