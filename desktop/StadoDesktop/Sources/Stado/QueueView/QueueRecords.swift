import SwiftUI
import WisentDesignSystem

/// The row and facet vocabulary of the queue screen, and the four readers that
/// derive it from a published snapshot.
///
/// These types and the extension below are internal rather than private only
/// because the rail, the table, the inspector and the rerun dialog that name
/// them sit in sibling files: Swift scopes `private` to one file.
enum QueueFacet: String, Hashable {
    case allOutcomes
    case failed
    case completed
    case models
}

struct QueueRecord: Identifiable {
    enum Kind {
        case failed
        case completed

        var label: String {
            switch self {
            case .failed: "Failed"
            case .completed: "Completed"
            }
        }

        var tone: WisentTone {
            switch self {
            case .failed: .danger
            case .completed: .success
            }
        }
    }

    let id: String
    let kind: Kind
    let jobID: String
    let model: String?
    let task: String?
    let wallSeconds: Double?
    let completedAt: String?
    let error: String?
}

struct ModelRecord: Identifiable {
    let id: String
    let model: String
    let counts: JobCounts

    var active: Int { counts.queue + counts.running }
}

// MARK: Records

extension QueueView {
    func allRecords(_ snapshot: DashboardSnapshot) -> [QueueRecord] {
        let failed = snapshot.recentFailed.enumerated().map { index, job in
            QueueRecord(
                id: "failed-\(index)-\(job.jobID)",
                kind: .failed,
                jobID: job.jobID,
                model: job.model,
                task: job.task,
                wallSeconds: nil,
                completedAt: nil,
                error: job.error
            )
        }
        let completed = snapshot.completedRecent.enumerated().map { index, job in
            QueueRecord(
                id: "completed-\(index)-\(job.jobID)",
                kind: .completed,
                jobID: job.jobID,
                model: job.model,
                task: job.task,
                wallSeconds: job.wallSeconds,
                completedAt: job.completedAt,
                error: nil
            )
        }
        return failed + completed
    }

    func records(_ snapshot: DashboardSnapshot) -> [QueueRecord] {
        let records = allRecords(snapshot)
        switch facet {
        case .failed: return records.filter { $0.kind == .failed }
        case .completed: return records.filter { $0.kind == .completed }
        case .allOutcomes, .models: return records
        }
    }

    func modelRecords(_ snapshot: DashboardSnapshot) -> [ModelRecord] {
        snapshot.byModelState
            .filter { $0.value.queue > 0 || $0.value.running > 0 }
            .map { ModelRecord(id: "model-\($0.key)", model: $0.key, counts: $0.value) }
            .sorted { $0.active == $1.active ? $0.model < $1.model : $0.active > $1.active }
    }

    func minorityKind(in rows: [QueueRecord]) -> QueueRecord.Kind? {
        let failed = rows.count { $0.kind == .failed }
        let completed = rows.count - failed
        guard failed > 0, completed > 0 else { return nil }
        return failed <= completed ? .failed : .completed
    }
}
