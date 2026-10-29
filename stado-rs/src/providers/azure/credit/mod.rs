//! No cost-bearing Azure operation starts unless the account's credit, less
//! the first hour of the machine it starts, stays above the floor the
//! operator named in his ruling ([`floor`]).
//!
//! The billing monitor only alerted after the fact: a VM created on an
//! exhausted grant billed real money until somebody read the alert. Here,
//! immediately before a VM is created or started, the balance is read
//! through the same Skarbiec principal and ARM call the collector uses, the
//! machine's size is priced from the Azure Retail Prices API at the highest
//! on-demand hourly rate any configured location quotes for it, and the
//! operation is refused unless the balance less that hour stays above the
//! floor. No floor ruling, a balance that cannot be read, a section that
//! names none or names another currency, and a size no price quotes each
//! refuse with their cause: an unknown balance or cost is never read as
//! safe. How long the machine will run is not known when it starts, which
//! the refusal and the docs say; the first hour is the cost the gate can
//! know. The billing collector publishes the balance-against-floor part of
//! the decision ([`standing`]), so `stado billing show` and Stado Desktop
//! say whether a VM can start now.
//!
//! Machines already running are not stopped here: stopping running work is
//! cycling it, which needs Oko's admission or a ruling of the operator
//! naming it, never a side effect of a billing read. The floor refuses new
//! creates and starts, which is what his ruling asks for.

mod floor;

use serde_json::{json, Value};

use crate::providers::ProviderError;

/// The currency the Azure Retail Prices API quotes the machines the gate
/// prices in, and so the one a balance and floor must be in to subtract an
/// hour of a machine from them.
const PRICE_CURRENCY: &str = "USD";

/// Admit `operation` on a machine of `vm_size`, or refuse it naming the
/// balance, the machine's hourly price, the operator's floor ruling and
/// where each was read.
pub(super) async fn admit(operation: &str, vm_size: &str) -> Result<(), ProviderError> {
    admit_burn(operation, vm_size, 0.0).await.map(drop)
}

/// What the gate read when it admitted a machine: the balance, the hour of
/// the new machine with where it was priced, the hourly burn of the machines
/// already running beside it, and the floor.
pub(crate) struct Admitted {
    pub(crate) balance: f64,
    pub(crate) hourly: f64,
    pub(crate) priced: String,
    pub(crate) running_hourly: f64,
    pub(crate) floor: floor::Floor,
}

impl Admitted {
    /// The balance left after one more hour of every machine counted.
    pub(crate) fn after_hour(&self) -> f64 {
        self.balance - self.hourly - self.running_hourly
    }
}

/// [`admit`] for a machine started beside others that already burn
/// `running_hourly` USD an hour: one hour of all of them is taken from the
/// balance, and what remains must stay above the floor.
pub(crate) async fn admit_burn(
    operation: &str,
    vm_size: &str,
    running_hourly: f64,
) -> Result<Admitted, ProviderError> {
    let floor = floor::usable(operation)
        .await
        .map_err(ProviderError::Value)?;
    let section = crate::monitor::billing::azure_balance_section().await;
    let balance = balance(operation, &section)
        .and_then(|balance| floor::in_currency(operation, &section, &floor, balance))
        .map_err(ProviderError::Value)?;
    let (hourly, priced) = hourly(operation, vm_size)
        .await
        .map_err(ProviderError::Value)?;
    let admitted = Admitted {
        balance,
        hourly,
        priced,
        running_hourly,
        floor,
    };
    if admitted.after_hour() <= admitted.floor.amount {
        let beside = if running_hourly > 0.0 {
            format!(
                " and one hour of the machines already running ({running_hourly} {PRICE_CURRENCY})"
            )
        } else {
            String::new()
        };
        return Err(ProviderError::Value(format!(
            "{operation} is refused: the Azure credit balance {balance} {PRICE_CURRENCY} less one hour of {vm_size} \
             ({hourly} {PRICE_CURRENCY}, {}){beside} is not above the floor {}; how long the machine would run \
             is not known when it starts",
            admitted.priced,
            admitted.floor.named()
        )));
    }
    Ok(admitted)
}

