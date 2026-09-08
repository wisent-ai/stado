//! What the retention rules must decide, over names alone: the rollback
//! ladder, the newest deployable release of each family, and the precedence
//! between the pins.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use crate::providers::local::disk_cleanup::release_store::decision::{
        retention_decision, KeepReason,
    };
    use crate::providers::local::disk_cleanup::release_store::{family_key, ReleaseFamily};

    fn versions(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Which families each named version completes, as `complete_families`
    /// would have read them off the disk.
    fn families(rows: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<&'static str>> {
        rows.iter()
            .map(|(version, names)| {
                let set = names
                    .iter()
                    .map(|name| match *name {
                        "installer" => family_key(ReleaseFamily::Installer),
                        "signed" => family_key(ReleaseFamily::Signed),
                        other => panic!("unknown family {other}"),
                    })
                    .collect();
                (version.to_string(), set)
            })
            .collect()
    }

    /// The decision for one version, by name.
    fn verdict(decisions: &[(&str, KeepReason)], version: &str) -> KeepReason {
        decisions
            .iter()
            .find(|(name, _)| *name == version)
            .map(|(_, reason)| *reason)
            .expect("every version present is decided")
    }

    /// The 2026-09-04 shape: four newer coordinates carrying nothing but a
    /// `source-revision.json` claim, and the last version anybody can install
    /// sitting below the newest-three ladder.
    #[test]
    fn the_newest_installable_release_survives_a_ladder_full_of_claims() {
        let present = versions(&["0.15.21", "0.15.25", "0.15.26", "0.16.0", "0.16.1"]);
        let decisions = retention_decision(
            &present,
            &families(&[("0.15.21", &["installer"])]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(
            verdict(&decisions, "0.15.21"),
            Some("newest_installable_kept"),
            "the only installable release must not fall off the bottom of the ladder"
        );
        assert_eq!(verdict(&decisions, "0.16.1"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "0.16.0"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "0.15.26"), Some("newest_kept"));
        assert_eq!(
            verdict(&decisions, "0.15.25"),
            None,
            "a claim outside the ladder is still reclaimable"
        );
    }

    #[test]
    fn a_version_a_host_declares_survives_below_the_ladder() {
        let present = versions(&["0.14.6", "0.15.21", "0.16.0", "0.16.1", "0.16.2"]);
        let decisions = retention_decision(
            &present,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &set(&["0.14.6"]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(verdict(&decisions, "0.14.6"), Some("host_declares_it"));
        assert_eq!(verdict(&decisions, "0.15.21"), None);
    }

    #[test]
    fn a_version_an_operator_pins_in_config_survives_below_the_ladder() {
        let present = versions(&["0.15.3", "0.16.0", "0.16.1", "0.16.2"]);
        let decisions = retention_decision(
            &present,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &set(&["0.15.3"]),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(verdict(&decisions, "0.15.3"), Some("config_pins_it"));
    }

    /// Precedence is reported, not just obeyed: an operator reading
    /// `host_state_names_it` must be able to trust that the host's own state
    /// is why the version is still there.
    #[test]
    fn the_reported_reason_is_the_strongest_one_that_applies() {
        let present = versions(&["1.0.0"]);
        let decisions = retention_decision(
            &present,
            &families(&[("1.0.0", &["installer", "signed"])]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            &set(&["1.0.0"]),
            1,
        );
        assert_eq!(verdict(&decisions, "1.0.0"), Some("host_state_names_it"));

        let decisions = retention_decision(
            &present,
            &BTreeMap::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &set(&["1.0.0"]),
            0,
        );
        assert_eq!(verdict(&decisions, "1.0.0"), Some("pipeline_run_names_it"));
    }

    /// Only the newest installable version is kept for that reason. An
    /// installable ancestor is ordinary history: keeping every one of them
    /// would be a store that never reclaims, which is the state this cleaner
    /// exists to end.
    #[test]
    fn older_installable_versions_are_still_reclaimable() {
        let present = versions(&["0.13.0", "0.14.6", "0.15.21", "0.16.0", "0.16.1"]);
        let decisions = retention_decision(
            &present,
            &families(&[
                ("0.13.0", &["installer"]),
                ("0.14.6", &["installer"]),
                ("0.15.21", &["installer"]),
            ]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            2,
        );
        assert_eq!(
            verdict(&decisions, "0.15.21"),
            Some("newest_installable_kept")
        );
        assert_eq!(verdict(&decisions, "0.14.6"), None);
        assert_eq!(verdict(&decisions, "0.13.0"), None);
    }

    /// A version whose name is not dotted numbers sorts below every one that
    /// is, so it must never be counted as the newest of anything — including
    /// the newest installable one.
    #[test]
    fn an_unparseable_version_name_is_never_the_newest() {
        let present = versions(&["__release_preflight__", "0.1.0"]);
        let decisions = retention_decision(
            &present,
            &families(&[
                ("__release_preflight__", &["installer"]),
                ("0.1.0", &["installer"]),
            ]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            1,
        );
        assert_eq!(verdict(&decisions, "0.1.0"), Some("newest_kept"));
        assert_eq!(verdict(&decisions, "__release_preflight__"), None);
    }

    #[test]
    fn the_newest_signed_release_survives_a_ladder_full_of_claims() {
        let present = versions(&["0.1.1", "0.1.2", "0.1.3", "0.1.4", "0.1.5"]);
        let decisions = retention_decision(
            &present,
            &families(&[("0.1.1", &["signed"])]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(
            verdict(&decisions, "0.1.1"),
            Some("newest_signed_release_kept"),
            "a pipeline-signed product's only deployable version must survive"
        );
        assert_eq!(verdict(&decisions, "0.1.2"), None);
    }

    /// A product publishing both families keeps the newest complete release
    /// of EACH: `stado` is installed by `install-stado.sh` from the installer
    /// family and by `host release` from the signed one, and keeping only the
    /// newer of the two would leave the other installer with nothing.
    #[test]
    fn both_families_are_pinned_independently() {
        let present = versions(&["0.14.6", "0.15.21", "0.16.0", "0.16.1", "0.16.2", "0.16.3"]);
        let decisions = retention_decision(
            &present,
            &families(&[("0.14.6", &["signed"]), ("0.15.21", &["installer"])]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(
            verdict(&decisions, "0.15.21"),
            Some("newest_installable_kept")
        );
        assert_eq!(
            verdict(&decisions, "0.14.6"),
            Some("newest_signed_release_kept")
        );
    }

    /// One version completing both families is reported once, and reported as
    /// the installer family: the report counts versions, not reasons, so a
    /// version counted twice would make the skip totals disagree with the
    /// number of directories on disk.
    #[test]
    fn a_version_completing_both_families_is_reported_once() {
        let present = versions(&["0.9.0", "1.0.0", "1.1.0", "1.2.0", "1.3.0"]);
        let decisions = retention_decision(
            &present,
            &families(&[("0.9.0", &["installer", "signed"])]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            3,
        );
        assert_eq!(decisions.len(), present.len());
        assert_eq!(
            verdict(&decisions, "0.9.0"),
            Some("newest_installable_kept")
        );
    }
}
