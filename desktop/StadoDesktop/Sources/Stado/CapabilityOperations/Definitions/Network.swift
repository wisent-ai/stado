import Foundation

/// Stado's own host network on the selected API host: join it, read what
/// its node reaches, leave it. Each runs on the API host's machine, the one
/// whose node, key and unit it changes, so no other host is named. Beside
/// it, the fleet's public web edge, every `stado web edge` verb.
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
        // The fleet's public web edge: every security group between the
        // Internet and it, whether 80 and 443 answer, and the repair that
        // opens a group dropping them to the edge's own address only.
        .init(id: "web-edge-status", title: "Read the web edge: each security group between the Internet and it, whether 80 and 443 answer within its probe deadline, and what it terminates", path: ["web", "edge", "status"], hostPlacement: .none, mutates: false),
        .init(id: "web-edge-repair", title: "Open 80 and 443 in every security group that drops them, to the edge's own private address only", path: ["web", "edge", "repair"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Edge name, as provision created it", required: true),
        ]),
        .init(id: "web-edge-provision", title: "Create the web edge on Azure, open its path from the Internet, and record it", path: ["web", "edge", "provision"], hostPlacement: .none, fields: [
            .init(id: "name", label: "Edge name: lowercase letters, digits and dashes", required: true),
            .init(id: "region", label: "Azure region with a pre-provisioned vnet and subnet", option: "--region", required: true),
            .init(id: "size", label: "Azure VM size, matching the image's architecture", option: "--size", required: true),
            .init(id: "image", label: "Image URN publisher:offer:sku:version", option: "--image", required: true),
            .init(id: "disk-gb", label: "OS disk in GiB", option: "--disk-gb", required: true),
            .init(id: "contact", label: "Mail address Let's Encrypt warns of expiry at", option: "--contact", required: true),
            .init(id: "probe-seconds", label: "Seconds status waits for 80 and 443 to answer", option: "--probe-seconds", required: true),
        ]),
        .init(id: "web-edge-declare", title: "Record an edge host that already exists, provisioning nothing", path: ["web", "edge", "declare"], hostPlacement: .none, fields: [
            .init(id: "target", label: "Registry target name of the edge host", option: "--target", required: true),
            .init(id: "address", label: "Its public IPv4 address", option: "--address", required: true),
            .init(id: "contact", label: "Mail address Let's Encrypt warns of expiry at", option: "--contact", required: true),
            .init(id: "registrar-credential", label: "Skarbiec item of the registrar (blank keeps the declared one)", option: "--registrar-credential"),
            .init(id: "probe-seconds", label: "Seconds status waits for 80 and 443 (blank keeps the declared one)", option: "--probe-seconds"),
        ]),
        .init(id: "web-edge-hostnames", title: "Compare the hostnames the edge must terminate with the ones its proxy does", path: ["web", "edge", "hostnames"], hostPlacement: .none, mutates: false),
        .init(id: "web-edge-remove", title: "Delete the edge's Azure resources and forget it", path: ["web", "edge", "remove"], hostPlacement: .none, fields: [
            .init(id: "orphan-hostnames", label: "Delete even though products still name this edge", option: "--orphan-hostnames", flag: true),
            .init(id: "keep-resources", label: "Forget the declaration only, deleting nothing on Azure", option: "--keep-resources", flag: true),
        ]),
    ]
}
