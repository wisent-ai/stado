import Foundation

/// Which of the candidate's own streams to read off the host.
///
/// stderr first, and stderr alone by default: in the incident the answer was
/// in `.err` while `.out` was empty, and a reader that opens stdout first
/// buries it.
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

/// `stado release logs <product> --target <host> --stream … --lines … --json`.
struct ReleaseLogsReport: Decodable, Sendable {
    let product: String
    let target: String
    let version: String
    let streams: [ReleaseLogStream]

    enum CodingKeys: String, CodingKey {
        case product, target, version, streams
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        version = try values.decodeIfPresent(String.self, forKey: .version) ?? ""
        streams = try values.decodeIfPresent([ReleaseLogStream].self, forKey: .streams) ?? []
    }
}

/// One log file on the host: where it is, how big it is, and its tail.
///
/// A stream with no lines is not a blank pane. The file was either never
/// created or opened and never written to, and those are different findings
/// about a candidate that died — so `state` is carried and, when the CLI
/// predates it, derived from the bytes.
struct ReleaseLogStream: Decodable, Identifiable, Sendable {
    let stream: String
    let path: String
    let bytes: Int?
    let lines: [String]
    let state: String

    var id: String { stream }

    var isMissing: Bool { state == "missing" }
    var isEmpty: Bool { state == "empty" }

    enum CodingKeys: String, CodingKey {
        case stream, path, bytes, lines, state
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        stream = try values.decodeIfPresent(String.self, forKey: .stream) ?? ""
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        bytes = try values.decodeIfPresent(Int.self, forKey: .bytes)
        lines = try values.decodeIfPresent([String].self, forKey: .lines) ?? []
        if let reported = try values.decodeIfPresent(String.self, forKey: .state) {
            state = reported
        } else if bytes == nil {
            state = "missing"
        } else if lines.isEmpty {
            state = "empty"
        } else {
            state = "read"
        }
    }
}
