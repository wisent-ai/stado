import SwiftUI
import WisentDesignSystem

extension HostsView {
    /// The one line a healthy link earns: how fresh the beacon is and, when the
    /// beacon actually carried a link block, which way the packets went.
    ///
    /// A bare `unknown` path is left off this line on purpose. It is the
    /// command's word for "there was no link block to read", and appending it
    /// here reads as a diagnosis of the route rather than the absence of one.
    /// The Network path field below still carries the command's own word.
    func healthyLinkLine(_ link: HostLink) -> String {
        var text = "Healthy"
        if let age = link.beaconAgeSeconds {
            text += " · beacon \(ConsoleFormat.age(Double(age)))"
        }
        if link.linkReported, let kind = link.pathKind {
            text += " · \(kind.word)"
            if let endpoint = link.endpoint, !endpoint.isEmpty {
                text += " \(endpoint)"
            }
        }
        return text
    }

    func pathDescription(_ link: HostLink) -> String {
        guard let kind = link.pathKind else { return "Not reported" }
        guard let endpoint = link.endpoint, !endpoint.isEmpty else { return kind.word }
        return "\(kind.word) \(endpoint)"
    }

    /// Preferred host-control route followed by every declared fallback.
    ///
    /// The selector probes all of them, then chooses one for a real operation.
    /// Showing every answer is what keeps a healthy primary from hiding a
    /// fallback that will fail during the outage it exists for.
    func connectionPathsDescription(_ link: HostLink) -> String {
        if let error = link.connectionProbeError, !error.isEmpty {
            return "Routes could not be probed\n\(error)"
        }
        guard !link.connectionPaths.isEmpty else { return "Not reported" }
        return link.connectionPaths.map { path in
            var labels: [String] = []
            if path.name == link.selectedConnection {
                labels.append("selected")
            }
            if path.reachable {
                labels.append("answered")
            } else if let error = path.error, !error.isEmpty {
                labels.append("did not answer: \(error)")
            } else {
                labels.append("did not answer")
            }
            return "\(path.name) · \(labels.joined(separator: " · "))\n\(path.destination)"
        }
        .joined(separator: "\n")
    }

    /// The selected host's forward markers, one line each, or the reason the
    /// read produced none.
    func forwardDescription(_ link: HostLink) -> String {
        guard forwardStore.host == link.host else { return "Not read yet" }
        if forwardStore.isLoading { return "Reading…" }
        if let problem = forwardStore.problem { return problem }
        guard !forwardStore.markers.isEmpty else {
            return "This host publishes no service address markers"
        }
        return forwardStore.markers
            .map { marker in
                let declared: String
                switch marker.declarationVerdict {
                case "matches": declared = marker.declaredSource
                case "disagrees":
                    declared =
                        "declared \(marker.declaredUrl ?? "elsewhere") (\(marker.declaredSource))"
                default: declared = "undeclared"
                }
                return "\(marker.name): \(marker.url) — \(marker.reconciliation), \(declared)"
            }
            .joined(separator: "\n")
    }

    /// Danger for a marker that disagrees with the fleet's own declaration,
    /// warning for one nothing answers at or one nothing declares, neutral
    /// otherwise.
    func forwardTone(_ link: HostLink) -> WisentTone {
        guard forwardStore.host == link.host, forwardStore.problem == nil else { return .neutral }
        if forwardStore.markers.contains(where: { $0.declarationVerdict == "disagrees" }) {
            return .danger
        }
        if forwardStore.markers.contains(where: {
            $0.reconciliation != "matches" || $0.declarationVerdict == "undeclared"
        }) {
            return .warning
        }
        return .neutral
    }

