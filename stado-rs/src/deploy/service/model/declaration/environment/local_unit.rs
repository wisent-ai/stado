use crate::deploy::service::*;

/// The unit file on THIS machine, as the machine holds it.
///
/// A plain local read and never ssh, which is what makes it usable from
/// `registry doctor`: that command answers for the whole fleet out of the
/// store, and the one host whose unit files it may open is the one it is
/// running on. A unit on another host yields `None`, and the finding says
/// the read did not happen instead of reporting an empty environment —
/// "nothing was read" and "the unit carries nothing" are different facts,
/// and collapsing them is the exact defect this check exists to catch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUnitFile {
    /// Variable names the unit file carries, in file order. Names only: the
    /// finding is about which variables reach the unit, and a diagnostic has
    /// no business printing the values of the ones that do.
    pub carries: Vec<String>,
    /// Values retained only for comparison; diagnostic sentences print names.
    pub env: BTreeMap<String, String>,
    /// The first [`LocalUnitFile::arguments`] entry: the file the unit
    /// declares it starts. Empty for a systemd unit, whose `ExecStart` this
    /// reader does not parse.
    pub program: String,
    /// The whole `ProgramArguments` vector, empty for a systemd unit.
    ///
    /// The whole vector and not just the program, because that is what
    /// launchd execs and therefore what a process table shows: every stado
    /// unit on a host runs the same binary, so `argv[0]` alone cannot tell
    /// the coordinator's process from the agent's from the janitor's, and
    /// [`units_running_replaced_images`] joins on the vector for exactly
    /// that reason.
    pub arguments: Vec<String>,
}

/// Read one unit file off the local filesystem, when it is there to read.
///
/// Every failure — absent, unreadable, a binary plist, unparsable — is the
/// same `None`: the caller's sentence then states that this host's unit was
/// not read, which is true of all of them.
pub fn local_unit_file(path: &str, kind: &str) -> Option<LocalUnitFile> {
    let text = std::fs::read_to_string(path).ok()?;
    if kind == KIND_LAUNCHD {
        let document = parse_plist(&text).ok()?;
        // `Program` as well as `ProgramArguments`: a plist may carry either,
        // and `self_update::launchd_argv` already falls back the same way.
        let arguments: Vec<String> = document
            .get("ProgramArguments")
            .and_then(Value::as_array)
            .map(|argv| {
                argv.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .filter(|argv: &Vec<String>| !argv.is_empty())
            .or_else(|| {
                document
                    .get("Program")
                    .and_then(Value::as_str)
                    .map(|program| vec![program.to_string()])
            })
            .unwrap_or_default();
        let env: BTreeMap<String, String> = plist_env(&document).into_iter().collect();
        Some(LocalUnitFile {
            carries: env.keys().cloned().collect(),
            env,
            program: arguments.first().cloned().unwrap_or_default(),
            arguments,
        })
    } else {
        let parsed = parse_systemd_unit(&text);
        Some(LocalUnitFile {
            carries: parsed.env.iter().map(|(name, _)| name.clone()).collect(),
            env: parsed.env.into_iter().collect(),
            program: String::new(),
            arguments: Vec::new(),
        })
    }
}
