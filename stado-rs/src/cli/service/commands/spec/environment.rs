//! The environment, endpoint and grant verbs of `stado service`.

use clap::Subcommand;

/// The third block of `stado service` verbs. Flattened into
/// [`super::super::ServiceCommands`], so splitting the declaration across
/// files changes no command line.
#[derive(Subcommand)]
pub enum EnvironmentCommands {
    /// Replace one key in a managed runtime env file or systemd definition.
    ///
    /// The value is read from an owner-only local file and travels only inside
    /// the approved encrypted channel's request body.
    /// A systemd unit or its own .conf drop-in is updated in place; the manager
    /// reloads changed definitions without restarting the running service.
    EnvSet {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        /// Absolute owner-only local file containing the value.
        #[arg(long)]
        value_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove one key from a managed env file or systemd definition.
    EnvUnset {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact environment variable name.
        #[arg(long)]
        key: String,
        /// Runtime env file, or this service's exact systemd unit/drop-in path.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Read a managed service's owner-controlled env file, duplicates and all.
    ///
    /// The counterpart of `env-set`: same approved encrypted channel, same
    /// `$HOME` confinement, opposite direction. `service env` answers what the
    /// UNIT FILE declares; this answers what the file a launcher `.`-sources
    /// declares, which on this fleet is where the interesting values live.
    ///
    /// Every assignment is listed in FILE ORDER with its line number, and a
    /// key assigned twice is reported twice — `effective` for the last
    /// assignment, `shadowed` for every earlier one — because a sourced file
    /// assigns top to bottom and a later duplicate silently wins.
    ///
    /// A value whose key looks like a credential is withheld, and a URL
    /// carrying userinfo is withheld whatever its key is called. An endpoint,
    /// a port, a flag or a `$REFERENCE` is shown whatever its key is called:
    /// those are what an operator reads this file to verify. The decision is
    /// made ON THE HOST, so a withheld value never crosses the channel.
    EnvShow {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to read.
        #[arg(long)]
        host: String,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        /// Show this one variable's value in full, whatever its name suggests.
        /// The key name travels; no secret is ever placed in a remote command
        /// line either way.
        #[arg(long)]
        reveal: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Does this unit's env file agree with what is actually listening?
    ///
    /// The endpoint half of `env-show`: every loopback URL or port the file's
    /// effective assignments declare, checked against the host's own socket
    /// table, with the process that holds each port named. `host inventory`
    /// does this for forward markers; this does it for a unit's environment.
    ///
    /// Exits non-zero when a loopback endpoint is declared and nothing is
    /// listening there, and when the check could not be performed at all.
    EndpointCheck {
        /// Service whose host-local process reads the environment.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Environment file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Is the DECLARED unit the process on its own port?
    ///
    /// `show` reports what the unit file declares and used to spell that
    /// `runs`; `endpoint-check` reports whether anything answers on a declared
    /// port. Neither asks the one question an outage turns on. On 2026-08-30
    /// `com.wisent.always-on.weles` was reported `runs` while both pids its
    /// last restart produced were already gone and its stderr ended in
    /// `EADDRINUSE 127.0.0.1:58101`: something WAS listening there, and it was
    /// a different launchd job — the undeclared unit the Weles release
    /// deployer bootstraps, running an identical argument vector.
    ///
    /// So ownership here is decided by launchd label, never by argv. The pid
    /// holding each port is walked up its own parent chain until a pid appears
    /// in `launchctl list`, because a launcher script is the job and the
    /// server it starts is the child that holds the socket. A label that
    /// cannot be read — a system LaunchDaemon is invisible to an unprivileged
    /// `launchctl list` — is reported `unknown`, never as "nobody owns it".
    ///
    /// Verdicts are `serving`, `not_serving`, and `unknown` for a question
    /// that could not be answered; the third is never folded into either of
    /// the others. Exits non-zero on anything but `serving`, because a control
    /// plane that cannot tell reported this host healthy for days.
    Serving {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// One loopback port this unit is supposed to SERVE; repeat for each.
        ///
        /// Deliberately not taken from the unit's env file. That file names
        /// every endpoint the unit touches, and most of them are ports it
        /// CALLS — `STADO_API_URL`, `WC_SKARBIEC_URL` — owned by other
        /// services on purpose. Judging those as "this unit must own it" makes
        /// every healthy dependency a finding, which is how a check stops
        /// being read. `endpoint-check` is the command for dependencies; this
        /// one is about the ports the service itself answers on. Omit these
        /// and the service directory's declared endpoint for this host is
        /// used.
        #[arg(long = "port")]
        ports: Vec<u16>,
        #[arg(long)]
        json: bool,
    },

    /// Reconcile one Skarbiec consumer grant with an existing owner-only token file.
    ///
    /// The bearer never leaves the managed host: its local Skarbiec reads the
    /// raw file and records only its hash while replacing the declared grant.
    GrantSync {
        /// Service whose host-local deployer uses the grant.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Exact Skarbiec consumer name.
        #[arg(long)]
        consumer: String,
        /// One complete grant capability; repeat for every capability.
        #[arg(long = "capability", required = true)]
        capabilities: Vec<String>,
        /// Existing raw bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        /// Authoritative Skarbiec vault on the target, absolute or rooted at $HOME.
        #[arg(long, default_value = "$HOME/.stado/skarbiec.vault.json")]
        vault_file: String,
        /// Lifetime of the replacement grant.
        #[arg(long, default_value_t = 2_592_000)]
        ttl_seconds: u64,
        /// Grant audience; defaults to the consumer.
        #[arg(long)]
        audience: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Write one Skarbiec item field into an owner-only raw bearer file.
    ///
    /// `WC_STADO_STORAGE_TOKEN_FILE` has to name a file whose entire content
    /// is the bearer, because `queue/stado_object.rs` resolves a token file
    /// and nothing else. `secret-sync` can put a Skarbiec field into a unit's
    /// env file, and `grant-sync` can reconcile a grant against a token file
    /// that is already on the host, but nothing could create that file. So the
    /// only remaining way to bind a host to the fleet object store was to
    /// hand-copy a secret onto it, which is the one thing the fleet-wide
    /// "everything through Stado" rule exists to prevent. Lacking the file,
    /// charless-mac-mini's queue agent bound its `JobStorage` to a
    /// device-local store instead and published no capacity for seven days
    /// while 74 fleet jobs waited on a host every surface reported as
    /// in-sync -- a fleet claim written to a device store does not fail, it
    /// succeeds where nobody else can see it.
    ///
    /// The value is read through the isolated service-verifier grant and
    /// carried in the SSH request body. It is never printed or placed in argv.
    TokenFileSync {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Destination bearer file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        token_file: String,
        #[arg(long)]
        json: bool,
    },

    /// Verify a managed service's bearer against a read-only loopback endpoint.
    ///
    /// With `--repair`, a failed check atomically synchronizes the secret,
    /// restarts the unit, and checks the endpoint once more.
    AuthCheck {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer. Required unless the bearer
        /// is read from the unit's own runtime environment with --variable
        /// and --env-file instead.
        #[arg(long)]
        item: Option<String>,
        /// Host-side Skarbiec consumer used to read --item (defaults to the
        /// host's own selection).
        #[arg(long)]
        consumer: Option<String>,
        /// Token file for --consumer.
        #[arg(long)]
        token_file: Option<String>,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Read-only loopback HTTP endpoint that requires authentication.
        #[arg(long)]
        url: String,
        /// Send an empty JSON POST instead of a GET; useful for auth-first APIs.
        #[arg(long)]
        post_empty_json: bool,
        /// Treat this exact HTTP status as proof that authentication passed.
        #[arg(long)]
        expect_status: Option<u16>,
        /// On failure, synchronize the secret, restart, and check again.
        #[arg(long)]
        repair: bool,
        /// If repair still fails, stop the unmanaged process owning the URL port.
        #[arg(long, requires = "repair")]
        take_over_listener: bool,
        /// Environment variable holding the bearer. With --item omitted this
        /// names the assignment auth-check reads from --env-file; with
        /// --repair it is the assignment synchronized from the item.
        #[arg(long)]
        variable: Option<String>,
        /// Runtime env file holding (or, with --repair, receiving) the
        /// bearer assignment named by --variable.
        #[arg(long)]
        env_file: Option<String>,
        #[arg(long)]
        json: bool,
    },
}
