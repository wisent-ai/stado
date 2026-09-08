use crate::deploy::service::*;

/// The label prefix every unit this fleet installs carries, whichever writer
/// installed it: `local_install::LABEL_PREFIX` mints
/// `com.wisent.compute.<kind>.<name>` and the always-on set is
/// `com.wisent.always-on.<name>`, so one prefix covers both.
///
/// It NAMES a finding and never decides what gets looked at. It used to do
/// both, in three places at once — the `launchctl list` filter in
/// [`LOADED_LABELS_SCRIPT`], that script's `com.wisent.*.plist` glob, and a
/// `starts_with` in [`loaded_units`] — and a process outside the prefix could
/// therefore not be reported as undeclared, because it was never enumerated.
/// On 2026-09-01 charless-mac-mini had `com.stado.agent.charless-mac-mini`
/// loaded, the only label on the host outside `com.wisent.`, holding the pid
/// that was overwriting the janitor's state file — and
/// `service list --undeclared` answered that the host had no undeclared unit.
/// That answer was true about a window and false about the host.
///
/// This is the same shape as every other defect this module records: a
/// declaration checked against something narrower than the world. The fix is
/// not a wider prefix, because any prefix has an outside. The enumeration
/// walks every loaded label and every unit file, and the prefix survives only
/// as [`UndeclaredUnit::classification`] — `undeclared` and
/// `outside-fleet-prefix` are different sentences about a row, not different
/// decisions about whether to look.
pub(crate) const FLEET_LABEL_PREFIX: &str = "com.wisent.";

/// One launchd job loaded on a host, with everything the host could say about
/// it. The registry decides whether it is declared; the label's spelling
/// decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredUnit {
    pub host: String,
    pub label: String,
    /// Does the registry declare a service at this exact label on this host?
    ///
    /// The one question that decides whether a loaded job is accounted for.
    /// It is asked of the registry document, never of the label's spelling.
    pub declared: bool,
    /// The pid launchd holds for the label, or empty when it holds none.
    pub pid: String,
    /// The label's last exit status as launchd reports it.
    pub status: String,
    /// The unit file the host found for the label, or empty when it found none.
    pub path: String,
    /// Where `path` came from: `fleet-directory` for one of the three launchd
    /// directories this fleet installs into, `launchd` when only launchd knew
    /// and the host had to ask it, empty when no unit file was found at all.
    ///
    /// `com.stado.agent.charless-mac-mini` is loaded on charless-mac-mini from
    /// none of those three directories, so every reader that looked only there
    /// saw a label with no file and no program behind it.
    pub path_source: String,
    /// The argument vector that unit file declares, flattened to one line. A
    /// label alone is not actionable: three naming conventions produce three
    /// unrelated-looking labels for one job, and only the program says they are
    /// the same job.
    pub program: String,
    /// EVERY unit file found for this label, across the system-daemon, user-agent
    /// and system-agent domains. `path` is the first of these and stays what it
    /// was; this is the list, because more than one entry means one label is
    /// declared in more than one domain and launchd will happily run both.
    pub declaring_paths: Vec<String>,
    /// Every launchd domain that actually HOLDS this label, from that domain's
    /// own service table rather than from `launchctl list`.
    ///
    /// Empty means launchd holds no job under the label anywhere this login
    /// can print. Non-empty with no `declaring_paths` is the state that hid
    /// the respawner of #286: loaded, restarting, and in no directory.
    pub loaded_domains: Vec<String>,
    /// How many times launchd has started this job.
    ///
    /// A one-shot with `KeepAlive` does not read as broken anywhere else: it
    /// reads `active`, exits, and is restarted forever. The count is the only
    /// number that says so.
    pub runs: Option<u64>,
    /// The job's last exit code as `launchctl print` states it, which is a
    /// different field from [`Self::status`] and available in every domain.
    pub last_exit: Option<i64>,
    /// The variable names the unit file hands the program.
    pub env_keys: Vec<String>,
    /// Uppercase variables the program's own script reads.
    pub script_reads: Vec<String>,
    /// Uppercase variables that script sets or defaults for itself.
    pub script_assigns: Vec<String>,
    /// The argument vector the pid launchd holds is ACTUALLY executing, as the
    /// process table reports it, or empty when the label holds no pid.
    ///
    /// The declaration and the process are two different facts and this fleet
    /// has had them disagree: `com.wisent.compute.service.stado-local-control-plane`
    /// declares `stado coordinator` and launchd was holding a five-day-old
    /// `stado dashboard` under it — a command the product deleted on
    /// 2026-08-19, whose refresh loop still forced a disk-cleanup pass every
    /// two minutes and stamped the janitor's shared interval out from under
    /// the queue agent. Every report that read the unit file agreed with
    /// itself and none of them was looking at the process.
    pub running_program: String,
    /// When that process started, and when the binary it is executing was
    /// last written. A process older than its own binary is running code
    /// nobody shipped: the delivery landed, the unit was never restarted, and
    /// the label keeps answering with the previous version.
    pub started_epoch: Option<i64>,
    pub binary_written_epoch: Option<i64>,
}

