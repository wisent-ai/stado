use crate::deploy::service::*;

/// One product process on one host that no unit owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnownedProcess {
    pub host: String,
    pub pid: String,
    /// The full command line, tabs and newlines flattened on the host so one
    /// process can never span two marker lines.
    pub command: String,
    /// The host's own `ps` start stamp. Kept verbatim: four days is the fact
    /// that mattered on the always-on mac, and a reformatting that failed
    /// would report a process with no age at all.
    pub started_at: String,
}

impl UnownedProcess {
    pub fn product_guess(&self) -> String {
        product_guess(&self.command)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "pid": self.pid,
            "command": self.command,
            "started_at": self.started_at,
            "product_guess": self.product_guess(),
        })
    }
}

/// What one host's unowned-process scan searched, beside what it found.
///
/// The result alone could not be read. An empty `processes` meant either that
/// the host runs nothing unowned or that every root expanded to a path no
/// process could run out of, and those need opposite responses. This carries
/// the roots as the host expanded them, how many pids each one matched, how
/// many of those actually execute out of it, and how many pids launchd claimed
/// — so an empty answer states why it is empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnownedScan {
    pub processes: Vec<UnownedProcess>,
    /// `(root, pids matched by pgrep, pids executing out of the root)`.
    pub roots: Vec<(String, usize, usize)>,
    /// Pids launchd reported as owned across every printable domain.
    pub owned_pids: usize,
    /// `(pid, "owned"|"unowned", the ancestor pid launchd claimed)` for every
    /// candidate that executes out of a managed root. The verdict without its
    /// evidence is what made an empty table unreadable.
    pub judged: Vec<(String, String, String)>,
}

impl UnownedScan {
    /// One line an operator can read beside an empty table.
    pub fn account(&self, host: &str) -> String {
        let roots = self
            .roots
            .iter()
            .map(|(root, matched, under)| format!("{root} matched {matched}, under {under}"))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "{host}: launchd claimed {} pid(s); {}",
            self.owned_pids,
            if roots.is_empty() {
                "no root was searched".to_string()
            } else {
                roots
            }
        )
    }
}

/// Every product process on one host that no launchd job or systemd unit owns,
/// with an account of what was searched to find them.
///
/// Read-only: it starts nothing, stops nothing and signals nothing, so it is
/// safe to run against a live production host.
pub async fn unowned_processes(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<UnownedScan, DeployError> {
    let mut roots = Vec::new();
    for root in managed_roots()? {
        // The roots keep `$HOME` unexpanded on this side and expanded on
        // theirs, so the same rule `quote_unit_path` applies to a declared
        // unit path applies to them: a vetted charset inside double quotes.
        roots.push(format!("\"{}\"", quote_unit_path(&root)?));
    }
    let script = UNOWNED_SCRIPT.replace("@ROOTS@", &roots.join(" "));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the unowned-process scan did not complete",
        )));
    }
    let mut scan = UnownedScan {
        processes: parse_unowned(&target.name, &output.stdout),
        ..Default::default()
    };
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_UNOWNED_OWNED", count] => {
                scan.owned_pids = count.trim().parse().unwrap_or_default();
            }
            ["STADO_UNOWNED_ROOT", root, matched, under] => scan.roots.push((
                (*root).trim().to_string(),
                matched.trim().parse().unwrap_or_default(),
                under.trim().parse().unwrap_or_default(),
            )),
            ["STADO_UNOWNED_JUDGED", pid, verdict, owner] => scan.judged.push((
                (*pid).trim().to_string(),
                (*verdict).trim().to_string(),
                (*owner).trim().to_string(),
            )),
            _ => {}
        }
    }
    Ok(scan)
}

/// The `STADO_UNOWNED` marker lines, in the order the host printed them.
fn parse_unowned(host: &str, stdout: &str) -> Vec<UnownedProcess> {
    stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_UNOWNED", pid, started, command] => Some(UnownedProcess {
                host: host.to_string(),
                pid: (*pid).to_string(),
                command: (*command).trim().to_string(),
                started_at: (*started).trim().to_string(),
            }),
            _ => None,
        })
        .collect()
}
