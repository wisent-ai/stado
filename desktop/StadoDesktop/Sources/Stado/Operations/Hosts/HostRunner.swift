import Foundation

struct ServiceRunnerRuntimeReport: Decodable, Sendable {
    let stdout: String
}

/// One `stado runner … --json` lifecycle report.
///
/// `listener` is typed independently from the daemon's process state, and
/// `runnerScope` is the registration record read from the host itself.
struct RunnerListenerReport: Decodable, Sendable {
    let connected: Bool?
    let state: String
}
struct HostRunnerReport: Decodable, Sendable {
    let profile: String
    let target: String
    let platform: String
    let runnerKind: String
    let runnerLabels: String
    let runnerScope: String?
    let hostJobSlot: String
    let listener: RunnerListenerReport
    let status: String
    let exitCode: Int
    let stdout: String
    let stderr: String
    let registration: RunnerRegistrationReport?
    let modelReview: RunnerModelReviewReport?

    enum CodingKeys: String, CodingKey {
        case target
        case profile
        case platform
        case runnerKind = "runner_kind"
        case runnerLabels = "runner_labels"
        case runnerScope = "runner_scope"
        case hostJobSlot = "host_job_slot"
        case listener
        case status
        case exitCode = "exit_code"
        case stdout
        case stderr
        case registration
        case modelReview = "model_review"
    }
}

struct RunnerRegistrationReport: Decodable, Sendable {
    let scope: String
    let runner: String
    let present: Bool?
    let status: String?
    let reconfigured: Bool
}

struct RunnerModelReviewReport: Decodable, Sendable {
    let secret: String
    let state: String
    let reconcileWith: String?

    enum CodingKeys: String, CodingKey {
        case secret, state
        case reconcileWith = "reconcile_with"
    }
}
