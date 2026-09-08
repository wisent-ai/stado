use crate::deploy::service::*;

/// Every launchd unit on THIS machine that this fleet is answerable for,
/// keyed by label and valued by the unit file to read.
///
/// Two sources, because either alone has a blind spot this check cannot
/// afford. The registry's own `services` array is the declared set — and
/// `com.wisent.compute.disk-cleanup.disk-cleanup`, the unit the whole incident
/// happened to, is not in it on `lukasz-macbook`. The three unit directories
/// carry every label this fleet installed whether the document adopted it or
/// not, which is the class [`UndeclaredUnit::fleet_affiliated`] was widened to
/// see, and they miss a declared unit whose file has been deleted. The union
/// is what the fleet is answerable for.
pub(super) fn local_launchd_units(target: &ComputeTarget, home: &str) -> BTreeMap<String, String> {
    let mut units: BTreeMap<String, String> = BTreeMap::new();
    for service in declared_services(target) {
        if service.kind != KIND_LAUNCHD || service.path.is_empty() {
            continue;
        }
        units.insert(
            service.unit_id().to_string(),
            service.path.replace("$HOME", home),
        );
    }
    for directory in LAUNCHD_UNIT_DIRECTORIES {
        let Ok(entries) = std::fs::read_dir(PathBuf::from(directory.replace("$HOME", home))) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // Exactly `<label>.plist`. The disabled, retired and dated
            // siblings beside them — `.plist.stado-disabled`,
            // `.plist.retired-20260818` — are not units launchd loads, and
            // reporting on them would be reporting on files nobody runs.
            let Some(label) = name.strip_suffix(".plist") else {
                continue;
            };
            if !label.starts_with(FLEET_LABEL_PREFIX) {
                continue;
            }
            units
                .entry(label.to_string())
                .or_insert_with(|| entry.path().to_string_lossy().into_owned());
        }
    }
    units
}

/// Whether a running image is a finding, given the file the unit declares.
///
/// `None` for the two states that are not: the same file, and a replacement
/// young enough to still be mid-flight. Pure, so the boundary can be exercised
/// without a process to point it at.
pub fn classify_image(
    running: &ImageIdentity,
    installed: &ImageIdentity,
    installed_age_seconds: i64,
) -> Option<ImageState> {
    if running.is_same_file(installed) || installed_age_seconds < IMAGE_SETTLE_SECONDS {
        return None;
    }
    let (running, installed) = (running.clone(), installed.clone());
    if running.links == 0 {
        return Some(ImageState::Unlinked { running, installed });
    }
    Some(ImageState::Replaced { running, installed })
}

/// One managed unit's image, as read on the machine holding its process —
/// including the units that turned out to be fine.
///
/// Two callers need this and they must never disagree: `registry doctor`
/// reports the units that are stale, and `service refresh-image` refuses to
/// act on a unit that is not. A refusal has to name the identity it found, so
/// the clean answer is a value here rather than an absence, and the finding is
/// derived from it by [`UnitImageObservation::finding`] instead of being
/// produced by a second pass that could drift from the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitImageObservation {
    pub host: String,
    /// launchd label. Empty on the observation that covers a whole host.
    pub unit: String,
    pub unit_path: String,
    /// `ProgramArguments[0]`: the file the unit says it starts.
    pub program: String,
    pub pid: Option<u32>,
    pub process_age_seconds: Option<i64>,
    pub installed_age_seconds: Option<i64>,
    /// The image the live process is executing, when it was read.
    pub running: Option<ImageIdentity>,
    /// The file the unit declares, as it stands now.
    pub installed: Option<ImageIdentity>,
    /// `None` when the process is executing the file the unit declares, or
    /// when the replacement is still inside [`IMAGE_SETTLE_SECONDS`].
    pub state: Option<ImageState>,
}

impl UnitImageObservation {
    /// The `registry doctor` row for this observation, or `None` when there is
    /// nothing to report.
    pub fn finding(&self) -> Option<StaleUnitImage> {
        Some(StaleUnitImage {
            host: self.host.clone(),
            unit: self.unit.clone(),
            unit_path: self.unit_path.clone(),
            program: self.program.clone(),
            pid: self.pid,
            process_age_seconds: self.process_age_seconds,
            installed_age_seconds: self.installed_age_seconds,
            state: self.state.clone()?,
        })
    }

    /// Whether the running image and the declared file are the same file.
    ///
    /// `None` while either identity is unread, which is the answer this whole
    /// module exists to keep apart from `true`.
    pub fn agrees(&self) -> Option<bool> {
        Some(
            self.running
                .as_ref()?
                .is_same_file(self.installed.as_ref()?),
        )
    }
}

/// One row from the unit-image scan plus the native owner's observed argv.
///
/// The public observation predates the release revisit pass and remains the
/// stable value consumed by doctor and the manual refresh command. The
/// release pass additionally needs the subcommand to exclude units that
/// recycle themselves. Keeping it beside the observation internally preserves
/// that evidence from the same native process observation without widening the
/// public struct or reading either source again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitImageScan {
    pub observation: UnitImageObservation,
    pub arguments: Vec<String>,
}
