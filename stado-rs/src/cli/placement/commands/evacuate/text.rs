//! What `stado placement evacuate` prints without `--json`: one line for
//! each thing it moved, would move, prepared or left on the host, from the
//! same answers the JSON report carries.

use serde_json::Value;

use super::fixed::Profiled;
use super::scan::listed;
use crate::placement::PlacementProfile;

/// Everything one evacuation answered, as the text report reads it.
pub(super) struct Said<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub check: bool,
    pub portable: &'a [String],
    pub unstaged: &'a [&'a PlacementProfile],
    pub vaulted: &'a [PlacementProfile],
    pub profiles: &'a [PlacementProfile],
    pub roles: &'a [String],
    pub authority: bool,
    pub authority_report: &'a Value,
    pub repin_schedules: bool,
    pub schedules: &'a [String],
    pub drop_release_targets: bool,
    pub release_drops: &'a [String],
    pub profiled: &'a Profiled,
    pub routes: &'a [String],
    pub untold: &'a [String],
}

pub(super) fn print(said: &Said<'_>) {
    let Said {
        from, to, check, ..
    } = *said;
    let verb = if check {
        "would move (checked; nothing claimed or changed)"
    } else {
        "moved"
    };
    let stood = if check { "would prepare" } else { "prepared" };
    for product in said.portable {
        println!("{} {product}'s install root under {{home}} first, so {to} installs it under its own home", if check { "would write" } else { "wrote" });
    }
    for profile in said.unstaged {
        println!(
            "{stood} {} for {to} first (its catalog stores, placement standby)",
            profile.name
        );
    }
    for profile in said.vaulted {
        println!(
            "{verb} {} with the fleet vault from {from} to {to}",
            profile.name
        );
    }
    for profile in said.profiles {
        println!("{verb} {} from {from} to {to}", profile.name);
    }
    for role in said.roles {
        println!("{verb} {role} (a role of the host Stado process) from {from} to {to}");
    }
    if said.authority {
        println!(
            "{}",
            if said.authority_report["status"] == "already_authority" {
                format!("{to} already holds the registry authority")
            } else {
                format!("{verb} the registry authority from {from} to {to}")
            }
        );
    }
    if said.repin_schedules {
        println!(
            "{} pinned from {from} to {to}: {}",
            if check {
                "would repin the schedules"
            } else {
                "repinned the schedules"
            },
            listed(said.schedules)
        );
    }
    if said.drop_release_targets {
        println!(
            "{} to {from}: {}",
            if check {
                "would stop releasing"
            } else {
                "stopped releasing"
            },
            listed(said.release_drops)
        );
    }
    for drafted in &said.profiled.drafted {
        let verb = if check { "would give" } else { "gave" };
        let service = crate::cli::reporting::field(&drafted["service"]);
        println!("{verb} fixed route {service} a placement profile of its own first");
    }
    for stays in &said.profiled.stays {
        let (service, refused) = (
            crate::cli::reporting::field(&stays["service"]),
            crate::cli::reporting::field(&stays["refused"]),
        );
        println!("{service} stays on {from}: the profile draft refused it: {refused}");
    }
    for service in said
        .routes
        .iter()
        .filter(|route| !said.profiled.covers(route))
    {
        println!("{service} stays on {from}: it is a fixed route with no placement profile; give it one with stado placement profile draft, or evacuate with --profile-fixed-routes");
    }
    for untold in said.untold {
        println!("placement cannot be told, so it is not evacuated: {untold}");
    }
    if !check {
        println!(
            "the registry places no profile and no role of the host Stado process on {from} now"
        );
    }
}
