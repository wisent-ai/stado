import Foundation

/// The submit queue's operations on the queue screen, each the same CLI
/// command it names (`stado change …`): queue a commit to land on its
/// repository's main, run one pass of a repository's queue (rebase onto the
/// remote main, the declared gates on the fleet, the risk classification and
/// harsh review, the fast-forward push and the per-defect records), read one
/// candidate or the list, withdraw one, and move one ahead of the queue.
enum NativeChangeQueueOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "change-submit", title: "Queue a commit to land on its repository's main", path: ["change", "submit"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "The repository's checkout on the machine of the selected Stado endpoint", required: true),
            .init(id: "commit", label: "Commit to land (one the checkout holds, or one origin gives by its id)", option: "--commit", required: true),
            .init(id: "defects", label: "Oko defects the commit repairs, one per line (the first scopes the harsh review; a high-risk change must name one)", option: "--defect", multiple: true),
        ]),
        .init(id: "change-run", title: "Run one pass of a repository's submit queue: rebase, gates on the fleet, review, land", path: ["change", "run"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "The repository's checkout on the machine of the selected Stado endpoint (blank runs every repository with a waiting candidate)"),
            .init(id: "hosts", label: "Registry hosts that run the compiling gates, one per line (blank chooses them from their capacity)", option: "--host", multiple: true),
            .init(id: "integrated", label: "Directory whose <product>.jsonl gets one line per landed defect (blank records in Oko only)", option: "--integrated"),
        ]),
        .init(id: "change-status", title: "Read one submit queue candidate: its rebase, risk, every gate shard, reviews and landing", path: ["change", "status"], hostPlacement: .none, fields: [
            .init(id: "id", label: "Candidate id, whole or a unique prefix", required: true),
        ], mutates: false),
        .init(id: "change-list", title: "List the submit queue's candidates in the queue's order: those moved ahead first, then oldest first", path: ["change", "list"], hostPlacement: .none, fields: [
            .init(id: "checkout", label: "A repository's checkout (blank lists every repository)"),
            .init(id: "state", label: "Only this state: queued, running, landed, refused, repaired or cancelled", option: "--state"),
        ], mutates: false),
        .init(id: "change-cancel", title: "Withdraw a queued or running submit queue candidate", path: ["change", "cancel"], hostPlacement: .none, fields: [
            .init(id: "id", label: "Candidate id, whole or a unique prefix", required: true),
        ]),
        .init(id: "change-prioritize", title: "Move a queued submit queue candidate ahead of the queue, with the reason", path: ["change", "prioritize"], hostPlacement: .none, fields: [
            .init(id: "id", label: "Candidate id, whole or a unique prefix", required: true),
            .init(id: "reason", label: "Why it goes before the candidates submitted before it", option: "--reason", required: true),
        ]),
    ]
}
