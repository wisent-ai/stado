//! A payout a source made in a token, and its value in US dollars.
//!
//! Many sources pay in their own token, and a token payout is income only at
//! the rate it is converted at. Every source adapter records a payout here:
//! the amount and token, and the rate it was valued at with that rate's time
//! and the published table it was read from. A payout whose rate is not
//! declared stays unpriced, naming why, so no income is counted at a guess;
//! a USD stablecoin is a token like any other and declares its rate too.

use serde::{Deserialize, Serialize};

/// One rate: the US dollars one unit of `token` was worth, when, per whom.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenRate {
    pub token: String,
    pub usd_per_unit: f64,
    pub as_of: String,
    pub source: String,
}

/// One payout as a source stated it, with the rate it is valued at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenPayout {
    /// The source, by its id in the declared source list.
    pub source: String,
    pub host: String,
    pub paid_at: String,
    pub token: String,
    pub amount: f64,
    pub rate: Option<TokenRate>,
}

impl TokenPayout {
    /// The payout in US dollars, or why it has no price: no rate declared, a
    /// rate for another token, or a rate that is not a positive number.
    pub fn usd(&self) -> Result<f64, String> {
        let rate = self.rate.as_ref().ok_or_else(|| {
            format!(
                "{} {} paid by {} declares no rate (token, usd_per_unit, as_of, source), so it \
                 has no US-dollar value",
                self.amount, self.token, self.source
            )
        })?;
        if rate.token != self.token {
            return Err(format!(
                "the payout is in {} and its rate is for {}",
                self.token, rate.token
            ));
        }
        if !(rate.usd_per_unit.is_finite() && rate.usd_per_unit > 0.0) {
            return Err(format!(
                "the rate for {} ({}) is not a positive number",
                rate.token, rate.usd_per_unit
            ));
        }
        if rate.as_of.trim().is_empty() || rate.source.trim().is_empty() {
            return Err(format!(
                "the rate for {} names no date or no source, so nobody stated it",
                rate.token
            ));
        }
        Ok(self.amount * rate.usd_per_unit)
    }
}
