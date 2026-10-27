import Foundation

/// Which of the candidate's own streams to read off the host.
///
/// Stderr comes first and is the default stream so startup failures remain
/// visible even when stdout contains no output.
enum ReleaseLogStreamSelection: String, CaseIterable, Identifiable, Sendable {
    case err
    case out
    case both

    var id: String { rawValue }

    var title: String {
        switch self {
        case .err: "stderr"
        case .out: "stdout"
        case .both: "Both"
        }
    }
}

/// `stado release logs <product> --target <host> --stream … --lines … --json`,
/// decoded as the CLI writes it.
struct ReleaseLogsReport: Decodable, Sendable {
    let product: String
    let target: String
    let version: String
    let streams: [ReleaseLogStream]
}

/// One log file on the host: where it is, how big it is, and its tail.
///
/// A stream with no lines is not a blank pane. The file was either never
/// created or opened and never written to, and those are different findings
/// about a candidate that died — so `state` is carried as the CLI judged it:
/// `missing`, `empty` or `read`. Every field is decoded as written; a report
/// that lacks one is a decoding error naming the key.
struct ReleaseLogStream: Decodable, Identifiable, Sendable {
    let stream: String
    let path: String
    /// `null` when the file is missing.
    let bytes: Int?
    let lines: [String]
    let state: String

    var id: String { stream }
    var isMissing: Bool { state == "missing" }
    var isEmpty: Bool { state == "empty" }
}
