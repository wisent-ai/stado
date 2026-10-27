//! Versioned financial units.

/// The expansion catalog's document version:
/// <https://stado.wisent.com/docs/fleet-expansion>.
pub const SCHEMA_VERSION: u32 = 1;
/// US dollar amounts carry two minor-unit decimals, the cents:
/// <https://www.iso.org/iso-4217-currency-codes.html>.
pub const CENT_DECIMALS: usize = 2;
/// Cents in one US dollar, from its two minor-unit decimals:
/// <https://www.iso.org/iso-4217-currency-codes.html>.
pub const CENTS_PER_USD: f64 = 100.0;
/// A percentage is a number of hundredths:
/// <https://www.bipm.org/en/publications/si-brochure>.
pub const PER_CENT: f64 = 100.0;
/// A month as a twelfth of the Julian year of 365.25 days, used
/// consistently for delivery delay and payback:
/// <https://www.iau.org/Iau/Publications/List-of-Recommendations>.
pub const DAYS_PER_MONTH: f64 = 365.25 / 12.0;
pub const CATALOG_PATH: &str = "state/fleet/expansion/catalog.json";
pub const PLAN_PREFIX: &str = "state/fleet/expansion/plans/";
