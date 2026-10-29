//! The one shell command the worker runs for an `omp-session`.

use super::super::{shell_quote, workspace_expression};
use crate::cli::workload::catalog::WorkloadHarness;

/// What one session command is built from.
pub(super) struct Session<'a> {
    pub checkout: &'a str,
    pub origin: &'a str,
    pub harness: &'a WorkloadHarness,
    pub task: &'a str,
    pub model: Option<&'a str>,
    pub max_time: &'a str,
    pub allow_write: bool,
    pub allow_command: bool,
}

/// The environment variable naming the directory the session writes its
/// result lines into, one `<name>.jsonl` per subject; `stado workload results`
/// reads it back from the job's output directory.
pub(crate) const RESULTS_VARIABLE: &str = "STADO_SESSION_RESULTS";

/// The job's output directory's results subdirectory, relative to the job's
/// work tree the worker starts the command in.
pub(crate) const RESULTS_DIRECTORY: &str = "output/results";

/// The harness's agent directory for this one session, inside the job's work
/// tree: its settings, model routes and session store live and end with the
/// job, so nothing the session does is written into the host account's own
/// harness configuration.
const AGENT_DIRECTORY: &str = "harness-agent";

/// The model gateway and the harness's agent directory, as shell: the host's
/// own Stado resolves the gateway's address for the declared consumer, and
/// every provider the declaration routes is sent there with the job's
/// gateway token, read by the harness from the environment variable the
/// declaration names, never written into the file.
fn gateway_setup(harness: &WorkloadHarness) -> String {
    let gateway = &harness.gateway;
    let mut routes = String::from("providers:\\n");
    for provider in &gateway.providers {
        routes.push_str(&format!(
            "  {provider}:\\n    baseUrl: %s\\n    transport: pi-native\\n    apiKey: {}\\n",
            gateway.token_variable
        ));
    }
    let arguments = gateway
        .providers
        .iter()
        .map(|_| "\"$gateway\"")
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "set -- $(\"$HOME\"/.stado/bin/stado service directory connect {service} --consumer \
         {consumer} --no-verify) || exit; gateway=$1; \
         case \"$gateway\" in http://127.0.0.1:*) ;; *) printf 'the service directory gives no \
         loopback route to %s for consumer %s on this host: %s\\n' {service} {consumer} \
         \"$gateway\" >&2; exit 1;; esac; \
         PI_CODING_AGENT_DIR=\"$PWD/{AGENT_DIRECTORY}\"; export PI_CODING_AGENT_DIR; \
         mkdir -p \"$PI_CODING_AGENT_DIR\" || exit; \
         printf '{routes}' {arguments} > \"$PI_CODING_AGENT_DIR/models.yml\" || exit; ",
        service = shell_quote(&gateway.service),
        consumer = shell_quote(&gateway.consumer),
    )
}

/// The command: results directory, the model gateway, the host's one
/// checkout of the repository (cloned from origin when absent, fetched when
/// present), the managed harness, then the harness itself in print mode with
/// the grants the caller gave. Git reaches GitHub with the job's
/// `GITHUB_TOKEN` through an environment-declared credential helper, so the
/// token never enters a command line or a file.
pub(super) fn session_command(session: &Session<'_>) -> String {
    let workspace = workspace_expression(session.checkout);
    let origin = shell_quote(session.origin);
    let managed = &session.harness.managed;
    let helper =
        shell_quote("!f() { echo username=x-access-token; echo \"password=$GITHUB_TOKEN\"; }; f");
    let git_environment = format!(
        "GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=credential.https://github.com.helper \
         GIT_CONFIG_VALUE_0= GIT_CONFIG_KEY_1=credential.https://github.com.helper \
         GIT_CONFIG_VALUE_1={helper}"
    );
    let approval = if session.allow_write && session.allow_command {
        " --approval-mode yolo"
    } else if session.allow_write {
        " --approval-mode write"
    } else {
        ""
    };
    let mut command = format!(
        "{RESULTS_VARIABLE}=\"$PWD/{RESULTS_DIRECTORY}\"; export {RESULTS_VARIABLE}; \
         mkdir -p \"${RESULTS_VARIABLE}\" || exit; \
         {gateway}\
         export {git_environment}; \
         if [ -d {workspace}/.git ]; then git -C {workspace} fetch --quiet origin || exit; \
         else mkdir -p {workspace} && git clone --quiet {origin} {workspace} || {{ printf 'the worker cannot clone %s into %s on this host\\n' {origin} {workspace} >&2; exit 1; }}; fi; \
         if ! cd {workspace}; then printf 'the worker cannot enter %s on this host; grant the Stado agent access to that directory\\n' {workspace} >&2; exit 1; fi; \
         if [ ! -x \"$HOME\"/{managed} ]; then printf 'the managed harness is missing or not executable at %s on this host; deliver it with `stado workload harness install`\\n' \"$HOME\"/{managed} >&2; exit 1; fi; \
         PATH=\"$HOME/.stado/bin:$PATH\" exec \"$HOME\"/{managed} -p --no-title --max-time {}{approval}",
        shell_quote(session.max_time),
        gateway = gateway_setup(session.harness),
    );
    if let Some(model) = session.model {
        command.push_str(&format!(" --model {}", shell_quote(model)));
    }
    command.push_str(&format!(" {}", shell_quote(session.task)));
    command
}
