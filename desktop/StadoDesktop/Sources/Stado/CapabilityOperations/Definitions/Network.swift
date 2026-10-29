import Foundation

/// Stado's own host network on the selected API host: join it, read what
/// its node reaches, leave it. Each runs on the API host's machine, the one
/// whose node, key and unit it changes, so no other host is named.
enum NativeNetworkOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "network-status", title: "Read this host's Stado network node and every declared peer: reached directly, through a relay, or not, with the cause", path: ["network", "status"], hostPlacement: .none, mutates: false),
        .init(id: "network-up", title: "Join this host to the Stado network and make it the host's preferred path, after its own SSH server answers through the node", path: ["network", "up"], hostPlacement: .none, fields: [
            .init(id: "endpoint", label: "Reachable HOST:PORT peers on other networks dial (one per line; optional)", option: "--endpoint", multiple: true),
            .init(id: "no-direct", label: "Declare none of the endpoints measured on this host (reached through a relay only)", option: "--no-direct", flag: true),
            .init(id: "relay", label: "Relay for hosts peers cannot dial", option: "--relay", flag: true),
            .init(id: "attach-retry-seconds", label: "With relay: seconds a node waits before attaching again", option: "--attach-retry-seconds"),
        ]),
        .init(id: "network-down", title: "Leave the Stado network: the first alternate path becomes preferred again and the node stops", path: ["network", "down"], hostPlacement: .none),
    ]
}
