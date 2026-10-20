//! The unit conversions the memory readers share.

/// One kibibyte, 2^10 bytes by the IEC binary prefix.
pub const KIB: i64 = 1024; // https://physics.nist.gov/cuu/Units/binary.html

/// One mebibyte, 2^20 bytes.
pub const MIB: i64 = KIB * KIB; // https://physics.nist.gov/cuu/Units/binary.html

/// One gibibyte, 2^30 bytes.
pub const GIB: i64 = MIB * KIB; // https://physics.nist.gov/cuu/Units/binary.html

/// Percent, as the whole the swap ratio is taken against.
pub const PERCENT: i64 = 100; // https://www.bipm.org/en/publications/si-brochure (per cent: 10^-2)
