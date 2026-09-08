//! The two money sections, printed back to back: what the fleet has already
//! spent, then what it is allowed to spend.
//!
//! `billing` renders the published provider snapshot, `budgets` the policy
//! limits and the GCP budgets, and `amounts` holds the number coercion and
//! the currency formatting both of them need — the providers report the same
//! figure as a number, as a string, or as units plus nanos.

mod amounts;
mod billing;
mod budgets;

use serde_json::Value;

pub(super) fn print_money(document: &Value) {
    billing::print_billing(document);
    budgets::print_budgets(document);
}
