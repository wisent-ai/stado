import Foundation

/// What `release doctor` said about one product/target pair, or why it could
/// not say anything.
///
/// A diagnosis that failed is a state of its own. Folding it into "no data"
/// would render an unreachable host exactly like a settled rollout, which is
/// the reading this screen exists to prevent.
enum ReleaseDiagnosis: Sendable {
    case pending
    case diagnosed(ReleaseDoctorReport)
    case failed(String)
}

/// One row of the Releases table: the pair, its diagnosis, and what the host
/// itself reported it runs.
struct ReleaseRow: Identifiable, Sendable {
    let pair: ReleaseInventoryPair
    let diagnosis: ReleaseDiagnosis
    /// Straight off `release status --json`, never recomputed here.
    let software: ReleaseSoftwareReport?

    init(
        pair: ReleaseInventoryPair,
        diagnosis: ReleaseDiagnosis,
        software: ReleaseSoftwareReport? = nil
    ) {
        self.pair = pair
        self.diagnosis = diagnosis
        self.software = software
    }

    var id: String { pair.id }
    var product: String { pair.product }
    var target: String { pair.target }

    var report: ReleaseDoctorReport? {
        guard case let .diagnosed(report) = diagnosis else { return nil }
        return report
    }

    var problem: String? {
        guard case let .failed(problem) = diagnosis else { return nil }
        return problem
    }

    var isPending: Bool {
        guard case .pending = diagnosis else { return false }
        return true
    }

    /// Sort key. Blocked first, then a rollout nobody could diagnose, then the
    /// ones still moving, and settled last.
    ///
    /// A host that cannot be shown to run what the fleet declares never sorts
    /// below a moving rollout, whatever the release agent's own state file says
    /// about the rollout. `brama desired=0.2.27 observed=unreported` sat quietly
    /// in a list for a day; a row whose software verdict failed is pulled up
    /// beside the blocked ones so it cannot do that again.
    var attentionRank: Int {
        let rollout: Int = switch diagnosis {
        case .pending:
            4
        case .failed:
            1
        case let .diagnosed(report):
            switch report.verdict {
            case .blocked:
                0
            case .rolling:
                2
            case .unrecognised:
                3
            case .settled:
                5
            }
        }
        return software?.failed == true ? min(rollout, 1) : rollout
    }
}