impl UndeclaredUnit {
    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "label": self.label,
            "pid": self.pid,
            "status": self.status,
            "declared": self.declared,
            "path": self.path,
            "path_source": self.path_source,
            "classification": self.classification(),
            "program": self.program,
            "declaring_paths": self.declaring_paths,
            "running_program": self.running_program,
            "started_epoch": self.started_epoch,
            "binary_written_epoch": self.binary_written_epoch,
        })
    }

    /// Does this label carry the prefix every unit this fleet installs
    /// carries?
    ///
    /// A fact about the name, kept out of every path that decides what to
    /// enumerate. See [`FLEET_LABEL_PREFIX`].
    pub fn in_fleet_prefix(&self) -> bool {
        self.label.starts_with(FLEET_LABEL_PREFIX)
    }

    /// Is there evidence tying this label to this fleet, independent of what it
    /// is called?
    ///
    /// Two facts, both read off the host: its unit file sits in one of the
    /// three launchd directories this fleet installs into, or the program it is
    /// running executes out of a declared product root.
    ///
    /// This exists because the widened enumeration has to stay readable.
    /// charless-mac-mini loads 537 labels and 494 of them are `com.apple.*`;
    /// a report that prints all of them equally has buried its finding as
    /// effectively as the prefix filter did, and burying a finding in noise is
    /// the failure this whole change is about. So the noise is separated by
    /// EVIDENCE rather than by spelling: every one of the six rows that
    /// mattered on that host — `com.stado.agent.charless-mac-mini`, three
    /// `ai.wisent.oko.*` agents and two `actions.runner.*` runners — has its
    /// plist in `~/Library/LaunchAgents` or `/Library/LaunchDaemons`, and not
    /// one `application.com.apple.*` row does.
    pub fn fleet_affiliated(&self) -> bool {
        !self.declaring_paths.is_empty()
            || (!self.running_program.is_empty()
                && product_guess(&self.running_program) != UNKNOWN_PRODUCT)
    }

    /// The sentence a report should use about this row, once the registry has
    /// been asked whether it declares the label.
    ///
    /// `declared` is the registry's own unit. `undeclared` is a label the
    /// registry does not declare that is spelled like one of ours — a
    /// duplicate agent, a superseded convention, a unit somebody bootstrapped
    /// by hand. `outside-fleet-prefix` is a label the registry does not declare
    /// and did not name either, yet which this host ties to the fleet anyway:
    /// the class that used to be invisible, and the one the janitor's writer
    /// was in. `unaffiliated` is a loaded job with no tie to this fleet at all
    /// — the platform's own agents.
    ///
    /// All four are enumerated and counted. The class chooses the sentence and
    /// the order rows are printed in, never whether the host was asked.
    pub fn classification(&self) -> &'static str {
        match (
            self.declared,
            self.in_fleet_prefix(),
            self.fleet_affiliated(),
        ) {
            (true, _, _) => "declared",
            (false, true, _) => "undeclared",
            (false, false, true) => "outside-fleet-prefix",
            (false, false, false) => "unaffiliated",
        }
    }

    /// Is this a row an operator has to act on? `declared` is the answer the
    /// document promised; `unaffiliated` is somebody else's job on the same
    /// machine. What is left is what this fleet put there and cannot account
    /// for.
    pub fn accounted_for(&self) -> bool {
        self.declared || self.classification() == "unaffiliated"
    }

    /// The first word of an argument vector: the program, without its flags.
    fn head(vector: &str) -> Option<&str> {
        vector.split_whitespace().next()
    }

    /// `plutil -extract ... json` escapes every path separator, so a declared
    /// program arrives as `\/Users\/charles\/...`. Comparing that against a
    /// process table entry is comparing two spellings of the same path.
    fn unescape(vector: &str) -> String {
        vector.replace("\\/", "/")
    }

    /// The first argument after `binary` that is not a flag: the subcommand.
    ///
    /// Anchored on the binary rather than on argv[0], because an interpreter
    /// is a legitimate argv[0]: `python3 .../uvicorn app.main:app` declares
    /// `uvicorn` as its program and the process table shows the interpreter
    /// first. Reading position 1 there compares `app.main:app` against the
    /// path of uvicorn itself and calls three healthy services broken.
    fn subcommand<'a>(vector: &'a str, binary: &str) -> Option<&'a str> {
        let mut words = vector.split_whitespace().skip_while(|word| *word != binary);
        words.next()?;
        words.find(|word| !word.starts_with('-'))
    }

    /// Is the label's live process executing the program its own unit file
    /// declares?
    ///
    /// `None` wherever the answer would be a guess, which is most of a real
    /// host: a label with no pid, an unreadable unit file, and — deliberately
    /// — every case where the declared binary does not appear in the running
    /// argv at all. That last one is the launcher shape, and it is legitimate
    /// and everywhere: `launch-mac.sh` execs `node`, a `.venv/bin/uvicorn`
    /// declaration runs as `/opt/homebrew/.../Python`, `mac-mini-web-launch.sh`
    /// becomes `npm start`. An exec chain and a wrong program are
    /// indistinguishable from the outside, so this check says nothing there
    /// rather than reporting fourteen healthy services to bury one real
    /// finding.
    ///
    /// What it does answer is the one case where the evidence is complete: the
    /// process IS running the declared binary, and the subcommand differs.
    /// Every stado unit on a host executes the same binary, so the subcommand
    /// is the entire difference between the coordinator, the agent and a
    /// dashboard the product deleted in August.
    pub fn runs_declared_program(&self) -> Option<bool> {
        if self.running_program.is_empty() || self.program.is_empty() {
            return None;
        }
        let declared = Self::unescape(&self.program);
        let binary = Self::head(&declared)?;
        if !self
            .running_program
            .split_whitespace()
            .any(|word| word == binary)
        {
            return None;
        }
        match (
            Self::subcommand(&declared, binary),
            Self::subcommand(&self.running_program, binary),
        ) {
            (Some(declared_word), Some(running_word)) => Some(declared_word == running_word),
            (None, None) => Some(true),
            _ => None,
        }
    }

    /// Is it executing the binary that is on disk now? `None` when either
    /// timestamp is missing, for the same reason.
    pub fn runs_current_binary(&self) -> Option<bool> {
        let started = self.started_epoch?;
        let written = self.binary_written_epoch?;
        Some(written <= started)
    }

    /// The binary the live process is executing, for a report that has to name
    /// it.
    pub fn running_binary(&self) -> Option<&str> {
        Self::head(&self.running_program)
    }

    /// The program the unit file declares, in the spelling a process table
    /// uses, so a report can print the two side by side.
    pub fn declared_program(&self) -> String {
        Self::unescape(&self.program)
    }

    /// How long after this process started its binary was replaced, in
    /// seconds. `None` unless both facts were read.
    pub fn binary_written_after_start(&self) -> Option<i64> {
        Some(self.binary_written_epoch? - self.started_epoch?)
    }
}
