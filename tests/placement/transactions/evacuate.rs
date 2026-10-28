//! What `stado placement evacuate` answers on the journey's registry before
//! any move is open: refused whole when one profile's move would be, refused
//! by standby's own check with `--standby`, refused for a host with nothing
//! on it unless schedules are handed over, and refused for one host twice.

use serde_json::Value;

use super::DESTINATION;
use crate::deployment::{Deployment, PLACED_ON, PROFILE, SERVICE};

pub(super) fn refusals(deployment: &Deployment) {
    // An evacuation checks every profile first: the same refusal refuses it
    // whole, with and without --check, before anything is claimed.
    for check in [true, false] {
        let mut evacuate = vec![
            "placement",
            "evacuate",
            "--from-host",
            PLACED_ON,
            "--to-host",
            DESTINATION,
            "--reason",
            "journey",
        ];
        if check {
            evacuate.push("--check");
        }
        deployment.refused(
            &evacuate,
            &format!(
                "evacuating {PLACED_ON} to {DESTINATION} would be refused, so nothing was moved"
            ),
        );
    }
    // With --standby that profile is one the run would stand the destination
    // by for first, so standby's own check runs, and its refusal (the test
    // service is no catalog product, so nothing could deliver it) refuses
    // the evacuation whole, as `placement standby --check` refuses itself.
    let uncatalogued = format!(
        "the Wisent service catalog this build ships declares no service named {SERVICE:?}"
    );
    deployment.refused(
        &[
            "placement",
            "evacuate",
            "--from-host",
            PLACED_ON,
            "--to-host",
            DESTINATION,
            "--reason",
            "journey",
            "--standby",
            "--check",
        ],
        &format!("{PROFILE}: standing {DESTINATION} by: {uncatalogued}"),
    );
    deployment.refused(
        &[
            "placement",
            "standby",
            PROFILE,
            "--host",
            DESTINATION,
            "--reason",
            "journey",
            "--check",
        ],
        &uncatalogued,
    );
    deployment.refused(
        &[
            "placement",
            "evacuate",
            "--from-host",
            DESTINATION,
            "--to-host",
            PLACED_ON,
            "--reason",
            "journey",
        ],
        &format!(
            "no placement profile and no role of the host Stado process is placed on {DESTINATION}"
        ),
    );
    // Handing over an emptied host's schedules is still an evacuation: the
    // check is not refused as having nothing to move, and its report says
    // which schedules it would repin (or the store's refusal names itself).
    let handover = deployment.run(&[
        "placement",
        "evacuate",
        "--from-host",
        DESTINATION,
        "--to-host",
        PLACED_ON,
        "--reason",
        "journey",
        "--repin-schedules",
        "--check",
        "--json",
    ]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&handover.stdout),
        String::from_utf8_lossy(&handover.stderr)
    );
    assert!(!said.contains("no placement profile and no role"), "{said}");
    match handover.status.success() {
        true => {
            let report: Value = serde_json::from_slice(&handover.stdout).unwrap();
            assert_eq!(report["status"], "would_evacuate", "{report}");
            assert!(report["repinned_schedules"].is_array(), "{report}");
        }
        false => assert!(
            said.contains("--repin-schedules: the schedules pinned to"),
            "{said}"
        ),
    }
    deployment.refused(
        &[
            "placement",
            "evacuate",
            "--from-host",
            PLACED_ON,
            "--to-host",
            PLACED_ON,
            "--reason",
            "journey",
        ],
        &format!("--from-host and --to-host both name {PLACED_ON}"),
    );
}
