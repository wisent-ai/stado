import Foundation

/// The one tuning value the declared-repair reads carry, kept in a constants
/// module so the number is named where it is declared and used by name where
/// it bounds a call.
enum RepairConstants {
    /// Seconds a dry run may take. A dry run only reads the host, so it is
    /// bounded; an applied run carries no timeout, because stopping a
    /// mutating step midway would leave the host in an unreported state.
    static let dryRunTimeoutSeconds: Int = 180
}
