import Foundation

/// The queue screen's operations, each the same CLI command it names: reading
/// one job (`stado status`) — a job the queue still holds, or one the run
/// reaper has retired, read back from the outcome its run retained
/// (`reaped: true`), an id no job holds being the CLI's own refusal naming it —
/// changing a queued job's priority, and the completed jobs as contributions
/// of the owners the registry declares for the hosts that ran them
/// (`stado contributions pending|report`): what would be credited, what the
/// ledger already holds, and sending the rest.
enum NativeQueueOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "job-status", title: "Read one job by its id, including a job the run reaper has retired", path: ["status"], hostPlacement: .none, fields: [
            .init(id: "job", label: "Job id — whole, or its first hex characters, with or without job-", required: true),
        ], mutates: false),
        .init(id: "job-priority", title: "Change a queued job's priority", path: ["job", "prioritize"], hostPlacement: .none, fields: [
            .init(id: "job", label: "Queued job id", required: true),
            .init(id: "priority", label: "Nonnegative integer priority — higher is claimed first, then FIFO", required: true),
        ]),
        .init(id: "contributions-pending", title: "List completed jobs as contributions of their hosts' owners, and which the ledger holds", path: ["contributions", "pending"], hostPlacement: .none, fields: [
            .init(id: "ledger", label: "Contribution ledger origin (blank uses STADO_CONTRIBUTION_LEDGER_URL, or lists without asking)", option: "--ledger"),
        ], mutates: false),
        .init(id: "contributions-report", title: "Send the contributions the ledger does not hold yet", path: ["contributions", "report"], hostPlacement: .none, fields: [
            .init(id: "ledger", label: "Contribution ledger origin (blank uses STADO_CONTRIBUTION_LEDGER_URL)", option: "--ledger"),
        ]),
    ]
}
