import Foundation

/// The five steps of one durable A/B storage-root transaction, and the sentence
/// the sheet shows before each of them runs.
///
/// Only `status` is read-only, and the sheet reads `isReadOnly` to decide
/// whether an operator sees a confirmation dialog first.
enum StorageReconciliationPhase: String, CaseIterable, Identifiable, Sendable {
    case run
    case resume
    case status
    case rollback
    case finalize

    var id: String { rawValue }

    var title: String {
        switch self {
        case .run: "Run"
        case .resume: "Resume"
        case .status: "Status"
        case .rollback: "Rollback"
        case .finalize: "Finalize"
        }
    }

    var isReadOnly: Bool { self == .status }

    var explanation: String {
        switch self {
        case .run:
            "Start this durable transaction. Acceptance only means the resident operation owns the request; it is not completion."
        case .resume:
            "Continue the same durable transaction after an interruption. Stado re-reads its recorded state rather than starting another transaction."
        case .status:
            "Read the durable transaction receipt and lifecycle fence. This performs no reconciliation step."
        case .rollback:
            "Restore the exact captured prior primary and mirror. Stado refuses rollback after the data-activation boundary."
        case .finalize:
            "Record lifecycle cleanup only after activation. Completion exists only when a later status receipt reports it."
        }
    }
}
