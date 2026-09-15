import Foundation

/// One network the product declares a connection path may name.
///
/// `stado registry host path list <host> --json` publishes these as
/// `known_providers`, read from `stado-rs/data/fleet/connections.json`. This
/// window holds no list of its own: an editor that offered `zerotier` while
/// the binary described something else would be a second vocabulary, and the
/// operator would have no way to tell which one the fleet honours.
struct HostConnectionNetwork: Decodable, Sendable, Hashable, Identifiable {
    let name: String
    let summary: String

    var id: String { name }
}

/// One host's declared routes beside the networks the product can describe.
struct HostConnectionPathListing: Decodable, Sendable {
    let target: String
    let connections: [Route]
    let knownProviders: [HostConnectionNetwork]

    struct Route: Decodable, Sendable, Hashable {
        let name: String
        let destination: String
        let order: Int
        let preferred: Bool
    }

    enum CodingKeys: String, CodingKey {
        case target
        case connections
        case knownProviders = "known_providers"
    }

    /// The name the product gives the host's own `ssh` destination. It is a
    /// position rather than a network, so the editor never offers it as one.
    static let preferredPathName = "primary"

    /// The networks an operator can still declare on this host: everything the
    /// product describes, minus the preferred position and minus the networks
    /// this host already has a route for.
    var networksToOffer: [HostConnectionNetwork] {
        let declared = Set(connections.map(\.name))
        return knownProviders.filter { network in
            network.name != Self.preferredPathName && !declared.contains(network.name)
        }
    }

    /// Whether the product describes this path name. A fleet-specific name is
    /// a legitimate route; the answer only decides whether the editor can show
    /// a sentence about the network.
    func describes(_ name: String) -> Bool {
        knownProviders.contains { $0.name == name }
    }

    /// The product's own sentence for one network, when it declares it.
    func summary(of name: String) -> String? {
        knownProviders.first { $0.name == name }?.summary
    }
}
