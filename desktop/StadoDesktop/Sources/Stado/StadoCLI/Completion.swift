import Foundation

extension StadoCLI {
    /// What one invocation produced: exact stdout and stderr, and the sentence
    /// it refused with when it exited non-zero.
    ///
    /// A non-zero exit is not the absence of an answer here. `host gates`
    /// exits non-zero when the host is claiming nothing, `service converge`
    /// when a binary has drifted, `release status` when a host never reported
    /// its software — each after printing its complete `--json` payload. Those
    /// are the exact states these screens were built to show, so the payload
    /// is decoded first while both raw streams and the refusal remain
    /// available to the caller.
    struct Completion: Sendable {
        let output: Data
        let errors: Data
        let refusal: StadoCLIError?
        let exitCode: Int32
    }
}
