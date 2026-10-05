//! Versioned financial units.
pub const SCHEMA_VERSION: u32 = 1;
pub const CENTS_PER_USD: f64 = 100.0;
/// Mean Gregorian year, used consistently for delivery delay and payback.
pub const DAYS_PER_MONTH: f64 = 365.25 / 12.0;
pub const CENT_PRECISION_TOLERANCE: f64 = 0.00001;
pub const COMPARISON_TOLERANCE: f64 = 0.000001;
pub const CATALOG_PATH: &str = "state/fleet/expansion/catalog.json";
pub const PLAN_PREFIX: &str = "state/fleet/expansion/plans/";
