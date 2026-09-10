use crate::deploy::service::*;

/// One `stado` a shell on the host could find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StadoCopy {
    /// The location probed.
    pub path: String,
    /// The version it reports, empty when it could not be run.
    pub version: String,
    /// `path` with every symlink followed.
    pub real: String,
}

/// Which `stado` binaries one host carries, and which one was delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBinary {
    /// The path the release channel delivers to.
    pub delivered: String,
    pub delivered_version: String,
    /// `delivered` with every symlink followed, empty when it is not there.
    pub delivered_real: String,
    /// Every location probed that exists.
    pub candidates: Vec<StadoCopy>,
}

impl PathBinary {
    /// The copies that are NOT the delivered binary.
    ///
    /// Compared by resolved real path, so a symlink from `~/.local/bin/stado`
    /// to the delivered file is the same binary and not a finding. A version
    /// match alone would not do: two builds of one version are not one file.
    pub fn shadows(&self) -> Vec<&StadoCopy> {
        if self.delivered_real.is_empty() {
            return Vec::new();
        }
        self.candidates
            .iter()
            .filter(|copy| !copy.real.is_empty() && copy.real != self.delivered_real)
            .collect()
    }

    /// Could this be judged at all?
    ///
    /// A host whose delivered binary could not be read, or where no location
    /// answered, is UNMEASURED and must not be reported as agreeing. The first
    /// version of this check called an empty answer clean, which is the
    /// false-negative shape this whole module exists to refuse.
    pub fn measurable(&self) -> bool {
        !self.delivered_real.is_empty() && !self.candidates.is_empty()
    }
}

/// [`LOADED_LABELS_SCRIPT`] once, for both of the things it answers: the
/// loaded units, and which `stado` the host's own PATH resolves.
///
/// One read, because both facts come out of one script and a sweep that asked
/// twice would pay two SSH round trips for one question.
pub async fn loaded_units_with_posture(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<(Vec<UndeclaredUnit>, Option<PathBinary>), DeployError> {
    read_loaded_units(target, runner, LOADED_LABELS_SCRIPT).await
}

/// Read every loaded label, domain, declaration and running command needed for
/// image reconciliation, without unrelated environment/script/PATH diagnostics.
pub async fn loaded_image_units(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<UndeclaredUnit>, DeployError> {
    let script = LOADED_LABELS_SCRIPT.replacen("details=full", "details=images", 1);
    Ok(read_loaded_units(target, runner, &script).await?.0)
}

async fn read_loaded_units(
    target: &ComputeTarget,
    runner: &Runner,
    script: &str,
) -> Result<(Vec<UndeclaredUnit>, Option<PathBinary>), DeployError> {
    let output = host_channel::run_script(target, script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the loaded-unit scan did not complete",
        )));
    }
    let declared: std::collections::BTreeSet<String> = declared_services(target)
        .iter()
        .map(|service| service.unit_id().to_string())
        .collect();
    let units: Vec<UndeclaredUnit> = output
        .stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            [
                "STADO_LOADED",
                pid,
                status,
                label,
                path,
                program,
                domains,
                running,
                started,
                written,
                path_source,
                loaded_domains,
                runs,
                last_exit,
                env_keys,
                script_reads,
                script_assigns,
            ] => {
                let label = (*label).trim().to_string();
                Some(UndeclaredUnit {
                    host: target.name.clone(),
                    declared: declared.contains(&label),
                    label,
                    pid: (*pid).trim().trim_matches('-').to_string(),
                    status: (*status).trim().to_string(),
                    path: (*path).trim().trim_matches('-').to_string(),
                    path_source: (*path_source).trim().trim_matches('-').to_string(),
                    program: (*program).trim().to_string(),
                    declaring_paths: (*domains)
                        .split_whitespace()
                        .filter(|path| *path != "-")
                        .map(str::to_string)
                        .collect(),
                    loaded_domains: split_marker_list(loaded_domains),
                    runs: (*runs).trim().trim_matches('-').parse().ok(),
                    last_exit: (*last_exit).trim().trim_matches('-').parse().ok(),
                    env_keys: split_marker_list(env_keys),
                    script_reads: split_marker_list(script_reads),
                    script_assigns: split_marker_list(script_assigns),
                    running_program: (*running).trim().trim_matches('-').trim().to_string(),
                    started_epoch: started.trim().parse().ok(),
                    binary_written_epoch: written.trim().parse().ok(),
                })
            }
            _ => None,
        })
        .collect();
    let mut posture: Option<PathBinary> = None;
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_PATH_DELIVERED", delivered, version, real] => {
                posture = Some(PathBinary {
                    delivered: undash(delivered),
                    delivered_version: undash(version),
                    delivered_real: undash(real),
                    candidates: Vec::new(),
                });
            }
            ["STADO_PATH_CANDIDATE", path, version, real] => {
                if let Some(posture) = posture.as_mut() {
                    let copy = StadoCopy {
                        path: undash(path),
                        version: undash(version),
                        real: undash(real),
                    };
                    // `command -v` and an explicit location can name the same
                    // file; one copy is one finding, not two.
                    if !posture
                        .candidates
                        .iter()
                        .any(|seen| seen.real == copy.real && !copy.real.is_empty())
                    {
                        posture.candidates.push(copy);
                    }
                }
            }
            _ => {}
        }
    }
    Ok((units, posture))
}
