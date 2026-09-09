//! Routing opened and closed on a machine that is not the one running the
//! test.
//!
//! The rest of this area drives the real binary against a registry naming THIS
//! host, which proves resolution and proves the local marker, and cannot prove
//! the one thing routing exists for: putting an address into a DIFFERENT
//! account over the fleet channel, and taking it away again.
//!
//! So these cases lease one. [`lease`] mints a throwaway account through
//! `stado scratch create` on a host the fleet itself calls leasable, seeds a
//! service directory into the registry that lease emits, and declares the
//! service through `stado service declare`. `route open --remote` then writes
//! the marker into the leased account's own home.
//!
//! No assertion here rests on the opening command's own words. `stado host
//! inventory` says whether that account's forwards directory exists at all,
//! `route list` says what the directory reports about the route, and `route
//! close` probes the far side itself: it says `closed remote forward` only
//! after removing a file that was really there, and refuses with its own exact
//! sentence when a second close finds nothing. Every case destroys its lease
//! and holds the destroy to account, home and record all absent.

mod lease;

use serde_json::Value;

use super::fleet::{json_stdout, said};
use lease::{declare_on, leasable_host, lease_on, rows, text, with_host_turn, ENDPOINT, SERVICE};

/// A name the leased registry never declares, for the refusal.
const UNDECLARED: &str = "route-leased-absent";
/// What `stado host inventory` calls a home with, and without, a forwards
/// directory. Both are the leased machine's own words, not this test's.
const PRESENT: &str = "directory";
const ABSENT: &str = "missing";

/// Opening the declared forward puts the address into the leased account's own
/// home, and closing it takes that address away again.
#[test]
fn a_forward_opened_on_a_leased_target_lands_in_that_account_and_closing_removes_it() {
    with_host_turn(|| {
        let host = leasable_host();
        let mut lease = lease_on(&host);
        declare_on(&lease);
        assert_eq!(
            lease.forwards_state(),
            ABSENT,
            "the leased account carried a forwards directory before this case opened one"
        );

        let opened = lease.stado(&["route", "open", SERVICE, "--remote", "--json"]);
        assert!(
            opened.status.success(),
            "opening the declared route on the leased target failed:\n{}",
            said(&opened)
        );
        let report = json_stdout(&opened);
        assert_eq!(report["status"], "open");
        assert_eq!(report["service"], SERVICE);
        assert_eq!(report["active_host"], lease.name.as_str());
        assert_eq!(report["authority"]["target"], lease.name.as_str());
        assert_eq!(report["endpoint"], ENDPOINT);
        assert_eq!(report["forward"]["location"], "remote");
        assert_eq!(report["forward"]["url"], ENDPOINT);
        assert_eq!(
            report["forward"]["marker"].as_str(),
            Some(lease.marker().as_str()),
            "the marker is not under the account home the lease itself named"
        );

        // The leased machine's own account, through a command `route` shares no
        // code with, and then the directory's own account of the route.
        assert_eq!(
            lease.forwards_state(),
            PRESENT,
            "the leased account home holds no forwards directory, so nothing reached it"
        );
        let listed = lease.stado(&["route", "list", "--json"]);
        assert!(listed.status.success(), "{}", said(&listed));
        let catalogue = json_stdout(&listed);
        let row = rows(&catalogue, "services")
            .into_iter()
            .find(|row| row["service"] == SERVICE)
            .unwrap_or_else(|| panic!("{SERVICE} is absent from {}", said(&listed)));
        let declared = rows(&row, "endpoints")
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(row["active_host"], lease.name.as_str());
        assert_eq!(declared["target"], lease.name.as_str());
        assert_eq!(declared["url"], ENDPOINT);
        assert_eq!(
            row["local_forward"],
            Value::Null,
            "the forward opened on the leased target also wrote into the invoking account"
        );

        let closed = lease.stado(&["route", "close", SERVICE]);
        assert!(
            closed.status.success(),
            "closing the leased forward failed:\n{}",
            said(&closed)
        );
        assert!(
            said(&closed).contains(&format!(
                "{SERVICE}: closed remote forward; no marker remains"
            )),
            "close did not report removing a marker from the leased account:\n{}",
            said(&closed)
        );

        // The far side is probed once more, by the product, and answers empty.
        let again = lease.stado(&["route", "close", SERVICE]);
        assert!(
            !again.status.success(),
            "a route with no marker anywhere closed a second time:\n{}",
            said(&again)
        );
        assert!(
            said(&again).contains(&format!(
                "{SERVICE} has no open forward marker; run `stado route open {SERVICE} --local` \
                 or `stado route open {SERVICE} --remote` first"
            )),
            "the second close did not refuse with the sentence naming both directions:\n{}",
            said(&again)
        );

        lease.destroy();
    });
}

/// A name the leased registry does not declare is refused by the directory, and
/// the refusal is proved to have stopped before the account was reached.
#[test]
fn a_service_the_leased_registry_does_not_declare_is_refused_and_reaches_no_account() {
    with_host_turn(|| {
        let host = leasable_host();
        let mut lease = lease_on(&host);
        declare_on(&lease);

        let refused = lease.stado(&["route", "open", UNDECLARED, "--remote"]);
        assert!(
            !refused.status.success(),
            "opening a service the leased registry does not declare succeeded:\n{}",
            said(&refused)
        );
        assert!(
            said(&refused).contains(&format!(
                "{UNDECLARED} is not in the service directory; add it to \
                 service_directory.services"
            )),
            "the refusal did not name the declaration to edit:\n{}",
            said(&refused)
        );
        assert_eq!(
            lease.forwards_state(),
            ABSENT,
            "a refused open still created a forwards directory in the leased account"
        );

        let listed = lease.stado(&["route", "list", "--json"]);
        assert!(listed.status.success(), "{}", said(&listed));
        let catalogue = json_stdout(&listed);
        let declared: Vec<String> = rows(&catalogue, "services")
            .iter()
            .map(|row| text(row, "service"))
            .collect();
        assert_eq!(
            declared,
            vec![SERVICE.to_string()],
            "the refused open changed what the leased registry declares"
        );

        lease.destroy();
    });
}
