//! Announcing a maintenance window to Vast before taking a listed host down.
//!
//! Vast's host documentation: the proper way to maintain a machine is to
//! wait until its contracts expire; for any other maintenance, `schedule
//! maint` notifies every renter that the machine is going down so they save
//! their work, and a machine that goes offline under an active contract
//! without it lowers its reliability score, which decides how often it is
//! rented. The request is the one vast-cli sends: `PUT
//! /machines/{id}/dnotify/` with the start in Unix seconds and the duration
//! in hours. Vast notifies only the renters it holds, so announcing on a
//! machine with none costs nothing.

use serde_json::{json, Value};

use crate::providers::vast::VastError;

use super::VastClient;

/// Why the machine is going down, in Vast's own categories.
pub const CATEGORIES: [&str; 6] = ["power", "internet", "disk", "gpu", "software", "other"];

impl VastClient {
    /// The machine this account holds whose Vast hostname is one of
    /// `hostnames`, or `None` when it holds none: an exact match only, so an
    /// operation on a host Vast does not list never notifies the renters of
    /// another machine of the account.
    pub async fn listed_machine(&self, hostnames: &[String]) -> Result<Option<i64>, VastError> {
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        for machine in super::json::machines_of(&resp)? {
            let named = super::json::machine_text(machine, "hostname")?
                .is_some_and(|name| hostnames.iter().any(|host| host == name.trim()));
            if named {
                return super::json::machine_id_of(machine).map(Some);
            }
        }
        Ok(None)
    }

    /// Announce maintenance on machine `machine_id`, for an operation run
    /// from another host (a reboot over the host channel).
    pub async fn schedule_maintenance_on_machine(
        &self,
        machine_id: i64,
        start_unix: i64,
        hours: f64,
        category: &str,
    ) -> Result<Value, VastError> {
        self.schedule_maintenance_on(machine_id, start_unix, hours, category)
            .await
    }
}

impl VastClient {
    /// Announce that this host's machine goes down at `start_unix` for
    /// `hours`, for `category` (one of [`CATEGORIES`]). Vast's answer names
    /// how many renters it notified.
    pub async fn schedule_maintenance(
        &self,
        start_unix: i64,
        hours: f64,
        category: &str,
    ) -> Result<Value, VastError> {
        let machine_id = self.machine_id().await?;
        self.schedule_maintenance_on(machine_id, start_unix, hours, category)
            .await
    }

    async fn schedule_maintenance_on(
        &self,
        machine_id: i64,
        start_unix: i64,
        hours: f64,
        category: &str,
    ) -> Result<Value, VastError> {
        if !CATEGORIES.contains(&category) {
            return Err(VastError::config(format!(
                "maintenance category {category:?} is not one of Vast's: {}",
                CATEGORIES.join(", ")
            )));
        }
        if !(hours.is_finite() && hours > 0.0) {
            return Err(VastError::config(format!(
                "a maintenance window of {hours} hours is not a duration"
            )));
        }
        let body = json!({
            "client_id": "me",
            "sdate": start_unix,
            "duration": hours,
            "maintenance_category": category,
        });
        self.request(
            "PUT",
            &format!("/machines/{machine_id}/dnotify/"),
            Some(&body),
        )
        .await
    }
}
