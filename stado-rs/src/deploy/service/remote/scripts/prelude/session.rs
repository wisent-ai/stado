use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// The session behind the domain, asked as a question
// ---------------------------------------------------------------------------

/// Marker word the session probe frames its one answer in, in the same
/// tab-delimited `STADO_*` family every other program on this channel speaks.
const SESSION_MARKER: &str = "STADO_SESSION";

/// `session.kind` when an account holds the screen: [`DOMAIN_RESOLVER`] found
/// the console owned by this login and launchd holding that login's
/// `gui/<uid>`.
pub const SESSION_GRAPHICAL: &str = "graphical";
/// `session.kind` when nobody holds the screen.
///
/// This one word is the whole of control-host's condition, and the start
/// of the chain nothing in this product could previously state: no graphical
/// session, so launchd builds no `gui/<uid>`, so a LaunchAgent has nowhere to
/// load, so the host publishes no capacity, so a job pinned to it waits.
pub const SESSION_HEADLESS: &str = "headless";
/// `session.kind` when the probe could not answer — the host did not answer
/// at all, the read failed, or it is not a macOS host. Never a guess in
/// either direction: a diagnostic that could not read a fact says so, and an
/// unreadable session must not make a readable host look unreadable.
pub const SESSION_UNKNOWN: &str = "unknown";

/// The wall-clock cap on the session read.
///
/// Deliberately far under the channel's own
/// [`host_channel::remote_timeout`]: this is four `exec`s behind an ssh hop
/// the shared options already bound at `ConnectTimeout=15`, so thirty seconds
/// leaves the reads fifteen of their own. A probe that has not answered by
/// then is [`SESSION_UNKNOWN`] — a diagnostic that hangs on one of its facts
/// is worse than one that reports that fact as unread.
pub const SESSION_TIMEOUT_SECONDS: u64 = 30;

/// The read-only half of [`DOMAIN_RESOLVER`]: who owns the console, whether
/// launchd has a graphical domain, and nothing else.
///
/// `stado service restart` has performed exactly this determination since the
/// fallback domain was named, and until now it took a restart — a write, on a
/// host that is already the wrong shape — to see the answer. This is that
/// same function asked as a question. It resolves a per-login path so the
/// system branch cannot short-circuit the two checks it exists to make, and
/// it bootstraps nothing, kickstarts nothing and loads nothing.
///
/// The reason is flattened over tab/CR/LF, because a marker that spans two
/// lines desynchronises the parser — but it is deliberately NOT cut at 160
/// characters the way the `STADO_DOMAIN` marker cuts it. The reason IS the
/// answer here rather than a note beside one, and a truncated sentence is not
/// the resolver's sentence.
const SESSION_PROBE: &str = "set -u
os=$(/usr/bin/uname -s)
if [ \"$os\" != Darwin ]; then
  printf 'STADO_SESSION\\t%s\\t%s\\t%s\\n' 'unsupported' '' \"$os has no console session of the kind a per-login unit needs\"
  exit 0
fi
uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
console=''
@DOMAIN_RESOLVER@stado_domain_of \"$HOME/Library/LaunchAgents\"
printf 'STADO_SESSION\\t%s\\t%s\\t%s\\n' \"$domain_status\" \"$console\" \"$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ')\"
";

/// What one host answered about whether anybody is logged in on its screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSession {
    /// [`SESSION_GRAPHICAL`], [`SESSION_HEADLESS`] or [`SESSION_UNKNOWN`].
    pub kind: &'static str,
    /// Who owns `/dev/console`: the login name while a graphical session
    /// exists, `root` at the login window. `None` when the probe could not
    /// answer, never an invented name.
    pub console_owner: Option<String>,
    /// [`DOMAIN_RESOLVER`]'s own sentence for this answer, verbatim — the
    /// same words `stado service restart --json` prints under
    /// `launchd_domain.reason`. When the probe could not answer, this is why.
    pub detail: String,
}

impl HostSession {
    /// The answer for a probe that did not produce one, carrying why.
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            kind: SESSION_UNKNOWN,
            console_owner: None,
            detail: detail.into(),
        }
    }

    /// True only for [`SESSION_HEADLESS`]. [`SESSION_UNKNOWN`] is not
    /// headless: acting on a fact nobody read is how a diagnostic starts
    /// inventing findings.
    pub fn is_headless(&self) -> bool {
        self.kind == SESSION_HEADLESS
    }

    /// The probe's one marker line, read out of its stdout.
    pub fn parse(stdout: &str) -> Self {
        let Some(fields) = stdout
            .lines()
            .map(host_channel::marker_fields)
            .find(|fields| fields.first() == Some(&SESSION_MARKER))
        else {
            return Self::unknown(
                "this host ran the session read and printed no answer to it".to_string(),
            );
        };
        let [_, status, console, reason] = fields[..] else {
            return Self::unknown(format!(
                "this host's session answer came back in {} field(s) instead of 4",
                fields.len()
            ));
        };
        // `unavailable` is headless too, and not a failed read: launchd having
        // no `gui/<uid>` for this login is precisely the absence of a
        // graphical session, whatever the console says about who owns it.
        let kind = match status {
            DOMAIN_STATUS_GRAPHICAL => SESSION_GRAPHICAL,
            DOMAIN_STATUS_FALLBACK | DOMAIN_STATUS_UNAVAILABLE => SESSION_HEADLESS,
            _ => SESSION_UNKNOWN,
        };
        Self {
            kind,
            console_owner: (!console.is_empty()).then(|| console.to_string()),
            detail: reason.to_string(),
        }
    }

    /// The one line an operator reads first, in the words they use for it.
    ///
    /// No `gui/<uid>`, no domain and no bootstrap here. Those are true, they
    /// are what the next command needs, and they belong in [`Self::detail`]
    /// underneath — not in the sentence that answers "is anyone logged in".
    pub fn headline(&self) -> String {
        match (self.kind, self.console_owner.as_deref()) {
            (SESSION_GRAPHICAL, Some(owner)) => {
                format!("{owner} is logged in on the screen here")
            }
            (SESSION_GRAPHICAL, None) => "someone is logged in on the screen here".to_string(),
            (SESSION_HEADLESS, _) => "nobody is logged in on the screen here".to_string(),
            _ => "whether anyone is logged in on the screen here could not be read".to_string(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "kind": self.kind,
            "console_owner": self.console_owner,
            "detail": self.detail,
        })
    }
}

/// Ask one host whether anybody is logged in on its screen.
///
/// Read-only and infallible by construction: every way this can fail — an
/// unresolvable key, a refused connection, a remote non-zero, a missing
/// marker — comes back as [`SESSION_UNKNOWN`] carrying the failure's own
/// words. A caller diagnosing a sick host must never lose the facts it
/// already has because one more optional read did not land.
pub async fn read_session(target: &ComputeTarget, runner: &Runner) -> HostSession {
    let probe = SESSION_PROBE.replace("@DOMAIN_RESOLVER@", DOMAIN_RESOLVER);
    match host_channel::run_script_with_timeout(
        target,
        &probe,
        std::time::Duration::from_secs(SESSION_TIMEOUT_SECONDS),
        runner,
    )
    .await
    {
        Ok(output) if output.ok() => HostSession::parse(&output.stdout),
        Ok(output) => HostSession::unknown(host_channel::last_error_line(
            &output,
            "this host refused the session read and said nothing about why",
        )),
        Err(exc) => HostSession::unknown(exc.to_string()),
    }
}
