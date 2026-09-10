//! Declared end states: the marker a probe prints, what met, unmet and
//! unobserved each mean, and the verdict a caller reads instead of trusting
//! that a command which exited zero left the host where it said it would.

use super::marker_fields;
use super::run::run_script;
use crate::deploy::{shlex_quote, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

// ---------------------------------------------------------------------------
// Declared end states
// ---------------------------------------------------------------------------

/// The marker a postcondition probe prints, in the same tab-delimited family
/// as every `STADO_*` marker on this channel.
pub const POSTCONDITION_MARKER: &str = "POSTCONDITION";

/// The probe found the host in the state the operation said it would leave
/// behind.
pub const POSTCONDITION_MET: &str = "met";
/// The probe ran and the host is NOT in that state.
pub const POSTCONDITION_UNMET: &str = "unmet";
/// No verdict line came back at all, so the end state was never observed.
/// Deliberately not folded into [`POSTCONDITION_UNMET`]: "the host says no"
/// and "nobody asked the host" are different facts, and only one of them
/// tells an operator where to look.
pub const POSTCONDITION_UNOBSERVED: &str = "unobserved";

/// The end state a host operation intends, and the probe that checks it on
/// the host itself.
///
/// `stado service restart` on the always-on Mac booted a unit out, could not
/// bootstrap it back because launchd still held children of the old job,
/// reported `restart_failed: disowned process survived` and left the unit
/// UNLOADED. Every step of that script did exactly what it was written to
/// do. Nothing at all asked whether the machine had ended up in the state
/// the operator asked for, so the one fact that mattered — the listeners are
/// gone — was the one fact no report carried. An operation that states its
/// intended end state and never compares it against the world is a
/// declaration, not a check.
///
/// `describe` is the intent in the operator's words ("the unit is loaded and
/// has a pid"), and doubles as the probe's identity in the output so a line
/// printed by some other program cannot be read as this operation's verdict.
/// `probe` is a POSIX sh fragment that prints exactly
/// `POSTCONDITION\t<describe>\t<met|unmet>\t<detail>`; it emits that through
/// the `stado_post` helper [`PostCondition::arm`] defines, which is `say`
/// with a different marker word, because a second output convention on this
/// channel would be a second parser to keep in step.
pub struct PostCondition {
    /// The intended end state, in the operator's words.
    pub describe: &'static str,
    /// POSIX sh that observes the host and emits one verdict line.
    pub probe: String,
}

impl PostCondition {
    /// The shell that defines the verdict emitter and arms the probe.
    ///
    /// Armed as an `EXIT` trap and spliced in BEFORE the operation body
    /// rather than appended after it, for two reasons that are not
    /// stylistic. Every lifecycle body on this channel exits the moment it
    /// has something to say — `say 'restarted' "$domain in place"; exit 0`
    /// is the FIRST branch of the restart script — so a fragment appended
    /// after the body would run on every path except the ones that claim
    /// success, which are precisely the paths that lied. And a trap runs in
    /// the operation's own shell, so it observes the domain and unit the
    /// body finally settled on rather than the ones it started with; the
    /// restart fallback renames `$unit` to the `-recovery` label, and a
    /// probe reading the original label would call a healthy host broken.
    ///
    /// The trap hands back the status that triggered it, so declaring an
    /// end state cannot change the exit code the transport reports.
    fn arm(&self) -> String {
        let describe = shlex_quote(self.describe);
        let probe = &self.probe;
        format!(
            "stado_post() {{
  pc_detail=$(printf '%s' \"$2\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf '{POSTCONDITION_MARKER}\\t%s\\t%s\\t%s\\n' {describe} \"$1\" \"$pc_detail\"
}}
stado_postcondition() {{
  pc_rc=$?
{probe}  return $pc_rc
}}
trap stado_postcondition EXIT
"
        )
    }
}

/// What the host said about the end state an operation promised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostConditionVerdict {
    /// The intended end state, echoed back so a report can print the intent
    /// beside the outcome instead of a bare `unmet`.
    pub describe: String,
    /// [`POSTCONDITION_MET`], [`POSTCONDITION_UNMET`] or
    /// [`POSTCONDITION_UNOBSERVED`].
    pub state: String,
    /// The probe's own words about what it found.
    pub detail: String,
}

/// Read one probe verdict out of an operation's stdout.
///
/// A verdict for some other end state is ignored, and a missing verdict is
/// [`POSTCONDITION_UNOBSERVED`] rather than an optimistic default: an
/// unobserved end state is exactly the thing this channel is no longer
/// allowed to assume in the operation's favour.
pub fn postcondition_verdict(stdout: &str, postcondition: &PostCondition) -> PostConditionVerdict {
    for line in stdout.lines() {
        if let [POSTCONDITION_MARKER, describe, state, detail] = marker_fields(line).as_slice() {
            if *describe == postcondition.describe {
                return PostConditionVerdict {
                    describe: (*describe).to_string(),
                    state: (*state).to_string(),
                    detail: (*detail).to_string(),
                };
            }
        }
    }
    PostConditionVerdict {
        describe: postcondition.describe.to_string(),
        state: POSTCONDITION_UNOBSERVED.to_string(),
        detail: "the host returned no verdict for this end state".to_string(),
    }
}

/// Run a fixed remote script that has to leave the host in a declared state,
/// and bring back both halves of the answer: what the operation said about
/// itself, and what the host said about the state the operation promised.
///
/// `head` is the operation's vocabulary (the caller's prelude, which the
/// probe reads its unit, domain and init-system helpers from) and `body` is
/// the operation. Assembling the two here, rather than letting each caller
/// concatenate its own, is what keeps the arming order — and therefore the
/// guarantee that the probe survives an early `exit` — in one place.
pub async fn run_checked_script(
    target: &ComputeTarget,
    head: &str,
    body: &str,
    postcondition: &PostCondition,
    runner: &Runner,
) -> Result<(CommandOutput, PostConditionVerdict), DeployError> {
    let script = format!("{head}{}{body}", postcondition.arm());
    let output = run_script(target, &script, runner).await?;
    let verdict = postcondition_verdict(&output.stdout, postcondition);
    Ok((output, verdict))
}