/// The gate's balance-against-floor decision on one read of the Azure
/// billing section, as the collector publishes it under `provision_gate`:
/// `admits`, the operator's floor with its ruling, the `refusal` when it does
/// not admit, and what each start adds to it.
pub(crate) async fn standing(section: &Value) -> Value {
    let operation = "a VM create or start";
    let per_start = "each create or start also prices one hour of its machine size and needs the balance less that hour above the floor";
    let floor = match floor::usable(operation).await {
        Ok(floor) => floor,
        Err(refusal) => {
            return json!({ "admits": false, "refusal": refusal, "perStart": per_start })
        }
    };
    let named = json!({ "amount": floor.amount, "currency": floor.currency, "ruling": floor.ruling, "said": floor.said });
    let decided = balance(operation, section)
        .and_then(|balance| floor::in_currency(operation, section, &floor, balance));
    match decided {
        Ok(balance) if balance > floor.amount => {
            json!({ "admits": true, "floor": named, "perStart": per_start })
        }
        Ok(balance) => json!({
            "admits": false,
            "floor": named,
            "refusal": format!(
                "{operation} is refused: the Azure credit balance {balance} {} is not above the floor {}",
                floor.currency,
                floor.named()
            ),
            "perStart": per_start,
        }),
        Err(refusal) => {
            json!({ "admits": false, "floor": named, "refusal": refusal, "perStart": per_start })
        }
    }
}

/// The available balance one read of the Azure billing section states.
fn balance(operation: &str, section: &Value) -> Result<f64, String> {
    let status = section.get("status").and_then(Value::as_str);
    if status != Some("ok") {
        let status = match status {
            Some(status) => status.to_owned(),
            None => "absent".to_owned(),
        };
        let cause = match section.get("detail").and_then(Value::as_str) {
            Some(detail) => format!("cause: {detail}"),
            None => format!("section {section}"),
        };
        return Err(format!(
            "{operation} is refused: the Azure credit balance could not be read (billing status {status}, {cause}); \
             `stado billing show` reads the same section"
        ));
    }
    section.get("available_balance").and_then(Value::as_f64).ok_or_else(|| {
        format!("{operation} is refused: the Azure billing section reports ok but names no available_balance: {section}")
    })
}

/// The highest on-demand hourly price in [`PRICE_CURRENCY`] any configured
/// location quotes for `vm_size`, with where it was read; refused when none
/// quotes it.
async fn hourly(operation: &str, vm_size: &str) -> Result<(f64, String), String> {
    let source = crate::autonomy::cost::azure_vm_prices().await;
    let highest = source
        .quotes
        .iter()
        .filter(|quote| {
            quote.machine_type.as_deref() == Some(vm_size)
                && quote.purchase_option == "on_demand"
                && quote.currency == PRICE_CURRENCY
                && quote.hourly_usd.is_normal()
                && quote.hourly_usd.is_sign_positive()
        })
        .max_by(|left, right| left.hourly_usd.total_cmp(&right.hourly_usd));
    if let Some(quote) = highest {
        let region = match quote.region.as_deref() {
            Some(region) => format!("in {region}"),
            None => "with no region named".to_owned(),
        };
        return Ok((
            quote.hourly_usd,
            format!(
                "{} {region}, {} at {}",
                quote.sku, quote.source, quote.observed_at
            ),
        ));
    }
    let read = match source.error.as_deref() {
        Some(error) => format!("{}: {error}", source.source),
        None => source.source.clone(),
    };
    Err(format!(
        "{operation} is refused: no on-demand {PRICE_CURRENCY} hourly price quotes {vm_size} in the configured Azure locations ({read}), so its cost is not known"
    ))
}