    /// The resolved vault first, then every candidate the host holds, then the
    /// resolution's own sentence when it refuses.
    func vaultDescription(_ link: HostLink) -> String {
        guard vaultStore.host == link.host else { return "Not read yet" }
        if vaultStore.isLoading { return "Reading…" }
        if let problem = vaultStore.problem { return problem }
        guard let authority = vaultStore.authority else { return "Not reported" }
        var lines: [String] = []
        if let path = authority.path, !path.isEmpty {
            lines.append("\(authority.state): \(path)")
        } else {
            lines.append(authority.state)
        }
        if let detail = authority.detail, !detail.isEmpty {
            lines.append(detail)
        }
        for vault in vaultStore.vaults {
            let items = vault.items.map { "\($0) items" } ?? "unknown items"
            let owner = vault.owner ?? "unknown owner"
            let marker = vault.path == authority.path ? "* " : "  "
            lines.append("\(marker)\(items) · \(owner) · \(vault.path)")
        }
        return lines.joined(separator: "\n")
    }

    /// Danger when the host resolves no vault — every owner write and
    /// authoritative read there is refused — warning when its release cannot
    /// report the declaration, neutral when it resolves one.
    func vaultTone(_ link: HostLink) -> WisentTone {
        guard vaultStore.host == link.host, vaultStore.problem == nil else { return .neutral }
        switch vaultStore.authority?.state {
        case "ambiguous", "declared-absent", "none": return .danger
        case "unreadable": return .warning
        default: return .neutral
        }
    }

    func connectionPathsTone(_ link: HostLink) -> WisentTone {
        if link.connectionProbeError != nil
            || (!link.connectionPaths.isEmpty && link.selectedConnection == nil) {
            return .danger
        }
        return link.connectionPaths.contains(where: { !$0.reachable }) ? .warning : .neutral
    }

    /// The plain words first, then the host's own evidence for them.
    ///
    /// The headline is what an operator asked for: is anybody logged in there.
    /// The command's sentence beneath it names the console device and the
    /// launchd domain, which is what an operator needs the moment they doubt
    /// the headline — and dropping it would leave this console asserting a
    /// fact with the evidence removed.
    func sessionDescription(_ link: HostLink) -> String {
        guard let session = link.session, !session.detail.isEmpty else { return link.sessionLine }
        return "\(session.headline)\n\(session.detail)"
    }

    /// The stamp the collector recorded and how long ago that was. The stamp
    /// alone answers "did it sleep at 18:29"; the age alone answers "was that
    /// during the gap". The incident needed both.
    func stampDescription(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "Not reported" }
        guard let date = StadoFormat.date(value) else { return value }
        return "\(value) · \(ConsoleFormat.age(Date().timeIntervalSince(date)))"
    }

    func interfaceDescription(_ link: HostLink) -> String {
        guard !link.interfaceChanges.isEmpty else {
            return link.linkReported ? "None recorded" : "Not reported"
        }
        var lines = link.interfaceChanges.prefix(3).map { "\($0.at) — \($0.detail)" }
        if link.interfaceChanges.count > 3 {
            lines.append("and \((link.interfaceChanges.count - 3).formatted(.number)) more")
        }
        return lines.joined(separator: "\n")
    }

    func silenceDescription(_ link: HostLink) -> String {
        guard !link.silences.isEmpty else { return "None recorded" }
        return link.silences.prefix(5).map { silence -> String in
            var line = silence.startedAt
            if let ended = silence.endedAt {
                line += " → \(ended)"
            } else {
                line += " → still quiet"
            }
            line += " · \(StadoFormat.duration(silence.elapsedSeconds))"
            if !silence.observedBy.isEmpty {
                line += " · seen by \(silence.observedBy.joined(separator: ", "))"
            }
            return line
        }
        .joined(separator: "\n")
    }

    func refusalDescription(_ refusals: HostReaderRefusals?) -> String {
        guard let refusals else { return "Not reported" }
        let window = StadoFormat.duration(Double(refusals.windowSeconds))
        guard refusals.count > 0 else { return "None in the last \(window)" }
        let reasons = refusals.rankedReasons
            .map { "\($0.reason) \($0.count.formatted(.number))" }
            .joined(separator: " · ")
        let head = "\(refusals.count.formatted(.number)) in the last \(window)"
        return reasons.isEmpty ? head : "\(head)\n\(reasons)"
    }
}
