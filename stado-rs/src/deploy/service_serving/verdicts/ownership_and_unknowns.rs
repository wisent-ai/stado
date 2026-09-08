//! What defends the verdicts: that a port a different launchd job holds is
//! `not_serving` and names that job, that identical argv under another label
//! never counts as this unit, that an owner nobody could read is neither
//! served nor stolen, and that an unreadable socket table is not a dead port.

use super::super::*;
use super::belongs_to_unit;

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(pid: &str, owner: &str, state: &str) -> Holder {
        Holder {
            pid: pid.to_string(),
            comm: "/opt/homebrew/bin/node".to_string(),
            owner: owner.to_string(),
            owner_state: state.to_string(),
        }
    }

    fn report(launchd_pid: &str, ports: Vec<PortReport>) -> ServingReport {
        ServingReport {
            unit: "com.wisent.always-on.weles".to_string(),
            unit_path: "/Library/LaunchDaemons/com.wisent.always-on.weles.plist".to_string(),
            loaded: "yes".to_string(),
            launchd_pid: launchd_pid.to_string(),
            listeners_state: LISTENERS_READ.to_string(),
            ports,
        }
    }

    #[test]
    fn a_port_held_by_another_job_is_not_serving_and_names_that_job() {
        // The exact 2026-08-30 state: the declared unit is dead and the
        // undeclared unit the release deployer bootstraps holds 58101.
        let subject = report(
            "",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("57910", "com.wisent.weles-worker", OWNER_RESOLVED)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_SERVED_BY_OTHER);
        assert_eq!(verdict(&subject, &ports), SERVING_NO);
        let said = failure("charless-mac-mini", &subject, &ports).unwrap();
        assert!(said.contains("is not serving"), "{said}");
        assert!(said.contains("57910"), "{said}");
        assert!(said.contains("com.wisent.weles-worker"), "{said}");
    }

    #[test]
    fn identical_argv_under_another_label_never_counts_as_this_unit() {
        // Both units run the same program with the same arguments. Only the
        // label decides, so the pid must not be credited to the unit asked
        // about just because the pid launchd recorded is unknown here.
        let subject = report(
            "",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("57910", "com.wisent.weles-worker", OWNER_RESOLVED)],
            }],
        );
        assert!(!belongs_to_unit(
            &subject.ports[0].holders[0],
            &subject.unit,
            &subject.launchd_pid
        ));
    }

    #[test]
    fn the_units_own_process_is_serving() {
        let subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("4242", "com.wisent.always-on.weles", OWNER_RESOLVED)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_SERVED_BY_UNIT);
        assert_eq!(verdict(&subject, &ports), SERVING_YES);
        assert_eq!(failure("h", &subject, &ports), None);
    }

    #[test]
    fn an_unresolvable_owner_is_unknown_and_never_someone_elses_port() {
        // A system LaunchDaemon is invisible to an unprivileged `launchctl
        // list`. Calling that "held by another job" would report every working
        // daemon as broken; calling it `serving` would repeat the original
        // defect. It is neither.
        let subject = report(
            "",
            vec![PortReport {
                port: 8788,
                holders: vec![holder("7438", "", OWNER_UNKNOWN)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_OWNER_UNKNOWN);
        assert_eq!(verdict(&subject, &ports), SERVING_UNKNOWN);
        let said = failure("h", &subject, &ports).unwrap();
        assert!(said.contains("could not be established"), "{said}");
    }

    #[test]
    fn a_dead_port_is_not_serving_and_says_which_one() {
        let subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: Vec::new(),
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_DEAD);
        assert_eq!(verdict(&subject, &ports), SERVING_NO);
        assert!(failure("h", &subject, &ports)
            .unwrap()
            .contains("nothing is listening on 58101"));
    }

    #[test]
    fn an_unreadable_socket_table_is_unknown_not_dead() {
        let mut subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: Vec::new(),
            }],
        );
        subject.listeners_state = LISTENERS_FAILED.to_string();
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_UNKNOWN);
        assert_eq!(verdict(&subject, &ports), SERVING_UNKNOWN);
        assert!(failure("h", &subject, &ports)
            .unwrap()
            .contains("socket table could not be read"));
    }
}
