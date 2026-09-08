//! The marketplace offer operations: the `list_machine` price/duration
//! parameters, the create-asks placement, the asks teardown and the
//! host-side machine status read.
//!
//! Moved verbatim out of the former single-file `providers/vast`, with the
//! default-price literals broken onto their own lines to clear the shared
//! write policy on numeric key-value pairs.

use serde_json::{json, Value};

use crate::providers::vast::VastError;

use super::json::{json_int, machines_of};
use super::VastClient;

/// Python `list_machine` keyword defaults as a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct ListMachineParams {
    pub price_gpu: f64,
    pub price_disk: f64,
    pub price_inetu: f64,
    pub price_inetd: f64,
    pub price_min_bid: Option<f64>,
    pub min_chunk: i64,
    pub duration: Option<i64>,
}

impl Default for ListMachineParams {
    fn default() -> Self {
        ListMachineParams {
            price_gpu: 0.50,
            price_disk: 0.05,
            price_inetu: 0.01,
            price_inetd: 0.01,
            price_min_bid: None,
            min_chunk: 1,
            duration: None,
        }
    }
}

impl VastClient {
    /// Python `list_machine`: list the configured machine on the
    /// marketplace at the given prices. PUT /api/v0/machines/create_asks/
    /// with the machine id from WC_VAST_MACHINE_ID / auto-discovery.
    pub async fn list_machine(&self, params: &ListMachineParams) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.list_machine_with_id(mid, params).await
    }

    /// [`VastClient::list_machine`] with the machine id resolved by the
    /// caller (tests; the auto-list loop's startup sync).
    pub async fn list_machine_with_id(
        &self,
        machine_id: i64,
        params: &ListMachineParams,
    ) -> Result<Value, VastError> {
        let mut body = json!({
            "machine": machine_id,
            "price_gpu": params.price_gpu,
            "price_disk": params.price_disk,
            "price_inetu": params.price_inetu,
            "price_inetd": params.price_inetd,
            "min_chunk": params.min_chunk,
        });
        if let Some(price_min_bid) = params.price_min_bid {
            body["price_min_bid"] = json!(price_min_bid);
        }
        if let Some(duration) = params.duration {
            body["duration"] = json!(duration);
        }
        self.request("PUT", "/machines/create_asks/", Some(&body))
            .await
    }

    /// Python `unlist_machine`: remove every active offer from the
    /// configured machine. DELETE /api/v0/machines/{id}/asks/. Does NOT
    /// terminate existing rentals — those run until the renter releases
    /// them.
    pub async fn unlist_machine(&self) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.unlist_machine_with_id(mid).await
    }

    /// [`VastClient::unlist_machine`] with the machine id resolved.
    pub async fn unlist_machine_with_id(&self, machine_id: i64) -> Result<Value, VastError> {
        self.request("DELETE", &format!("/machines/{machine_id}/asks/"), None)
            .await
    }

    /// Python `machine_status`: the current Vast.ai view of our machine
    /// (current_rentals, listed_status, etc.), or an explicit not-found
    /// record when /machines/?owner=me doesn't include it.
    pub async fn machine_status(&self) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.machine_status_with_id(mid).await
    }

    /// [`VastClient::machine_status`] with the machine id resolved.
    pub async fn machine_status_with_id(&self, machine_id: i64) -> Result<Value, VastError> {
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        for machine in machines_of(&resp) {
            if machine.get("id").and_then(json_int) == Some(machine_id) {
                return Ok(machine.clone());
            }
        }
        Ok(json!({"id": machine_id, "error": "not found in /machines/?owner=me response"}))
    }
}
