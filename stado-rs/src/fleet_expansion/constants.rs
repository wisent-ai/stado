//! Versioned financial units and explicit bounds on exact portfolio search.
pub const SCHEMA_VERSION: u32 = 1;
/// Twenty-four options bound exhaustive exact selection to 2^24 subsets.
pub const MAX_OPTIONS: usize = 24;
/// A ten-year horizon bounds the declared cashflow projection, not evidence validity.
pub const MAX_HORIZON_MONTHS: u32 = 120;
pub const MAX_WINDOW_DAYS: i64 = 3660;
pub const DEFAULT_HORIZON_MONTHS: u32 = 24;
pub const CENTS_PER_USD: f64 = 100.0;
pub const MAX_MONEY_USD: f64 = 1_000_000_000.0;
/// Mean Gregorian year, used consistently for delivery delay and payback.
pub const DAYS_PER_MONTH: f64 = 365.25 / 12.0;
pub const CENT_PRECISION_TOLERANCE: f64 = 0.00001;
pub const COMPARISON_TOLERANCE: f64 = 0.000001;
pub const MAX_IDENTIFIER_BYTES: usize = 80;
pub const MAX_TEXT_BYTES: usize = 4096;
pub const MAX_SUBJECT_BYTES: usize = 128;
pub const MAX_LEAD_TIME_DAYS: u32 = 36525;
pub const CATALOG_PATH: &str = "state/fleet/expansion/catalog.json";
pub const PLAN_PREFIX: &str = "state/fleet/expansion/plans/";
