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
    /// Executable selected by `Program` or `ExecStart`.
    pub program: String,
    /// Complete launchd argument vector or the first systemd `ExecStart`.
    ///
    /// The whole vector and not just the program, because that is what
    /// launchd execs and therefore what a process table shows: every stado
    /// unit on a host runs the same binary, so `argv[0]` alone cannot tell
    /// the coordinator's process from the agent's from the janitor's, and
    /// [`units_running_replaced_images`] joins on the vector for exactly
    /// that reason.
    pub arguments: Vec<String>,
    /// Native start-command count; a resident migration requires exactly one.
    pub start_commands: usize,
    /// References remain explicit until a caller resolves their effective values.
    pub environment_files: Vec<String>,
    /// Native substitutions whose effective values have not been captured.
    pub unresolved_expansions: Vec<String>,
    /// A launchd StartInterval becomes an in-process component schedule.
    pub start_interval_seconds: Option<std::num::NonZeroU64>,
}

/// Read one unit file off the local filesystem, when it is there to read.
///
/// Every failure — absent, unreadable, a binary plist, unparsable — is the
/// same `None`: the caller's sentence then states that this host's unit was
/// not read, which is true of all of them.
pub fn local_unit_file(path: &str, kind: &str) -> Option<LocalUnitFile> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_local_unit_file(&text, kind).ok()
}

/// Parse a captured native definition without discarding the failure cause.
pub fn parse_local_unit_file(text: &str, kind: &str) -> Result<LocalUnitFile, DeployError> {
    if kind == KIND_LAUNCHD {
        let document = parse_plist(text)?;
        let program = plist_program(&document)?.unwrap_or_default().to_string();
        let start_interval_seconds = document
            .get("StartInterval")
            .map(|value| {
                value
                    .as_unsigned_integer()
                    .and_then(std::num::NonZeroU64::new)
                    .ok_or_else(|| {
                        DeployError("StartInterval must be a positive integer".to_string())
                    })
            })
            .transpose()?;
        let mut arguments = match document.get("ProgramArguments") {
            Some(value) => value
                .as_array()
                .ok_or_else(|| DeployError("ProgramArguments is not an array".to_string()))?
                .iter()
                .map(|value| {
                    value.as_string().map(str::to_string).ok_or_else(|| {
                        DeployError("ProgramArguments contains a non-string argument".to_string())
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            None => Vec::new(),
        };
        if arguments.is_empty() && !program.is_empty() {
            arguments.push(program.clone());
        }
        let env: BTreeMap<String, String> = plist_env(&document)?.into_iter().collect();
        Ok(LocalUnitFile {
            carries: env.keys().cloned().collect(),
            env,
            start_commands: usize::from(!program.is_empty()),
            program,
            arguments,
            environment_files: Vec::new(),
            unresolved_expansions: Vec::new(),
            start_interval_seconds,
        })
    } else if kind == KIND_SYSTEMD {
        let parsed = parse_systemd_unit(text)?;
        let start_commands = parsed.exec_start.len();
        let arguments = parsed.exec_start.into_iter().next().unwrap_or_default();
        let program = arguments
            .first()
            .map(|argument| {
                argument
                    .trim_start_matches(['@', '-', ':', '+', '!', '|'])
                    .to_string()
            })
            .unwrap_or_default();
        Ok(LocalUnitFile {
            carries: parsed.env.iter().map(|(name, _)| name.clone()).collect(),
            env: parsed.env.into_iter().collect(),
            program,
            arguments,
            start_commands,
            environment_files: parsed.environment_files,
            unresolved_expansions: parsed.unresolved_expansions,
            start_interval_seconds: None,
        })
    } else {
        Err(DeployError(format!("unsupported native unit kind: {kind}")))
    }
}
