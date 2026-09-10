import Combine
import Foundation
import WisentDesignSystem

/// The argv `ReleaseEvidenceStore` runs, and the ordering its table depends on.
///
/// Every member is a `nonisolated static`: a command to run, a diagnosis of one
/// pair, or a sentence for something that already happened.
extension ReleaseEvidenceStore {
    nonisolated static func inventoryArguments() -> [String] {
        ["release", "status", "--json"]
    }

    nonisolated static func resumeArguments(runID: String) -> [String] {
        ["release", "resume", runID, "--json"]
    }

    nonisolated static func doctorArguments(pair: ReleaseInventoryPair) -> [String] {
        ["release", "doctor", pair.product, "--target", pair.target, "--json"]
    }

    nonisolated static func logsArguments(
        pair: ReleaseInventoryPair,
        stream: ReleaseLogStreamSelection,
        lines: Int
    ) -> [String] {
        [
            "release", "logs", pair.product,
            "--target", pair.target,
            "--stream", stream.rawValue,
            "--lines", String(lines),
            "--json",
        ]
    }

    nonisolated static func quarantineArguments(pair: ReleaseInventoryPair) -> [String] {
        ["release", "quarantine", "list", pair.product, "--target", pair.target, "--json"]
    }

    nonisolated static func clearArguments(
        pair: ReleaseInventoryPair,
        digest: String,
        reason: String
    ) -> [String] {
        [
            "release", "quarantine", "clear", pair.product,
            "--target", pair.target,
            "--digest", digest,
            "--reason", reason,
            "--json",
        ]
    }

    nonisolated static func diagnosis(
        of pair: ReleaseInventoryPair,
        using cli: StadoCLI
    ) async -> ReleaseDiagnosis {
        do {
            return .diagnosed(
                try await cli.json(
                    ReleaseDoctorReport.self,
                    arguments: doctorArguments(pair: pair)
                )
            )
        } catch {
            return .failed(message(for: error))
        }
    }

    /// Blocked rollouts first, then the ones nobody could diagnose, then the
    /// ones still moving. A settled rollout is the row an operator scrolls
    /// past, so it sits at the bottom.
    nonisolated static func ordered(_ rows: [ReleaseRow]) -> [ReleaseRow] {
        rows.sorted { lhs, rhs in
            lhs.attentionRank == rhs.attentionRank
                ? lhs.id < rhs.id
                : lhs.attentionRank < rhs.attentionRank
        }
    }

    nonisolated static func summary(of clearance: ReleaseQuarantineClearance) -> String {
        "Cleared \(clearance.digest) for \(clearance.product) on \(clearance.target). "
            + "Nothing was started, stopped or restarted; the release agent rolls this digest out on its next tick."
    }

    nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
