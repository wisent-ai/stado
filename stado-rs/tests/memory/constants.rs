//! Every number the memory tests declare, in one place.
//!
//! The fixtures state watermarks in MiB and percentages, and each value below
//! is chosen to make one verdict certain on any machine this suite runs on
//! rather than to look plausible.

/// A low watermark no machine can be under: the pass must report the host as
/// over its watermark whatever it actually has.
pub const ALWAYS_OVER_LOW_MB: i64 = 1_048_576;
/// The target that goes with it. Greater than the low watermark, because the
/// validator refuses a target that is not.
pub const ALWAYS_OVER_TARGET_MB: i64 = 2_097_152;

/// A low watermark no machine can be over.
pub const NEVER_OVER_LOW_MB: i64 = 64;
/// The target that goes with it.
pub const NEVER_OVER_TARGET_MB: i64 = 128;

/// A swap watermark no machine reaches, so the memory watermark alone decides.
pub const UNREACHABLE_SWAP_PCT: i64 = 100;

/// One repair per pass, which is the reporting default and the value every
/// fixture keeps.
pub const MAX_REPAIRS_PER_PASS: i64 = 1;

/// A target watermark above the validator's own floor and below its low
/// watermark: the incoherent declaration the validator must refuse with the
/// coherence sentence rather than with the floor's.
pub const INCOHERENT_TARGET_MB: i64 = 128;

/// The registry schema version the fixtures declare.
pub const REGISTRY_SCHEMA_VERSION: i64 = 2;
