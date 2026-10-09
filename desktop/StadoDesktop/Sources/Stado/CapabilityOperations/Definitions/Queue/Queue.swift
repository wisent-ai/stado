import Foundation

/// The queue screen's reads of one job, the same `stado status` the CLI
/// answers: a job the queue still holds, or one the run reaper has retired,
/// read back from the outcome its run retained (`reaped: true`). An id no
/// job holds is the CLI's own refusal naming it.
enum NativeQueueOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "job-status", title: "Read one job by its id, including a job the run reaper has retired", path: ["status"], hostPlacement: .none, fields: [
            .init(id: "job", label: "Job id — whole, or its first hex characters, with or without job-", required: true),
        ], mutates: false),
    ]
}
