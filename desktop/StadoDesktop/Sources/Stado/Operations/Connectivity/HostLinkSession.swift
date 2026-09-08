import Foundation
import WisentDesignSystem

/// The `session` block of `stado host link <host> --json`: whether anybody is
/// logged in on the screen of that machine.
///
/// The fact lived only in CLI output until now, and the GUI was silent about
/// the single reason `control-host` can take no work. Nobody at the
/// screen of an always-on box is the normal state for an always-on box, so
/// nothing here is coloured: what is wrong, when something is, arrives as one
/// of the command's own blockers beside this line.
struct HostLinkSession: Decodable, Sendable {
    let kind: HostLinkSessionKind
    /// Who owns `/dev/console`, as the host reports it: `root` where nobody is
    /// logged in, the login name where somebody is. `nil` only where the probe
    /// could not answer.
    let consoleOwner: String?
    /// The resolver's own sentence, verbatim. It names the console device and
    /// the domain launchd did or did not build — the machine detail, which
    /// belongs beneath the plain words rather than in them.
    let detail: String

    /// The plain words an operator reads first.
    ///
    /// A graphical session that named no console owner still says somebody is
    /// there: the kind is the host's answer, and the owner is the name for it.
    var headline: String {
        switch kind {
        case .graphical:
            guard let consoleOwner, !consoleOwner.isEmpty else { return "Somebody is logged in" }
            return "Logged in as \(consoleOwner)"
        case .headless:
            return "Nobody logged in (headless)"
        case .unknown:
            return "Not reported"
        case let .unrecognised(raw):
            return raw.humanizedIdentifier
        }
    }

    enum CodingKeys: String, CodingKey {
        case kind, detail
        case consoleOwner = "console_owner"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        kind = HostLinkSessionKind(try values.decodeIfPresent(String.self, forKey: .kind) ?? "")
        consoleOwner = try values.decodeIfPresent(String.self, forKey: .consoleOwner)
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }
}

/// Whether the host has a graphical session, in the command's own word.
///
/// `unknown` is the probe's answer for "ran and could not tell", so it is a
/// case rather than an absence, and an unrecognised spelling is carried
/// through rather than folded into `headless` — reading an unfamiliar word as
/// "nobody is logged in" would invent the fact this reading reports.
enum HostLinkSessionKind: Hashable, Sendable {
    case graphical
    case headless
    case unknown
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "graphical": self = .graphical
        case "headless": self = .headless
        case "unknown": self = .unknown
        default: self = .unrecognised(raw)
        }
    }
}

/// Which way the packets went when the beacon was collected.
///
/// `unknown` is the beacon's own word for "the collector ran and could not
/// tell", so it is a case rather than an absence, and an unrecognised spelling
/// is carried through instead of folded into `unknown`.
enum HostLinkPathKind: Hashable, Sendable {
    case direct
    case relay
    case unknown
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "direct": self = .direct
        case "relay": self = .relay
        case "unknown": self = .unknown
        default: self = .unrecognised(raw)
        }
    }

    /// The beacon's own word, never a translation of it.
    var word: String {
        switch self {
        case .direct: "direct"
        case .relay: "relay"
        case .unknown: "unknown"
        case let .unrecognised(raw): raw
        }
    }

    /// A relay path is slower but working, and the collector failing to tell is
    /// not an outage either. Neither is coloured: this is a fact about the
    /// route, not a severity.
    var tone: WisentTone {
        .neutral
    }
}

/// One `link.interface_changes` entry: when the machine's interfaces moved, and
/// the collector's own description of the move.
struct HostLinkInterfaceChange: Decodable, Identifiable, Sendable {
    let at: String
    let detail: String

    var id: String { "\(at)|\(detail)" }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        at = try values.decodeIfPresent(String.self, forKey: .at) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }

    enum CodingKeys: String, CodingKey {
        case at, detail
    }
}
