import Foundation

enum NativeRouteOperations {
    static let all: [NativeCapabilityOperation] = [
        .init(id: "consumer-add", title: "Declare or update a consumer and its route", path: ["service", "directory", "consumer-add"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "consumer", label: "Consumer", required: true),
            .init(id: "capability", label: "Capabilities (one per line; blank preserves existing)", option: "--capability", multiple: true),
            .init(id: "target", label: "Resolver host (requires a loopback address)", option: "--target"),
            .init(id: "bind", label: "Loopback IP:port (requires a resolver host)", option: "--bind"),
        ]),
        .init(id: "consumer-rm", title: "Remove a consumer and all its resolver bindings", path: ["service", "directory", "consumer-rm"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "consumer", label: "Consumer to remove from every host", required: true),
        ]),
        .init(id: "open", title: "Open a declared service route", path: ["route", "open"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "target", label: "Endpoint holder (blank uses active host)", option: "--target"),
            .init(id: "direction", label: "Local API account or remote endpoint holder", required: true, choices: ["--local", "--remote"]),
        ]),
        .init(id: "close", title: "Close the service's current route", path: ["route", "close"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
            .init(id: "target", label: "Remote endpoint holder (local marker is checked first)", option: "--target"),
        ], jsonOutput: false),
        .init(id: "capability", title: "Read service credential routes", path: ["route", "capability"], hostPlacement: .none, fields: [
            .init(id: "service", label: "Service", required: true),
        ], mutates: false),
        .init(id: "key", title: "Authorize this host's resolver public key", path: ["route", "key"], hostPlacement: .positional),
        .init(id: "placement", title: "Publish declared fleet placement policies", path: ["route", "placement", "publish"], hostPlacement: .none, fields: [
            .init(id: "mobile", label: "Only active hosts declaring mobile runtime", option: "--mobile", flag: true),
        ]),
        .init(id: "placement-relief", title: "Read what placement relief would move off a host over its memory watermark", path: ["placement", "relief"], hostPlacement: .none, mutates: false),
        .init(id: "placement-move", title: "Move a placement profile to another declared host", path: ["placement", "move"], hostPlacement: .none, fields: [
            .init(id: "services", label: "Logical services naming one placement profile (one per line)", required: true, multiple: true),
            .init(id: "to_host", label: "Destination host declared by the profile", option: "--to-host", required: true),
        ]),
    ]
}
