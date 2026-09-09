import Foundation
import WisentDesignSystem

/// The one word `stado web origin status --json` decides a declared public
/// origin on.
///
/// An unrecognised word is carried through rather than folded into a known
/// one. A public release origin the console cannot classify must read as
/// unclassified: reading an unfamiliar word as `serving` would report a
/// bearer-free download path as working on the strength of not knowing what
/// Stado said about it.
enum PublicOriginVerdict: Hashable, Sendable {
    case serving
    case originUndeclared
    case originNotPublic
    case resolverUnavailable
    case originUnpublished
    case originUnreachable
    case originMismatch
    case diagnosticIncomplete
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "serving": self = .serving
        case "origin-undeclared": self = .originUndeclared
        case "origin-not-public": self = .originNotPublic
        case "resolver-unavailable": self = .resolverUnavailable
        case "origin-unpublished": self = .originUnpublished
        case "origin-unreachable": self = .originUnreachable
        case "origin-mismatch": self = .originMismatch
        case "diagnostic-incomplete": self = .diagnosticIncomplete
        default: self = .unrecognised(raw)
        }
    }

    /// The command's own word, never a translation of it.
    var word: String {
        switch self {
        case .serving: "serving"
        case .originUndeclared: "origin-undeclared"
        case .originNotPublic: "origin-not-public"
        case .resolverUnavailable: "resolver-unavailable"
        case .originUnpublished: "origin-unpublished"
        case .originUnreachable: "origin-unreachable"
        case .originMismatch: "origin-mismatch"
        case .diagnosticIncomplete: "diagnostic-incomplete"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var title: String {
        switch self {
        case .serving: "Serving"
        case .originUndeclared: "Not declared"
        case .originNotPublic: "Not public"
        case .resolverUnavailable: "Resolver unavailable"
        case .originUnpublished: "Not published"
        case .originUnreachable: "Unreachable"
        case .originMismatch: "The edge selects another origin"
        case .diagnosticIncomplete: "Diagnostic reads incomplete"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }

    /// What this verdict means for a public, bearer-free read. Stado's own
    /// sentence about the origin arrives separately in `origin_error` and is
    /// always shown beside this.
    var effect: String {
        switch self {
        case .serving:
            "The declared hostname resolves publicly, its publication carries every declared path, and the edge selects this origin."
        case .originUndeclared:
            "No public origin is declared under this name, so nothing converges it and no report can say what a public edge should fetch."
        case .originNotPublic:
            "The declared hostname has no public A or AAAA record, so nothing outside this deployment's own network can reach it, whatever it is serving."
        case .resolverUnavailable:
            "The resolver could not be asked, so this is not evidence that the hostname is missing."
        case .originUnpublished:
            "The host is not publishing every declared path, so the edge would fetch a path this origin does not answer."
        case .originUnreachable:
            "The separate origin request failed. Its recorded phase and error show what failed; DNS resolution alone does not establish the cause."
        case .originMismatch:
            "The edge selects a different origin than the one declared here, so converging this declaration would not change what a release client reads."
        case .diagnosticIncomplete:
            "One or more reads failed or did not finish. Completed readings remain visible; unknown state is not evidence that the origin is down."
        case .unrecognised:
            "This Stado returned a verdict this console does not classify. It is shown in the command's own word rather than read as any known state."
        }
    }

    var tone: WisentTone {
        switch self {
        case .serving: .success
        case .resolverUnavailable, .diagnosticIncomplete, .unrecognised: .warning
        case .originUndeclared, .originNotPublic, .originUnpublished, .originUnreachable, .originMismatch: .danger
        }
    }

    var needsAttention: Bool {
        if case .serving = self { return false }
        return true
    }
}

/// What the resolver answered about the declared hostname: the same three
/// words `/docs/channels` specifies for `originDiagnosis`, and no fourth one
/// invented here.
enum PublicOriginResolutionState: Hashable, Sendable {
    case dnsUnresolved
    case dnsResolved
    case dnsUnavailable
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "dns_unresolved": self = .dnsUnresolved
        case "dns_resolved": self = .dnsResolved
        case "dns_unavailable": self = .dnsUnavailable
        default: self = .unrecognised(raw)
        }
    }

    var word: String {
        switch self {
        case .dnsUnresolved: "dns_unresolved"
        case .dnsResolved: "dns_resolved"
        case .dnsUnavailable: "dns_unavailable"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var title: String {
        switch self {
        case .dnsUnresolved: "No public address"
        case .dnsResolved: "Resolves publicly"
        case .dnsUnavailable: "Resolver could not be asked"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }

    /// A resolver that could not be asked is not a missing name, so it is
    /// never coloured as one.
    var tone: WisentTone {
        switch self {
        case .dnsResolved: .success
        case .dnsUnresolved: .danger
        case .dnsUnavailable, .unrecognised: .warning
        }
    }
}

/// Whether the host is publishing the declared paths at all.
enum PublicOriginPublicationState: Hashable, Sendable {
    case published
    case unpublished
    case unknown
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "published": self = .published
        case "unpublished": self = .unpublished
        case "unknown": self = .unknown
        default: self = .unrecognised(raw)
        }
    }

    var word: String {
        switch self {
        case .published: "published"
        case .unpublished: "unpublished"
        case .unknown: "unknown"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var title: String {
        switch self {
        case .published: "Published"
        case .unpublished: "Not published"
        case .unknown: "Not reported"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }

    /// `unknown` is the reader's own word for "ran and could not tell", so it
    /// is neither healthy nor a failure.
    var tone: WisentTone {
        switch self {
        case .published: .success
        case .unpublished: .danger
        case .unknown, .unrecognised: .warning
        }
    }
}

/// Whether the public edge selects the origin this declaration names.
enum PublicOriginEdgeSelectionState: Hashable, Sendable {
    case agrees
    case differs
    case undeclared
    case unreadable
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "agrees": self = .agrees
        case "differs": self = .differs
        case "undeclared": self = .undeclared
        case "unreadable": self = .unreadable
        default: self = .unrecognised(raw)
        }
    }

    var word: String {
        switch self {
        case .agrees: "agrees"
        case .differs: "differs"
        case .undeclared: "undeclared"
        case .unreadable: "unreadable"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var title: String {
        switch self {
        case .agrees: "The edge selects this origin"
        case .differs: "The edge selects a different origin"
        case .undeclared: "The edge declares no origin"
        case .unreadable: "The edge could not be read"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }

    var tone: WisentTone {
        switch self {
        case .agrees: .success
        case .differs, .undeclared: .danger
        case .unreadable, .unrecognised: .warning
        }
    }
}

/// What one convergence did.
enum PublicOriginConvergeStatus: Hashable, Sendable {
    case converged
    case unchanged
    case refused
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "converged": self = .converged
        case "unchanged": self = .unchanged
        case "refused": self = .refused
        default: self = .unrecognised(raw)
        }
    }

    var word: String {
        switch self {
        case .converged: "converged"
        case .unchanged: "unchanged"
        case .refused: "refused"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var title: String {
        switch self {
        case .converged: "Converged"
        case .unchanged: "Unchanged"
        case .refused: "Refused"
        case let .unrecognised(raw): raw.isEmpty ? "Not reported" : raw.humanizedIdentifier
        }
    }

    var tone: WisentTone {
        switch self {
        case .converged: .success
        case .unchanged: .neutral
        case .refused: .danger
        case .unrecognised: .warning
        }
    }
}
