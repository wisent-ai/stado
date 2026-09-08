use super::classify::local_launchd_units;
use crate::deploy::service::*;

/// Every managed unit on one host with the image its live process is
/// executing.
///
/// LOCAL ONLY, and the signature says so. Which image a pid is executing is a
/// question only the kernel holding that pid can answer: nothing in the store
/// carries it, the beacon publishes one `state` word per unit, and
/// [`ManagedService`] records a path rather than an identity. So `local_units`
/// is the name of the host this process is running on, exactly as
/// [`unreachable_product_environments`] uses it, and every other host gets one
/// observation saying its units were not measured. That row is deliberate: the
/// state this check exists to remove is an unread one rendered as passing, and
/// a remote host silently omitted would be that same defect wearing this
/// check's name.
///
/// The native manager supplies each label's live PID. The unit file supplies
/// the installed program to compare with that PID's kernel image; its argv may
/// already differ from launchd's cached definition. An unloaded or stopped
/// unit holds no image, while ambiguous ownership remains explicitly unread.
pub(crate) async fn observe_unit_image_scan(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<UnitImageScan> {
    let blank = |unit: &str, unit_path: &str, state: Option<ImageState>| UnitImageScan {
        observation: UnitImageObservation {
            host: target.name.clone(),
            unit: unit.to_string(),
            unit_path: unit_path.to_string(),
            program: String::new(),
            pid: None,
            process_age_seconds: None,
            installed_age_seconds: None,
            running: None,
            installed: None,
            state,
        },
        // No argv: the native owner or declared program could not be read.
        arguments: Vec::new(),
    };
    let whole_host = |reason: String| {
        blank(
            "",
            "",
            Some(ImageState::Unread {
                subject: format!("the executing image of every unit on {}", target.name),
                reason,
            }),
        )
    };
    if local_units != Some(target.name.as_str()) {
        let declared = declared_services(target).len();
        if declared == 0 {
            return Vec::new();
        }
        return vec![whole_host(format!(
            "which image a process is executing is readable only on the machine holding that \
             process, and this command is running on {}; {declared} declared unit(s) on {} are \
             unmeasured until `stado registry doctor` runs there",
            local_units.unwrap_or("a host no registry target names"),
            target.name
        ))];
    }
    let Some(home) = std::env::var_os("HOME").map(|home| home.to_string_lossy().into_owned())
    else {
        return vec![whole_host(
            "this process has no HOME, so the launchd unit directories could not be named"
                .to_string(),
        )];
    };
    let native_units = match loaded_units(target, &crate::deploy::production_runner()).await {
        Ok(units) => units,
        Err(error) => return vec![whole_host(error.to_string())],
    };
    let native_by_label = native_units
        .iter()
        .map(|unit| (unit.label.as_str(), unit))
        .collect::<BTreeMap<_, _>>();

    // One pass over the unit files, then one image read for every pid they
    // name.
    let mut rows: Vec<UnitImageScan> = Vec::new();
    // Keep the native owner's PID and argv beside the declared executable.
    struct Matched {
        label: String,
        unit_path: String,
        program: String,
        arguments: Vec<String>,
        pid: u32,
        age: Option<i64>,
    }
    let mut pending: Vec<Matched> = Vec::new();
    for (label, unit_path) in local_launchd_units(target, &home) {
        let unread = |subject: String, reason: String| {
            blank(
                &label,
                &unit_path,
                Some(ImageState::Unread { subject, reason }),
            )
        };
        let Some(unit) = local_unit_file(&unit_path, KIND_LAUNCHD) else {
            rows.push(unread(
                format!("{label}'s unit file {unit_path}"),
                "it is absent, unreadable, or not a plist this build can parse".to_string(),
            ));
            continue;
        };
        if unit.arguments.is_empty() {
            rows.push(unread(
                format!("{label}'s declared program"),
                format!(
                    "{unit_path} carries neither ProgramArguments nor Program, so there is no \
                     declared file for a running image to be compared against"
                ),
            ));
            continue;
        }
        let Some(native) = native_by_label.get(label.as_str()) else {
            continue;
        };
        if native.loaded_domains.len() > 1 {
            rows.push(unread(
                format!("{label}'s native owner"),
                format!(
                    "launchd reports {} loaded domains; refusing to choose a process",
                    native.loaded_domains.len()
                ),
            ));
            continue;
        }
        let Ok(pid) = native.pid.parse::<u32>() else {
            continue;
        };
        if native.loaded_domains.is_empty() || native.running_program.is_empty() {
            rows.push(unread(
                format!("{label}'s native owner"),
                "a live PID has no readable owner domain or argument vector".to_string(),
            ));
            continue;
        }
        pending.push(Matched {
            label,
            unit_path,
            program: unit.program,
            arguments: native
                .running_program
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            pid,
            age: native.started_epoch.map(|started| now_epoch - started),
        });
    }

    let pids: Vec<u32> = pending.iter().map(|matched| matched.pid).collect();
    let images = match running_images(&pids) {
        Ok(images) => images,
        Err(reason) => {
            // One row, not one per pid: the cause is a reader that would not
            // answer, and it is the same cause for every process.
            rows.push(whole_host(reason));
            rows.sort_by(|left, right| left.observation.unit.cmp(&right.observation.unit));
            return rows;
        }
    };

    for matched in pending {
        let Matched {
            label,
            unit_path,
            program,
            arguments,
            pid,
            age,
        } = matched;
        let installed_read = installed_image(Path::new(&program));
        let mut scan = UnitImageScan {
            observation: UnitImageObservation {
                host: target.name.clone(),
                unit: label,
                unit_path,
                program,
                pid: Some(pid),
                process_age_seconds: age,
                installed_age_seconds: None,
                running: images.get(&pid).cloned(),
                installed: None,
                state: None,
            },
            arguments,
        };
        let row = &mut scan.observation;
        let (installed, written_epoch) = match installed_read {
            Ok(read) => read,
            Err(reason) => {
                row.state = Some(ImageState::Unread {
                    subject: format!("{}'s declared program {}", row.unit, row.program),
                    reason,
                });
                rows.push(scan);
                continue;
            }
        };
        let installed_age = now_epoch - written_epoch;
        row.installed_age_seconds = Some(installed_age);
        row.installed = Some(installed.clone());
        let Some(running) = row.running.clone() else {
            row.state = Some(ImageState::Unread {
                subject: format!("the image pid {pid} is executing for {}", row.unit),
                reason: "no text mapping was readable for that pid: it exited between the \
                         process listing and this read, or it belongs to another account"
                    .to_string(),
            });
            rows.push(scan);
            continue;
        };
        row.state = classify_image(&running, &installed, installed_age);
        rows.push(scan);
    }
    rows.sort_by(|left, right| {
        left.observation
            .unit
            .cmp(&right.observation.unit)
            .then(left.observation.pid.cmp(&right.observation.pid))
    });
    rows
}
