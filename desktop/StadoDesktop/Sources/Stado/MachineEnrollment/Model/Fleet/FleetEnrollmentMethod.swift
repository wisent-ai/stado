import Foundation

/// One documented way into the fleet, exactly as the control plane reports it.
///
/// `stado fleet methods --json` is the only source of this list. The app keeps
/// no copy of it: a release that gains a method, and a registry whose catalog
/// denies one, both have to be visible here without shipping a new app.
struct FleetEnrollmentMethod: Decodable, Identifiable, Equatable, Sendable {
    let name: String
    let command: String
    let summary: String
    let requires: String
    let provides: String
    let allowed: Bool
    /// The registry field that gates the method, or nil for a method no
    /// catalog can switch off.
    let gate: String?

    var id: String { name }

    /// The screen this app can drive for the method, if it has one.
    var flow: MachineEnrollmentFlow? { MachineEnrollmentFlow(methodName: name) }

    /// Why the row is shown but not usable, in the words of whatever refused
    /// it. A disabled row that says nothing is worse than a missing one: the
    /// operator retries it, then goes looking for the fault in the machine.
    var refusal: String? {
        if !allowed {
            guard let gate, !gate.isEmpty else {
                return "The registry catalog for this fleet does not permit this method."
            }
            return "The registry catalog for this fleet sets \(gate) to false. The control plane refuses this method in its preflight, before it reaches any machine."
        }
        if flow == nil {
            return "This Stado release offers \(name), but this app has no screen for it. Run it from a terminal with \(command), or update the app."
        }
        return nil
    }

    var isOpen: Bool { refusal == nil }

    private enum CodingKeys: String, CodingKey {
        case name, command, summary, requires, provides, allowed, gate
    }

    /// Read leniently on everything except the name: a method whose prose the
    /// control plane trimmed is still a method, and dropping the whole list
    /// over a missing sentence would leave the operator with no way in at all.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        command = try values.decodeIfPresent(String.self, forKey: .command) ?? "stado fleet \(name)"
        summary = try values.decodeIfPresent(String.self, forKey: .summary) ?? ""
        requires = try values.decodeIfPresent(String.self, forKey: .requires) ?? ""
        provides = try values.decodeIfPresent(String.self, forKey: .provides) ?? ""
        allowed = try values.decodeIfPresent(Bool.self, forKey: .allowed) ?? false
        gate = try values.decodeIfPresent(String.self, forKey: .gate)
    }
}

/// `{"methods": [...]}` — the envelope `fleet methods --json` prints.
struct FleetEnrollmentMethodList: Decodable, Sendable {
    let methods: [FleetEnrollmentMethod]
}
