//! The marketplace offer operations: the `list_machine` price/duration
//! parameters, the create-asks placement, the asks teardown and the
//! host-side machine status read.

use serde_json::{json, Value};

use crate::providers::vast::VastError;

use super::json::{json_int, machines_of};
use super::VastClient;

/// What a listing offers, every price as the operator stated it. There is
/// no default: a listing at a price nobody stated rents the machine out on
/// terms nobody chose. Bandwidth prices and the minimum chunk are not sent,
/// so Vast.ai applies its own.
#[derive(Debug, Clone, PartialEq)]
pub struct ListMachineParams {
    /// Per-GPU-hour price in USD.
    pub price_gpu: f64,
    /// Per-GB-month disk price in USD.
    pub price_disk: f64,
    /// The interruptible-bid price floor, when one is stated.
    pub price_min_bid: Option<f64>,
    /// The longest rental in seconds; `None` leaves the offer open-ended.
    pub duration: Option<i64>,
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
    /// (current_rentals, listed_status, etc.).
    pub async fn machine_status(&self) -> Result<Value, VastError> {
        let mid = self.machine_id().await?;
        self.machine_status_with_id(mid).await
    }

    /// [`VastClient::machine_status`] with the machine id resolved. A
    /// machine the account's listing does not hold is refused naming the
    /// ids it does hold: answered as a record, readiness read it as Ready
    /// and the bridge as a machine that is merely not listed.
    pub async fn machine_status_with_id(&self, machine_id: i64) -> Result<Value, VastError> {
        let resp = self.request("GET", "/machines/?owner=me", None).await?;
        let machines = machines_of(&resp)?;
        if let Some(machine) = machines
            .iter()
            .find(|machine| machine.get("id").and_then(json_int) == Some(machine_id))
        {
            return Ok((*machine).clone());
        }
        let held: Vec<String> = machines
            .iter()
            .map(|machine| super::json::shown(machine.get("id")))
            .collect();
        Err(VastError::config(format!(
            "Vast.ai /machines/?owner=me does not hold machine {machine_id}; this key's \
             account holds [{}]",
            held.join(", ")
        )))
    }
}
